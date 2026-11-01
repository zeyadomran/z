use crate::{journal, provider::*, records, store::Store, *};
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
    pub authorization: crate::pending::PendingAuthorization,
}
impl TurnRun {
    pub async fn run(self) -> Result<TurnOutcome> {
        let budget = journal::budget(&self.store, self.turn).await?;
        let started = Instant::now();
        let previous_ms = budget.active_ms;
        let store = self.store.clone();
        let turn = self.turn;
        let session = self.session;
        let cursor = self.cursor.clone();
        let interval = std::time::Duration::from_millis(journal::ACTIVE_CHECKPOINT_MS as u64);
        let mut checkpoint = tokio::time::interval_at(started + interval, interval);
        checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let (halt, checkpoint_failure) = watch::channel(false);
        let work = self.run_active(budget, started, checkpoint_failure);
        tokio::pin!(work);
        loop {
            tokio::select! { biased;
                result = &mut work => return result,
                _ = checkpoint.tick() => {
                    let ms = previous_ms.saturating_add(started.elapsed().as_millis().min(i64::MAX as u128) as i64);
                    match journal::active_checkpoint(&store, session, turn, ms).await {
                        Ok(Some(sequence)) => { cursor.send_replace(sequence); },
                        Ok(None) => {},
                        Err(error) => {
                        // Settle accepted writes and join any bounded read before
                        // releasing OS ownership, even if the checkpoint failed.
                        halt.send_replace(true);
                        let _ = work.await;
                        return Err(error);
                        }
                    }
                }
            }
        }
    }
    async fn run_active(
        mut self,
        budget: journal::Budget,
        started: Instant,
        checkpoint_failure: watch::Receiver<bool>,
    ) -> Result<TurnOutcome> {
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
        let mut discarded_attempts = 0;
        let mut final_text = None;
        let reason = loop {
            if let Some(reason) = self
                .pending(&mut tools, &mut output, &elapsed, &checkpoint_failure)
                .await?
            {
                break reason;
            }

            if *self.cancellation.borrow()
                || *checkpoint_failure.borrow()
                || self.cursor.is_closed()
            {
                break StopReason::Cancelled;
            }
            if elapsed() >= self.config.turn_timeout.as_millis() as i64 {
                break StopReason::Deadline;
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
            self.progress.send_replace(Progress {
                attempt: attempts,
                discarded_attempts,
                ..Progress::default()
            });
            let mut response = tokio::select! {biased;
                _=cancelled(self.cancellation.clone())=>ProviderResponse::stopped(StopReason::Cancelled),
                _=cancelled(checkpoint_failure.clone())=>ProviderResponse::stopped(StopReason::Cancelled),
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
                    discarded_attempts += 1;
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
                        _=cancelled(checkpoint_failure.clone())=>break StopReason::Cancelled,
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
            batch.ok_or_else(|| Error::Protocol("missing durable batch".into()))?;
        };
        let state = match reason {
            StopReason::ApprovalRequired => TurnState::AwaitingApproval,
            StopReason::RecoveryRequired => TurnState::SuspendedRecovery,
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
