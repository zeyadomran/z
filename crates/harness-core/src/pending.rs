use crate::{
    patches::{self, Authorization},
    store::Store,
    turn_loop::TurnRun,
    workspace::{Workspace, patch::Reconciliation},
    *,
};
use rusqlite::OptionalExtension;
pub(crate) struct SavedCall {
    pub call: ToolCall,
    pub resolved: bool,
    pub result: Option<String>,
}
pub(crate) async fn batch(store: &Store, turn: TurnId) -> Result<Option<(i64, Vec<SavedCall>)>> {
    store.call(move |c| {
        let batch:Option<i64>=c.query_row("SELECT batch_id FROM tool_calls WHERE turn_id=?1 AND batch_id>COALESCE((SELECT closed_message_id FROM checkpoints WHERE turn_id=?1),0) ORDER BY batch_id LIMIT 1",[turn.0],|r|r.get(0)).optional()?;
        let Some(batch)=batch else { return Ok(None); };
        let mut q=c.prepare("SELECT provider_id,name,arguments,state,result FROM tool_calls WHERE batch_id=?1 ORDER BY ordinal")?;
        let calls=q.query_map([batch],|r|Ok(SavedCall {call:ToolCall{id:r.get(0)?,name:r.get(1)?,arguments:r.get(2)?},resolved:r.get::<_,String>(3)?=="resolved",result:r.get(4)?}))?.collect::<std::result::Result<Vec<_>,_>>()?;
        Ok(Some((batch,calls)))
    }).await
}
impl TurnRun {
    pub(crate) async fn pending(
        &mut self,
        tools: &mut usize,
        output: &mut usize,
        elapsed: &impl Fn() -> i64,
        checkpoint_failure: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<Option<StopReason>> {
        let Some((batch, calls)) = batch(&self.store, self.turn).await? else {
            return Ok(None);
        };
        // Validate the entire authoritative batch before even the first effect.
        // Invalid calls are resolved together, preserving protocol pairing.
        let schemas: serde_json::Value = serde_json::from_str(&self.request.epoch.tool_schema)?;
        let valid = calls.iter().all(|c| {
            schemas.as_array().is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool["function"]["name"] == c.call.name)
            }) && Workspace::validate_tool(&c.call.name, &c.call.arguments).is_ok()
        });
        let mut skip = (!valid).then_some("rejected: complete batch failed tool schema validation");
        let mut stop = None;
        for saved in calls {
            if saved.resolved {
                // A committed refusal to proceed still stops later calls after
                // interruption. Only engine-owned patch statuses carry this
                // decision; read content must not be interpreted as policy.
                if saved.call.name == "apply_patch" {
                    let result: serde_json::Value =
                        serde_json::from_str(saved.result.as_deref().ok_or(Error::Worker)?)?;
                    skip = match result["status"].as_str() {
                        Some("denied") => Some("denial skips unstarted calls"),
                        Some("stale") => Some("stale approval skips unstarted calls"),
                        Some("rejected") => Some("invalid patch skips unstarted calls"),
                        _ => skip,
                    };
                }
                continue;
            }
            let call = saved.call;
            if *self.cancellation.borrow()
                || *checkpoint_failure.borrow()
                || self.cursor.is_closed()
            {
                stop = Some(StopReason::Cancelled);
            }
            if elapsed() >= self.config.turn_timeout.as_millis() as i64 && stop.is_none() {
                stop = Some(StopReason::Deadline);
            }
            if *tools >= self.config.max_tool_calls && stop.is_none() {
                stop = Some(StopReason::ToolLimit);
            }
            if *output >= self.config.max_output_bytes && stop.is_none() {
                stop = Some(StopReason::OutputLimit);
            }
            // Recovery evidence takes precedence over cancellation/budgets:
            // an effect that already happened must never be reported skipped.
            let reconciliation = if call.name == "apply_patch" {
                patches::reconcile(&self.store, &self.config, batch, call.id.clone()).await?
            } else {
                None
            };
            if matches!(reconciliation, Some(Reconciliation::Unknown)) {
                return Ok(Some(StopReason::RecoveryRequired));
            }
            let mut result = if matches!(reconciliation, Some(Reconciliation::Applied)) {
                patches::reconciled_result(&self.store, self.turn, batch, call.id.clone()).await?
            } else if let Some(reason) = &stop {
                serde_json::json!({"status":"skipped","reason":reason}).to_string()
            } else if let Some(error) = skip {
                serde_json::json!({"status":"skipped","error":error}).to_string()
            } else if call.name == "apply_patch" {
                if self.config.policy == Policy::Inspect {
                    skip = Some("denial skips unstarted calls");
                    serde_json::json!({"status":"denied","error":"inspect mode denies patches"})
                        .to_string()
                } else if let Some(auth) = self.authorization.take() {
                    let p = &auth.preview;
                    if p.turn != self.turn
                        || p.session != self.session
                        || p.batch != batch
                        || p.call_id != call.id
                        || p.workspace != self.config.workspace_identity()?
                        || p.policy_version != patches::POLICY_VERSION
                        || serde_json::to_string(&p.prepared.args)?
                            != serde_json::to_string(&crate::workspace::patch::PatchArgs::parse(
                                &call.arguments,
                            )?)?
                    {
                        return Err(Error::Binding);
                    }
                    if auth.decision == ApprovalDecision::Deny {
                        skip = Some("denial skips unstarted calls");
                        serde_json::json!({"status":"denied","approval_id":p.approval_id})
                            .to_string()
                    } else {
                        // Reserve enough result budget before a write. Never
                        // replace evidence of completed effects with truncation.
                        if self.config.max_output_bytes.saturating_sub(*output)
                            < 2048 + serde_json::to_vec(&auth.preview.instructions)?.len()
                        {
                            return Ok(Some(StopReason::OutputLimit));
                        }
                        let value = patches::execute(&self.store, &self.config, auth, || {
                            if *self.cancellation.borrow()
                                || *checkpoint_failure.borrow()
                                || self.cursor.is_closed()
                            {
                                Some(StopReason::Cancelled)
                            } else if elapsed() >= self.config.turn_timeout.as_millis() as i64 {
                                Some(StopReason::Deadline)
                            } else {
                                None
                            }
                        })
                        .await?;
                        *tools += 1;
                        if value["status"] == "cancelled" {
                            stop = Some(serde_json::from_value(value["reason"].clone())?);
                        }
                        if value["status"] == "unknown" {
                            return Ok(Some(StopReason::RecoveryRequired));
                        }
                        if value["status"] == "stale" {
                            skip = Some("stale approval skips unstarted calls");
                        }
                        value.to_string()
                    }
                } else {
                    let w = Workspace::new(&self.config);
                    let args = call.arguments.clone();
                    let prepared = tokio::task::spawn_blocking(move || w.prepare_patch(&args))
                        .await
                        .map_err(|_| Error::Worker)?;
                    match prepared {
                        Ok(_) => return Ok(Some(StopReason::ApprovalRequired)),
                        Err(e) => {
                            skip = Some("invalid patch skips unstarted calls");
                            serde_json::json!({"status":"rejected","error":e.to_string()})
                                .to_string()
                        }
                    }
                }
            } else {
                journal::intent(&self.store, batch, call.id.clone(), self.turn).await?;
                *tools += 1;
                let w = Workspace::new(&self.config);
                let known = journal::instruction_keys(&self.store, self.turn).await?;
                let name = call.name.clone();
                let args = call.arguments.clone();
                tokio::task::spawn_blocking(move || w.execute(&name, &args, &known).to_string())
                    .await
                    .map_err(|_| Error::Worker)?
            };
            if call.name != "apply_patch"
                && result.len() > self.config.max_output_bytes.saturating_sub(*output)
            {
                stop = Some(StopReason::OutputLimit);
                result=serde_json::json!({"error":"inspection result exceeds remaining output budget"}).to_string();
            }
            *output = output.saturating_add(result.len());
            let mut saved = journal::tool_result(
                &self.store,
                self.session,
                self.turn,
                batch,
                call.id.clone(),
                result.clone(),
                elapsed(),
            )
            .await;
            if matches!(saved, Err(Error::Busy)) {
                saved = journal::tool_result(
                    &self.store,
                    self.session,
                    self.turn,
                    batch,
                    call.id,
                    result,
                    elapsed(),
                )
                .await;
            }
            saved?;
        }
        self.request = journal::close(&self.store, self.session, self.turn, batch).await?;
        Ok(stop)
    }
}
// Authorization is owned by the running turn, consumed once, and never loaded
// from durable audit records. The caller can only supply a current Engine preview.
pub(crate) type PendingAuthorization = Option<Authorization>;
