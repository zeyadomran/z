use crate::{
    provider::*,
    records::{append, event},
    store::Store,
    *,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
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
pub(crate) async fn attempt(s: &Store, session: SessionId, t: TurnId, ms: i64) -> Result<i64> {
    s.call(move |c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE turns SET active_ms=?1 WHERE id=?2",
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
    tx.execute("UPDATE turns SET active_ms=?1,output_bytes=output_bytes+?2 WHERE id=?3",params![ms,p.bytes as i64,t.0])?;
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
    if tx.execute("UPDATE tool_calls SET state='resolved',result=?1 WHERE batch_id=?2 AND provider_id=?3 AND state!='resolved'",params![result,batch,id])?!=1{return Err(Error::Persistence("tool result conflict".into()));}
    tx.execute("UPDATE turns SET output_bytes=output_bytes+?1,active_ms=?2 WHERE id=?3",params![result.len() as i64,ms,t.0])?;
    event(&tx,session,t,EventKind::ToolFinished{turn_id:t,call_id:id})?;tx.commit()?;Ok(())
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
/// Called only while holding the workspace OS lock. M1a has no mutating tools:
/// unfinished inspections are truthfully skipped, completed results are reused.
pub(crate) async fn reconcile(s: &Store, w: String) -> Result<()> {
    s.call(move|c|{
    let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let turns={let mut q=tx.prepare("SELECT id,session_id FROM turns WHERE workspace=?1 AND state='running'")?;q.query_map([w],|r|Ok((TurnId(r.get(0)?),SessionId(r.get(1)?))))?.collect::<std::result::Result<Vec<_>,_>>()?};
    for(t,session) in turns {
        let lost_start:Option<i64>=tx.query_row("SELECT MIN(started_unix_ms) FROM model_attempts WHERE turn_id=?1 AND state='running'",[t.0],|r|r.get(0))?;
        if let Some(start)=lost_start {
            let now=unix_ms();
            let lost=if now>=start {(now-start).min(86_400_000)} else {180_000};
            tx.execute("UPDATE turns SET active_ms=active_ms+?1,output_bytes=MAX(output_bytes,COALESCE(json_extract(limits,'$.max_output_bytes'),0)) WHERE id=?2",params![lost,t.0])?;
        }
        // A fully stored final response is authoritative even if the process
        // died before the terminal transition. Never request its replacement.
        let final_payload:Option<String>=tx.query_row("SELECT payload FROM protocol_messages WHERE turn_id=?1 ORDER BY id DESC LIMIT 1",[t.0],|r|r.get(0)).optional()?;
        if let Some(payload)=final_payload && let ProtocolMessage::AssistantData(m)=serde_json::from_str(&payload)? && m.calls.is_empty() {
            let outcome=TurnOutcome{turn_id:t,state:TurnState::Completed,reason:StopReason::Final,text:Some(m.text)};
            tx.execute("UPDATE turns SET state='completed',outcome=?1 WHERE id=?2",params![serde_json::to_string(&outcome)?,t.0])?;
            event(&tx,session,t,EventKind::TurnStopped{outcome})?;
            continue;
        }
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
    if tx.execute("UPDATE turns SET state='running',outcome=NULL,limits=?1 WHERE id=?2 AND session_id=?3 AND state IN ('interrupted','suspended_limit')",params![limits,t.0,session.0])?!=1{return Err(Error::Busy);}
    let r=records::request(&tx,session)?;tx.commit()?;Ok(r)
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
