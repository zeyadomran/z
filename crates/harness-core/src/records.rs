use crate::{
    provider::{Epoch, ProtocolMessage, ProviderRequest},
    store::Store,
    *,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

pub(crate) const PREFIX: &str = "M0 deterministic harness. No tools are available.";

fn binding(connection: &Connection, session: SessionId, workspace: &str) -> Result<()> {
    let saved: Option<(String, String, String, String, String)> = connection
        .query_row(
            "SELECT workspace,provider,account_ref,model,options FROM sessions WHERE id=?1",
            [session.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    match saved {
        None => Err(Error::NotFound),
        Some((w, p, a, m, o))
            if w == workspace
                && p == "scripted"
                && a == "local"
                && m == "script-v1"
                && o == "{}" =>
        {
            Ok(())
        }
        _ => Err(Error::Binding),
    }
}

pub(crate) async fn create_session(store: &Store, workspace: String) -> Result<SessionId> {
    store.call(move |connection| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO sessions(workspace,provider,account_ref,model,options) VALUES(?1,'scripted','local','script-v1','{}')", [&workspace])?;
        let session = SessionId(tx.last_insert_rowid());
        tx.execute("INSERT INTO epochs(session_id,state,prefix,tool_schema) VALUES(?1,'active',?2,'[]')", params![session.0, PREFIX])?;
        tx.commit()?;
        Ok(session)
    }).await
}

pub(crate) async fn start(
    store: &Store,
    workspace: String,
    session: SessionId,
    prompt: String,
    limits: String,
) -> Result<(TurnId, ProviderRequest, EventSequence)> {
    store.call(move |connection| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        binding(&tx, session, &workspace)?;
        let unresolved: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE workspace=?1 AND state IN ('running','awaiting_approval','suspended_limit','suspended_recovery','interrupted'))", [&workspace], |r| r.get(0))?;
        if unresolved { return Err(Error::Busy); }
        let epoch = tx.query_row("SELECT id,prefix,tool_schema FROM epochs WHERE session_id=?1 AND state='active'", [session.0], |r| Ok(Epoch {
            id: ContextEpochId(r.get(0)?), prefix: r.get(1)?, tool_schema: r.get(2)?,
        }))?;
        tx.execute("INSERT INTO turns(session_id,workspace,epoch_id,state,limits) VALUES(?1,?2,?3,'running',?4)", params![session.0,workspace,epoch.id.0,limits])?;
        let turn = TurnId(tx.last_insert_rowid());
        tx.execute("INSERT INTO messages(turn_id,epoch_id,role,content) VALUES(?1,?2,'user',?3)", params![turn.0,epoch.id.0,prompt])?;
        tx.execute("INSERT INTO model_attempts(turn_id,state) VALUES(?1,'running')", [turn.0])?;
        let sequence = event(&tx, session, turn, EventKind::TurnStarted { turn_id: turn })?;
        let messages = history_rows(&tx, session)?.into_iter().map(|message| match message.role {
            Role::User => ProtocolMessage::User(message.text),
            Role::Assistant => ProtocolMessage::Assistant { text: message.text, calls: vec![] },
        }).collect();
        tx.commit()?;
        Ok((turn, ProviderRequest { epoch, messages }, sequence))
    }).await
}

pub(crate) async fn finish(
    store: &Store,
    session: SessionId,
    outcome: TurnOutcome,
    active_ms: i64,
) -> Result<EventSequence> {
    store.call(move |connection| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let turn = outcome.turn_id;
        let state = serde_json::to_value(outcome.state)?.as_str().ok_or(Error::Worker)?.to_owned();
        let changed = tx.execute("UPDATE turns SET state=?1,outcome=?2,active_ms=?3 WHERE id=?4 AND session_id=?5 AND state='running'",
            params![state,serde_json::to_string(&outcome)?,active_ms,turn.0,session.0])?;
        if changed != 1 { return Err(Error::Persistence("turn transition conflict".into())); }
        let attempt_state = match outcome.state {
            TurnState::Completed => "completed", TurnState::Cancelled => "cancelled",
            TurnState::Refused => "refused", TurnState::SuspendedLimit => "incomplete", _ => "failed",
        };
        tx.execute("UPDATE model_attempts SET state=?1,stop_reason=?2 WHERE turn_id=?3 AND state='running'",
            params![attempt_state,serde_json::to_string(&outcome.reason)?,turn.0])?;
        if let Some(text) = &outcome.text {
            tx.execute("INSERT INTO messages(turn_id,epoch_id,role,content) SELECT id,epoch_id,'assistant',?1 FROM turns WHERE id=?2", params![text,turn.0])?;
            event(&tx, session, turn, EventKind::AssistantCompleted { turn_id: turn })?;
        }
        let sequence = event(&tx, session, turn, EventKind::TurnStopped { outcome })?;
        tx.commit()?;
        Ok(sequence)
    }).await
}

fn event(
    tx: &Transaction<'_>,
    session: SessionId,
    turn: TurnId,
    event: EventKind,
) -> Result<EventSequence> {
    let sequence: i64 = tx.query_row(
        "UPDATE sessions SET next_sequence=next_sequence+1 WHERE id=?1 RETURNING next_sequence-1",
        [session.0],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO events VALUES(?1,?2,?3,?4)",
        params![session.0, sequence, turn.0, serde_json::to_string(&event)?],
    )?;
    Ok(EventSequence(sequence))
}

fn history_rows(connection: &Connection, session: SessionId) -> Result<Vec<Message>> {
    let mut query = connection.prepare("SELECT m.role,m.content FROM messages m JOIN turns t ON t.id=m.turn_id WHERE t.session_id=?1 ORDER BY m.id")?;
    let rows = query.query_map([session.0], |row| {
        let role: String = row.get(0)?;
        Ok(Message {
            role: if role == "user" {
                Role::User
            } else {
                Role::Assistant
            },
            text: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub(crate) async fn history(
    store: &Store,
    workspace: String,
    session: SessionId,
) -> Result<Vec<Message>> {
    store
        .call(move |connection| {
            binding(connection, session, &workspace)?;
            history_rows(connection, session)
        })
        .await
}

pub(crate) async fn events(
    store: &Store,
    workspace: String,
    session: SessionId,
    after: EventSequence,
) -> Result<Vec<DurableEvent>> {
    store.call(move |connection| {
        binding(connection, session, &workspace)?;
        let mut query = connection.prepare("SELECT sequence,payload FROM events WHERE session_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 256")?;
        let rows = query.query_map(params![session.0,after.0], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?;
        rows.map(|row| { let (sequence,payload) = row?; Ok(DurableEvent { sequence: EventSequence(sequence), event: serde_json::from_str(&payload)? }) }).collect()
    }).await
}

pub(crate) async fn outcome(
    store: &Store,
    workspace: String,
    session: SessionId,
    turn: TurnId,
) -> Result<Option<TurnOutcome>> {
    store
        .call(move |connection| {
            binding(connection, session, &workspace)?;
            let value: Option<Option<String>> = connection
                .query_row(
                    "SELECT outcome FROM turns WHERE id=?1 AND session_id=?2",
                    params![turn.0, session.0],
                    |r| r.get(0),
                )
                .optional()?;
            value
                .ok_or(Error::NotFound)?
                .map(|json| serde_json::from_str(&json).map_err(Error::from))
                .transpose()
        })
        .await
}
