use crate::{journal, provider::*, records, store::Store, workspace::Workspace, *};
use std::sync::Arc;
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};
pub(crate) struct TurnRun {
    pub store: Store,
    pub provider: Arc<Mutex<Box<dyn Provider>>>,
    pub config: Config,
    pub session: SessionId,
    pub turn: TurnId,
    pub request: ProviderRequest,
    pub cancellation: watch::Receiver<bool>,
    pub cursor: watch::Sender<EventSequence>,
    pub progress: watch::Sender<Progress>,
}
impl TurnRun {
    pub async fn run(mut self) -> Result<TurnOutcome> {
        let budget = journal::budget(&self.store, self.turn).await?;
        let started = Instant::now();
        let mut attempts = budget.attempts;
        let mut tools = budget.tools;
        let mut output = budget.output;
        let elapsed = || {
            budget
                .active_ms
                .saturating_add(started.elapsed().as_millis().min(i64::MAX as u128) as i64)
        };
        let deadline = tokio::time::sleep(
            self.config
                .turn_timeout
                .saturating_sub(std::time::Duration::from_millis(budget.active_ms as u64)),
        );
        tokio::pin!(deadline);
        let caps = self.provider.lock().await.capabilities();
        let mut retries = 0;
        let mut final_text = None;
        let reason = loop {
            if *self.cancellation.borrow() || self.cursor.is_closed() {
                break StopReason::Cancelled;
            }
            if attempts >= self.config.max_model_attempts {
                break StopReason::RequestLimit;
            }
            if output >= self.config.max_output_bytes {
                break StopReason::OutputLimit;
            }
            if !context::fits(&self.request, &self.config, &caps)? {
                break StopReason::ContextLimit;
            }
            let attempt = journal::attempt(&self.store, self.session, self.turn, elapsed()).await?;
            attempts += 1;
            self.progress.send_replace(Progress::default());
            let mut response = tokio::select! {biased;
                _=cancelled(self.cancellation.clone())=>ProviderResponse::stopped(StopReason::Cancelled),
                _=self.cursor.closed()=>ProviderResponse::stopped(StopReason::Cancelled),
                _=&mut deadline=>ProviderResponse::stopped(StopReason::Deadline),
                r=async{self.provider.lock().await.respond(self.request.clone(),self.config.max_output_bytes-output,self.progress.clone()).await}=>r.unwrap_or_else(|_|ProviderResponse::stopped(StopReason::Protocol)),
            };
            if let Some(message) = &response.message {
                let mut proposed = self.request.clone();
                proposed
                    .messages
                    .push(ProtocolMessage::AssistantData(message.clone()));
                for call in &message.calls {
                    proposed.messages.push(ProtocolMessage::ToolResult {
                        call_id: call.id.clone(),
                        result: String::new(),
                    });
                }
                let valid = message.calls.len() <= 60
                    && message.calls.iter().all(|c| {
                        c.id.len() <= 256 && c.name.len() <= 128 && c.arguments.len() <= 256 * 1024
                    })
                    && ScriptedProvider::new(vec![])
                        .validate_request(&proposed)
                        .is_ok()
                    && ((response.reason == StopReason::Final && message.calls.is_empty())
                        || (response.reason == StopReason::Tools && !message.calls.is_empty()));
                if !valid {
                    response = ProviderResponse::stopped(StopReason::Protocol);
                }
            }
            let progress = self.progress.borrow().clone();
            output = output.saturating_add(progress.bytes);
            // Do not cancel persistence futures: accepted responses/results are either
            // durably recorded or surfaced as persistence failures before further work.
            let mut saved_response = journal::response(
                &self.store,
                self.session,
                self.turn,
                attempt,
                response.clone(),
                progress.clone(),
                elapsed(),
            )
            .await;
            if matches!(saved_response, Err(Error::Busy)) {
                saved_response = journal::response(
                    &self.store,
                    self.session,
                    self.turn,
                    attempt,
                    response.clone(),
                    progress,
                    elapsed(),
                )
                .await;
            }
            let batch = saved_response?;
            let Some(message) = response.message else {
                if matches!(
                    response.reason,
                    StopReason::Throttled
                        | StopReason::Unavailable
                        | StopReason::RetryableTransport
                ) && retries < 2
                {
                    retries += 1;
                    let session = self.session;
                    let turn = self.turn;
                    self.store
                        .call(move |c| {
                            let tx = c.transaction()?;
                            records::event(
                                &tx,
                                session,
                                turn,
                                EventKind::AttemptDiscarded { turn_id: turn },
                            )?;
                            tx.commit()?;
                            Ok(())
                        })
                        .await?;
                    tokio::select! {biased;
                        _=cancelled(self.cancellation.clone())=>break StopReason::Cancelled,
                        _=self.cursor.closed()=>break StopReason::Cancelled,
                        _=&mut deadline=>break StopReason::Deadline,
                        _=tokio::time::sleep(response.retry_after.unwrap_or(std::time::Duration::from_secs(1<<retries)))=>{},
                    }
                    continue;
                }
                break response.reason;
            };
            retries = 0;
            if response.reason == StopReason::Final {
                final_text = Some(message.text);
                break StopReason::Final;
            }
            let batch = batch.ok_or_else(|| Error::Protocol("missing durable batch".into()))?;
            let mut stop = None;
            for call in message.calls {
                if *self.cancellation.borrow() || self.cursor.is_closed() {
                    stop = Some(StopReason::Cancelled);
                }
                if elapsed() >= self.config.turn_timeout.as_millis() as i64 {
                    stop = Some(StopReason::Deadline);
                }
                if tools >= self.config.max_tool_calls && stop.is_none() {
                    stop = Some(StopReason::ToolLimit);
                }
                if output >= self.config.max_output_bytes && stop.is_none() {
                    stop = Some(StopReason::OutputLimit);
                }
                let mut result = if let Some(reason) = &stop {
                    serde_json::json!({"error":"skipped before inspection","reason":reason})
                        .to_string()
                } else {
                    journal::intent(&self.store, batch, call.id.clone(), self.turn).await?;
                    tools += 1;
                    let workspace = Workspace::new(&self.config);
                    let name = call.name;
                    let args = call.arguments;
                    // Read-only blocking work is bounded and joined even on cancellation.
                    tokio::task::spawn_blocking(move || workspace.execute(&name, &args).to_string())
                        .await
                        .map_err(|_| Error::Worker)?
                };
                if result.len() > self.config.max_output_bytes.saturating_sub(output) {
                    stop = Some(StopReason::OutputLimit);
                    result=serde_json::json!({"error":"inspection result exceeds remaining output budget"}).to_string();
                }
                output = output.saturating_add(result.len());
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
            if let Some(reason) = stop {
                break reason;
            }
        };
        let state = match reason {
            StopReason::Final => TurnState::Completed,
            StopReason::Cancelled => TurnState::Cancelled,
            StopReason::Refused => TurnState::Refused,
            StopReason::Deadline
            | StopReason::OutputLimit
            | StopReason::ContextLimit
            | StopReason::RequestLimit
            | StopReason::ToolLimit => TurnState::SuspendedLimit,
            _ => TurnState::Failed,
        };
        let outcome = TurnOutcome {
            turn_id: self.turn,
            state,
            reason,
            text: final_text,
        };
        let mut saved =
            records::finish(&self.store, self.session, outcome.clone(), elapsed()).await;
        if matches!(saved, Err(Error::Busy)) {
            saved = records::finish(&self.store, self.session, outcome.clone(), elapsed()).await;
        }
        self.cursor.send_replace(saved?);
        Ok(outcome)
    }
}
pub(crate) async fn cancelled(mut signal: watch::Receiver<bool>) {
    loop {
        if *signal.borrow_and_update() {
            return;
        }
        if signal.changed().await.is_err() {
            return;
        }
    }
}
