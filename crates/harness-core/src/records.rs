use crate::{provider::*, store::Store, *};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
pub(crate) fn binding(c: &Connection, s: SessionId, w: &str, b: &ProviderBinding) -> Result<()> {
    let saved: Option<(String, String, String, String, String)> = c
        .query_row(
            "SELECT workspace,provider,account_ref,model,options FROM sessions WHERE id=?1",
            [s.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    match saved {
        None => Err(Error::NotFound),
        Some((sw, p, a, m, o))
            if sw == w
                && p == b.provider
                && a == b.account_ref
                && m == b.model
                && o == b.options =>
        {
            Ok(())
        }
        _ => Err(Error::Binding),
    }
}
pub(crate) async fn create_session(
    store: &Store,
    workspace: String,
    b: ProviderBinding,
    prefix: String,
    instructions: Vec<Instruction>,
) -> Result<SessionId> {
    store.call(move |c| {
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO sessions(workspace,provider,account_ref,model,options) VALUES(?1,?2,?3,?4,?5)",params![workspace,b.provider,b.account_ref,b.model,b.options])?;
        let session=SessionId(tx.last_insert_rowid());
        tx.execute("INSERT INTO epochs(session_id,state,prefix,tool_schema) VALUES(?1,'active',?2,?3)",params![session.0,prefix,context::schemas()])?;
        let epoch=tx.last_insert_rowid();
        for i in instructions { tx.execute("INSERT INTO instructions VALUES(?1,?2,?3,?4)",params![epoch,i.origin,i.sha256,i.content])?; }
        tx.commit()?; Ok(session)
    }).await
}
pub(crate) fn request(c: &Connection, session: SessionId) -> Result<ProviderRequest> {
    let epoch = c.query_row(
        "SELECT id,prefix,tool_schema FROM epochs WHERE session_id=?1 AND state='active'",
        [session.0],
        |r| {
            Ok(Epoch {
                id: ContextEpochId(r.get(0)?),
                prefix: r.get(1)?,
                tool_schema: r.get(2)?,
            })
        },
    )?;
    // Refusal retains the complete audit trail, but none of that turn's protocol
    // belongs in later provider input. Keep other turns' closed call/result pairs.
    let mut q=c.prepare("SELECT p.payload FROM protocol_messages p JOIN turns t ON t.id=p.turn_id WHERE t.session_id=?1 AND p.epoch_id=?2 AND t.state!='refused' ORDER BY p.id")?;
    let rows = q.query_map(params![session.0, epoch.id.0], |r| r.get::<_, String>(0))?;
    let messages = rows
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok(ProviderRequest { epoch, messages })
}
pub(crate) async fn start(
    store: &Store,
    workspace: String,
    session: SessionId,
    prompt: String,
    limits: String,
    b: ProviderBinding,
) -> Result<(TurnId, ProviderRequest, EventSequence)> {
    store.call(move |c| {
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        binding(&tx,session,&workspace,&b)?;
        if let Some(work) = unresolved(&tx,&workspace)? { return Err(Error::UnresolvedWork(work)); }
        let epoch:i64=tx.query_row("SELECT id FROM epochs WHERE session_id=?1 AND state='active'",[session.0],|r|r.get(0))?;
        tx.execute("INSERT INTO turns(session_id,workspace,epoch_id,state,limits) VALUES(?1,?2,?3,'running',?4)",params![session.0,workspace,epoch,limits])?;
        let turn=TurnId(tx.last_insert_rowid());
        append(&tx,turn,&ProtocolMessage::User(prompt))?;
        let sequence=event(&tx,session,turn,EventKind::TurnStarted{turn_id:turn})?;
        let request=request(&tx,session)?;
        tx.commit()?; Ok((turn,request,sequence))
    }).await
}
pub(crate) fn unresolved(c: &Connection, workspace: &str) -> Result<Option<UnresolvedWork>> {
    let ids = c.query_row("SELECT session_id,id FROM turns WHERE workspace=?1 AND state IN ('running','awaiting_approval','suspended_limit','suspended_recovery','interrupted') ORDER BY id LIMIT 1", [workspace], |r| Ok((SessionId(r.get(0)?), TurnId(r.get(1)?)))).optional()?;
    ids.map(|(session_id, turn_id)| {
        let data_dir = std::path::Path::new(c.path().ok_or(Error::Worker)?)
            .parent()
            .ok_or(Error::Worker)?
            .to_path_buf();
        Ok(UnresolvedWork {
            session_id,
            turn_id,
            data_dir,
        })
    })
    .transpose()
}
pub(crate) fn append(tx: &Transaction<'_>, turn: TurnId, message: &ProtocolMessage) -> Result<i64> {
    tx.execute("INSERT INTO protocol_messages(turn_id,epoch_id,payload) SELECT id,epoch_id,?1 FROM turns WHERE id=?2",params![serde_json::to_string(message)?,turn.0])?;
    let id = tx.last_insert_rowid();
    // The original M0 table remains intact for migrations; new history is derived
    // from complete protocol messages, including opaque state and tool results.
    Ok(id)
}
pub(crate) async fn finish(
    store: &Store,
    session: SessionId,
    outcome: TurnOutcome,
    active_ms: i64,
) -> Result<EventSequence> {
    store.call(move |c| {
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let turn=outcome.turn_id;
        let state=serde_json::to_value(outcome.state)?.as_str().ok_or(Error::Worker)?.to_owned();
        let changed=tx.execute("UPDATE turns SET state=?1,outcome=?2,active_ms=MAX(active_ms,?3) WHERE id=?4 AND session_id=?5 AND state='running'",params![state,serde_json::to_string(&outcome)?,active_ms,turn.0,session.0])?;
        if changed!=1 {return Err(Error::Persistence("turn transition conflict".into()));}
        tx.execute("UPDATE model_attempts SET state='incomplete',stop_reason=?1 WHERE turn_id=?2 AND state='running'",params![serde_json::to_string(&outcome.reason)?,turn.0])?;
        let sequence=event(&tx,session,turn,EventKind::TurnStopped{outcome})?;
        tx.commit()?; Ok(sequence)
    }).await
}
pub(crate) fn event(
    tx: &Transaction<'_>,
    session: SessionId,
    turn: TurnId,
    event: EventKind,
) -> Result<EventSequence> {
    let seq = tx.query_row(
        "UPDATE sessions SET next_sequence=next_sequence+1 WHERE id=?1 RETURNING next_sequence-1",
        [session.0],
        |r| r.get::<_, i64>(0),
    )?;
    tx.execute(
        "INSERT INTO events VALUES(?1,?2,?3,?4)",
        params![session.0, seq, turn.0, serde_json::to_string(&event)?],
    )?;
    Ok(EventSequence(seq))
}
pub(crate) async fn history(
    store: &Store,
    workspace: String,
    session: SessionId,
    b: ProviderBinding,
) -> Result<Vec<Message>> {
    store.call(move |c| {binding(c,session,&workspace,&b)?;
        let mut q=c.prepare("SELECT payload FROM protocol_messages p JOIN turns t ON t.id=p.turn_id WHERE t.session_id=?1 ORDER BY p.id")?;
        let rows=q.query_map([session.0],|r|r.get::<_,String>(0))?;
        rows.map(|r| { let m=serde_json::from_str::<ProtocolMessage>(&r?)?; Ok(match m {
            ProtocolMessage::User(text)=>Message{role:Role::User,text},
            ProtocolMessage::Assistant{text,..}=>Message{role:Role::Assistant,text},
            ProtocolMessage::AssistantData(m)=>Message{role:Role::Assistant,text:m.text},
            ProtocolMessage::ToolResult{result,..}=>Message{role:Role::Tool,text:result},
        }) }).collect()
    }).await
}
pub(crate) async fn events(
    store: &Store,
    workspace: String,
    session: SessionId,
    after: EventSequence,
    b: ProviderBinding,
) -> Result<Vec<DurableEvent>> {
    store.call(move |c| {binding(c,session,&workspace,&b)?;
        let mut q=c.prepare("SELECT sequence,payload FROM events WHERE session_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 256")?;
        let rows=q.query_map(params![session.0,after.0],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?;
        rows.map(|r|{let(s,p)=r?;Ok(DurableEvent{sequence:EventSequence(s),event:serde_json::from_str(&p)?})}).collect()
    }).await
}
pub(crate) async fn outcome(
    store: &Store,
    workspace: String,
    session: SessionId,
    turn: TurnId,
    b: ProviderBinding,
) -> Result<Option<TurnOutcome>> {
    store
        .call(move |c| {
            binding(c, session, &workspace, &b)?;
            let v: Option<Option<String>> = c
                .query_row(
                    "SELECT outcome FROM turns WHERE id=?1 AND session_id=?2",
                    params![turn.0, session.0],
                    |r| r.get(0),
                )
                .optional()?;
            v.ok_or(Error::NotFound)?
                .map(|s| Ok(serde_json::from_str(&s)?))
                .transpose()
        })
        .await
}
