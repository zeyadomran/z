use crate::{
    provider::*,
    records::{append, event},
    store::Store,
    *,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
pub(crate) const ACTIVE_CHECKPOINT_MS: i64 = 1000;
#[derive(Debug, Clone, serde::Serialize)]
pub struct UsageRecord {
    pub attempt: i64,
    pub state: String,
    pub raw_usage: Option<serde_json::Value>,
    pub metadata: Option<Progress>,
}
#[derive(Default)]
pub(crate) struct Budget {
    pub attempts: usize,
    pub tools: usize,
    pub output: usize,
    pub active_ms: i64,
}
pub(crate) async fn budget(s: &Store, t: TurnId) -> Result<Budget> {
    s.call(move|c|Ok(c.query_row("SELECT (SELECT count(*) FROM model_attempts WHERE turn_id=t.id),tool_count,output_bytes,active_ms FROM turns t WHERE id=?1",[t.0],|r|Ok(Budget{attempts:r.get::<_,u32>(0)? as usize,tools:r.get::<_,u32>(1)? as usize,output:r.get::<_,u32>(2)? as usize,active_ms:r.get(3)?}))?)).await
}
pub(crate) async fn active_checkpoint(
    s: &Store,
    session: SessionId,
    t: TurnId,
    ms: i64,
) -> Result<Option<EventSequence>> {
    s.call(move |c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE turns SET active_ms=MAX(active_ms,?1) WHERE id=?2 AND state='running'",
            params![ms, t.0],
        )?;
        let seq = if changed == 1 {
            Some(event(
                &tx,
                session,
                t,
                EventKind::ActiveTimeCheckpoint {
                    turn_id: t,
                    active_ms: ms,
                },
            )?)
        } else {
            None
        };
        tx.commit()?;
        Ok(seq)
    })
    .await
}
fn recover_active(tx: &rusqlite::Transaction<'_>, session: SessionId, t: TurnId) -> Result<()> {
    // No wall-clock catch-up: the process may have been offline for hours.
    // Retain durable consumption and reserve one uncertain checkpoint interval.
    tx.execute("UPDATE turns SET active_ms=active_ms+?1,output_bytes=CASE WHEN EXISTS(SELECT 1 FROM model_attempts WHERE turn_id=?2 AND state='running') THEN MAX(output_bytes,COALESCE(json_extract(limits,'$.max_output_bytes'),0)) ELSE output_bytes END WHERE id=?2", params![ACTIVE_CHECKPOINT_MS, t.0])?;
    event(
        tx,
        session,
        t,
        EventKind::ActiveTimeRecovered {
            turn_id: t,
            uncertain_ms: ACTIVE_CHECKPOINT_MS,
        },
    )?;
    Ok(())
}
pub(crate) async fn attempt(s: &Store, session: SessionId, t: TurnId, ms: i64) -> Result<i64> {
    s.call(move |c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE turns SET active_ms=MAX(active_ms,?1) WHERE id=?2",
            params![ms, t.0],
        )?;
        tx.execute(
            "INSERT INTO model_attempts(turn_id,state,started_unix_ms) VALUES(?1,'running',?2)",
            params![t.0, unix_ms()],
        )?;
        let id = tx.last_insert_rowid();
        event(
            &tx,
            session,
            t,
            EventKind::AttemptStarted {
                turn_id: t,
                attempt: id,
            },
        )?;
        tx.commit()?;
        Ok(id)
    })
    .await
}
pub(crate) async fn response(
    s: &Store,
    session: SessionId,
    t: TurnId,
    a: i64,
    r: ProviderResponse,
    p: Progress,
    ms: i64,
) -> Result<Option<i64>> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let state=if r.message.is_some(){"completed"}else if r.reason==StopReason::Refused{"refused"}else{"incomplete"};
    tx.execute("UPDATE model_attempts SET state=?1,stop_reason=?2,raw_usage=?3,metadata=?4 WHERE id=?5",params![state,serde_json::to_string(&r.reason)?,p.usage.as_ref().map(serde_json::to_string).transpose()?,serde_json::to_string(&p)?,a])?;
    tx.execute("UPDATE turns SET active_ms=MAX(active_ms,?1),output_bytes=output_bytes+?2 WHERE id=?3",params![ms,p.bytes as i64,t.0])?;
    event(&tx,session,t,EventKind::Usage{turn_id:t,raw:p.usage})?;
    let mut batch=None;
    if let Some(message)=r.message {
        let id=append(&tx,t,&ProtocolMessage::AssistantData(message.clone()))?;
        event(&tx,session,t,EventKind::AssistantCompleted{turn_id:t})?;
        if !message.calls.is_empty() {
            batch=Some(id);
            for (ordinal,call) in message.calls.iter().enumerate(){
                tx.execute("INSERT INTO tool_calls(turn_id,batch_id,ordinal,provider_id,name,arguments,state) VALUES(?1,?2,?3,?4,?5,?6,'proposed')",params![t.0,id,ordinal as i64,call.id,call.name,call.arguments])?;
                event(&tx,session,t,EventKind::ToolProposed{turn_id:t,call_id:call.id.clone()})?;
            }
        } else { checkpoint(&tx,t,id)?; }
    }
    tx.commit()?;Ok(batch)
}).await
}
fn checkpoint(tx: &rusqlite::Transaction<'_>, t: TurnId, id: i64) -> Result<()> {
    tx.execute("INSERT OR REPLACE INTO checkpoints SELECT id,?1,active_ms,output_bytes,tool_count FROM turns WHERE id=?2",params![id,t.0])?;
    Ok(())
}
pub(crate) async fn intent(s: &Store, batch: i64, id: String, t: TurnId) -> Result<()> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if tx.execute("UPDATE tool_calls SET state='intent' WHERE batch_id=?1 AND provider_id=?2 AND state='proposed'",params![batch,id])?!=1{return Err(Error::Persistence("tool intent conflict".into()));}
    tx.execute("UPDATE turns SET tool_count=tool_count+1 WHERE id=?1",[t.0])?;tx.commit()?;Ok(())
}).await
}
pub(crate) async fn tool_result(
    s: &Store,
    session: SessionId,
    t: TurnId,
    batch: i64,
    id: String,
    result: String,
    ms: i64,
) -> Result<()> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let value: serde_json::Value = serde_json::from_str(&result)?;
    if let Some(instructions) = value.get("instructions") {
        for i in serde_json::from_value::<Vec<Instruction>>(instructions.clone())? {
            tx.execute("INSERT OR IGNORE INTO instructions SELECT epoch_id,?1,?2,?3 FROM turns WHERE id=?4", params![i.origin,i.sha256,i.content,t.0])?;
        }
    }
    if tx.execute("UPDATE tool_calls SET state='resolved',result=?1 WHERE batch_id=?2 AND provider_id=?3 AND state!='resolved'",params![result,batch,id])?!=1{return Err(Error::Persistence("tool result conflict".into()));}
    tx.execute("UPDATE turns SET output_bytes=output_bytes+?1,active_ms=MAX(active_ms,?2) WHERE id=?3",params![result.len() as i64,ms,t.0])?;
    event(&tx,session,t,EventKind::ToolFinished{turn_id:t,call_id:id})?;tx.commit()?;Ok(())
}).await
}
/// Only suppress snapshots that still belong to provider context, including
/// accepted results in the current open batch. Refused turns remain audit-only.
pub(crate) async fn instruction_keys(
    s: &Store,
    t: TurnId,
) -> Result<std::collections::BTreeSet<(String, String)>> {
    s.call(move |c| {
        let mut q = c.prepare("SELECT DISTINCT i.origin,i.sha256 FROM instructions i JOIN turns t ON t.epoch_id=i.epoch_id JOIN tool_calls tc ON tc.turn_id=t.id JOIN json_each(tc.result,'$.instructions') j WHERE i.epoch_id=(SELECT epoch_id FROM turns WHERE id=?1) AND t.state!='refused' AND json_extract(j.value,'$.origin')=i.origin AND json_extract(j.value,'$.sha256')=i.sha256")?;
        Ok(q.query_map([t.0], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?)
    }).await
}
pub(crate) async fn close(
    s: &Store,
    session: SessionId,
    t: TurnId,
    batch: i64,
) -> Result<ProviderRequest> {
    s.call(move |c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        close_tx(&tx, session, t, batch)?;
        let request = records::request(&tx, session)?;
        tx.commit()?;
        Ok(request)
    })
    .await
}
fn close_tx(
    tx: &rusqlite::Transaction<'_>,
    session: SessionId,
    t: TurnId,
    batch: i64,
) -> Result<()> {
    let rows = {
        let mut q = tx.prepare(
            "SELECT provider_id,result FROM tool_calls WHERE batch_id=?1 ORDER BY ordinal",
        )?;
        q.query_map([batch], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut last = batch;
    for (id, result) in rows {
        last = append(
            tx,
            t,
            &ProtocolMessage::ToolResult {
                call_id: id,
                result: result.ok_or_else(|| Error::Persistence("batch still pending".into()))?,
            },
        )?;
    }
    checkpoint(tx, t, last)?;
    event(tx, session, t, EventKind::BatchClosed { turn_id: t })?;
    Ok(())
}
fn recover_final(
    tx: &rusqlite::Transaction<'_>,
    session: SessionId,
    t: TurnId,
) -> Result<Option<TurnOutcome>> {
    let payload: Option<String> = tx
        .query_row(
            "SELECT payload FROM protocol_messages WHERE turn_id=?1 ORDER BY id DESC LIMIT 1",
            [t.0],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(payload) = payload
        && let ProtocolMessage::AssistantData(message) = serde_json::from_str(&payload)?
        && message.calls.is_empty()
    {
        let outcome = TurnOutcome {
            turn_id: t,
            state: TurnState::Completed,
            reason: StopReason::Final,
            text: Some(message.text),
        };
        tx.execute(
            "UPDATE turns SET state='completed',outcome=?1 WHERE id=?2",
            params![serde_json::to_string(&outcome)?, t.0],
        )?;
        event(
            tx,
            session,
            t,
            EventKind::TurnStopped {
                outcome: outcome.clone(),
            },
        )?;
        return Ok(Some(outcome));
    }
    Ok(None)
}
/// Called only while holding the workspace OS lock. M1a has no mutating tools:
/// unfinished inspections are truthfully skipped, completed results are reused.
pub(crate) async fn reconcile(s: &Store, w: String) -> Result<()> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let turns={let mut q=tx.prepare("SELECT id,session_id FROM turns WHERE workspace=?1 AND state='running'")?;q.query_map([w],|r|Ok((TurnId(r.get(0)?),SessionId(r.get(1)?))))?.collect::<std::result::Result<Vec<_>,_>>()?};
    for(t,session) in turns {
        // A fully stored final response is authoritative even if the process
        // died before the terminal transition. Never request its replacement.
        if recover_final(&tx, session, t)?.is_some() {
            continue;
        }
        recover_active(&tx,session,t)?;
        let batches={let mut q=tx.prepare("SELECT DISTINCT batch_id FROM tool_calls WHERE turn_id=?1 AND batch_id>COALESCE((SELECT closed_message_id FROM checkpoints WHERE turn_id=?1),0)")?;q.query_map([t.0],|r|r.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?};
        tx.execute("UPDATE tool_calls SET state='resolved',result='{\"error\":\"interrupted inspection; no completed result\"}' WHERE turn_id=?1 AND state!='resolved'",[t.0])?;
        for batch in batches{close_tx(&tx,session,t,batch)?;}
        tx.execute("UPDATE model_attempts SET state='incomplete',stop_reason='\"interrupted\"' WHERE turn_id=?1 AND state='running'",[t.0])?;
        let outcome=TurnOutcome{turn_id:t,state:TurnState::Interrupted,reason:StopReason::Interrupted,text:None};
        tx.execute("UPDATE turns SET state='interrupted',outcome=?1 WHERE id=?2",params![serde_json::to_string(&outcome)?,t.0])?;
        event(&tx,session,t,EventKind::TurnStopped{outcome})?;
    }
    tx.commit()?;Ok(())
}).await
}
pub(crate) async fn resume(
    s: &Store,
    w: String,
    session: SessionId,
    t: TurnId,
    b: ProviderBinding,
    limits: String,
) -> Result<ProviderRequest> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;records::binding(&tx,session,&w,&b)?;
    if tx.execute("UPDATE turns SET state='running',outcome=NULL,limits=?1 WHERE id=?2 AND session_id=?3 AND (state IN ('interrupted','suspended_limit') OR (state='suspended_recovery' AND json_extract(outcome,'$.reason') IN ('consent_required','reauthorization_required','subscription_limit','access_blocked','account_diagnosis')))",params![limits,t.0,session.0])?!=1{return Err(Error::Busy);}
    let r=records::request(&tx,session)?;tx.commit()?;Ok(r)
}).await
}
/// The caller holds OS ownership, so a running record is orphaned. This v0.1
/// operation only closes read-only inspections; it never invents tool success.
pub(crate) async fn abandon(
    s: &Store,
    w: String,
    session: SessionId,
    t: TurnId,
) -> Result<TurnOutcome> {
    s.call(move |c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved_workspace: Option<String> = tx.query_row(
            "SELECT workspace FROM sessions WHERE id=?1", [session.0], |r| r.get(0),
        ).optional()?;
        match saved_workspace {
            None => return Err(Error::NotFound),
            Some(saved) if saved != w => return Err(Error::Binding),
            _ => {},
        }
        let (state, saved): (String, Option<String>) = tx.query_row(
            "SELECT state,outcome FROM turns WHERE id=?1 AND session_id=?2",
            params![t.0, session.0], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?.ok_or(Error::NotFound)?;
        if matches!(state.as_str(), "completed" | "failed" | "refused" | "cancelled") {
            return Ok(serde_json::from_str(&saved.ok_or(Error::Worker)?)?);
        }
        if let Some(outcome) = recover_final(&tx, session, t)? {
            tx.commit()?;
            return Ok(outcome);
        }
        if state == "running" { recover_active(&tx, session, t)?; }
        let batches = {
            let mut q = tx.prepare("SELECT DISTINCT batch_id FROM tool_calls WHERE turn_id=?1 AND batch_id>COALESCE((SELECT closed_message_id FROM checkpoints WHERE turn_id=?1),0)")?;
            q.query_map([t.0], |r| r.get::<_, i64>(0))?.collect::<std::result::Result<Vec<_>, _>>()?
        };
        tx.execute("UPDATE tool_calls SET state='resolved',result=CASE WHEN state='intent' THEN '{\"error\":\"abandoned inspection; no completed result\"}' ELSE '{\"error\":\"cancelled before inspection\"}' END WHERE turn_id=?1 AND state!='resolved'", [t.0])?;
        for batch in batches { close_tx(&tx, session, t, batch)?; }
        tx.execute("UPDATE model_attempts SET state='incomplete',stop_reason='\"cancelled\"' WHERE turn_id=?1 AND state='running'", [t.0])?;
        let outcome = TurnOutcome { turn_id: t, state: TurnState::Cancelled, reason: StopReason::Cancelled, text: None };
        tx.execute("UPDATE turns SET state='cancelled',outcome=?1 WHERE id=?2", params![serde_json::to_string(&outcome)?, t.0])?;
        event(&tx, session, t, EventKind::TurnStopped { outcome: outcome.clone() })?;
        tx.commit()?;
        Ok(outcome)
    }).await
}
pub(crate) async fn usage(
    s: &Store,
    w: String,
    session: SessionId,
    b: ProviderBinding,
) -> Result<Vec<UsageRecord>> {
    s.call(move|c|{
    records::binding(c,session,&w,&b)?;let mut q=c.prepare("SELECT a.id,a.state,a.raw_usage,a.metadata FROM model_attempts a JOIN turns t ON t.id=a.turn_id WHERE t.session_id=?1 ORDER BY a.id")?;
    let rows=q.query_map([session.0],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))?;
    rows.map(|r|{let(a,state,u,m)=r?;Ok(UsageRecord{attempt:a,state,raw_usage:u.map(|v|serde_json::from_str(&v)).transpose()?,metadata:m.map(|v|serde_json::from_str(&v)).transpose()?})}).collect()
}).await
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
