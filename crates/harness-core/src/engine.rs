use crate::{
    records,
    store::{Store, StoreWorker},
    *,
};
use std::sync::Arc;
use tokio::{
    sync::{Mutex, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};

pub struct Engine {
    config: Config,
    workspace: String,
    store: Option<Store>,
    worker: Option<StoreWorker>,
    provider: Arc<Mutex<ScriptedProvider>>,
    active: Option<ActiveTurn>,
}

struct ActiveTurn {
    cancel: watch::Sender<bool>,
    outcome: watch::Receiver<Option<Result<TurnOutcome>>>,
    task: JoinHandle<Result<()>>,
}

/// Retained completion and coalescing durable-event cursor, independent of cancellation.
pub struct TurnHandle {
    pub id: TurnId,
    cancel: watch::Sender<bool>,
    cursor: watch::Receiver<EventSequence>,
    outcome: watch::Receiver<Option<Result<TurnOutcome>>>,
}

impl TurnHandle {
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }
    pub fn latest_sequence(&self) -> EventSequence {
        *self.cursor.borrow()
    }

    /// A cursor is a notification, never an execution acknowledgment.
    pub async fn changed(&mut self) -> Result<EventSequence> {
        self.cursor.changed().await.map_err(|_| Error::Shutdown)?;
        Ok(*self.cursor.borrow_and_update())
    }

    pub async fn outcome(&mut self) -> Result<TurnOutcome> {
        loop {
            if let Some(outcome) = self.outcome.borrow().clone() {
                return outcome;
            }
            self.outcome.changed().await.map_err(|_| Error::Worker)?;
        }
    }
}

impl Drop for TurnHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Engine {
    pub async fn open(config: Config, provider: ScriptedProvider) -> Result<Self> {
        let config = tokio::task::spawn_blocking(move || config.validate())
            .await
            .map_err(|_| Error::Worker)??;
        let workspace = config.workspace_identity()?;
        let (store, worker) = Store::open(config.data_dir.join("harness.sqlite")).await?;
        Ok(Self {
            config,
            workspace,
            store: Some(store),
            worker: Some(worker),
            provider: Arc::new(Mutex::new(provider)),
            active: None,
        })
    }

    fn store(&self) -> Result<&Store> {
        self.store.as_ref().ok_or(Error::Shutdown)
    }

    pub async fn create_session(&mut self) -> Result<SessionId> {
        records::create_session(self.store()?, self.workspace.clone()).await
    }

    /// Accepted prompt persistence precedes the returned handle. Drop the handle to cancel.
    pub async fn submit(&mut self, session: SessionId, prompt: String) -> Result<TurnHandle> {
        self.store()?;
        if prompt.trim().is_empty() || prompt.len() > self.config.max_prompt_bytes {
            return Err(Error::Configuration(
                "prompt is empty or exceeds its byte limit".into(),
            ));
        }
        if let Some(active) = &self.active
            && !active.task.is_finished()
            && active.outcome.borrow().is_none()
        {
            return Err(Error::Busy);
        }
        if let Some(active) = self.active.take() {
            active.task.await.map_err(|_| Error::Worker)??;
        }
        let (cancel, cancellation) = watch::channel(false);
        let (cursor, cursor_rx) = watch::channel(EventSequence(0));
        let (result, result_rx) = watch::channel(None);
        let (accepted, accepted_rx) = oneshot::channel();
        let store = self.store()?.clone();
        let provider = self.provider.clone();
        let config = self.config.clone();
        let workspace = self.workspace.clone();
        let limits = config.limits_json()?;
        // Ownership is installed before awaiting acceptance. Cancelling this submit
        // future drops the observer; the coordinator still owns and closes the turn.
        let task = tokio::spawn(async move {
            let (turn, request, sequence) =
                match records::start(&store, workspace, session, prompt, limits).await {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = accepted.send(Err(error));
                        return Ok(());
                    }
                };
            cursor.send_replace(sequence);
            let _ = accepted.send(Ok(turn));
            let started = Instant::now();
            let response = tokio::select! {
                biased;
                _ = cancelled(cancellation) => provider::Response { reason: StopReason::Cancelled, text: None },
                _ = cursor.closed() => provider::Response { reason: StopReason::Cancelled, text: None },
                _ = tokio::time::sleep(config.turn_timeout) => provider::Response { reason: StopReason::Deadline, text: None },
                response = async { provider.lock().await.respond(request, config.max_output_bytes).await } =>
                    response.unwrap_or(provider::Response { reason: StopReason::Protocol, text: None }),
            };
            let state = match response.reason {
                StopReason::Final => TurnState::Completed,
                StopReason::Cancelled => TurnState::Cancelled,
                StopReason::Deadline | StopReason::OutputLimit => TurnState::SuspendedLimit,
                StopReason::Refused => TurnState::Refused,
                _ => TurnState::Failed,
            };
            let outcome = TurnOutcome {
                turn_id: turn,
                state,
                reason: response.reason,
                text: response.text,
            };
            let elapsed = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            // Bounded persistence retry reuses the same result, never the provider.
            let mut saved = records::finish(&store, session, outcome.clone(), elapsed).await;
            if matches!(saved, Err(Error::Busy)) {
                saved = records::finish(&store, session, outcome.clone(), elapsed).await;
            }
            match saved {
                Ok(sequence) => {
                    cursor.send_replace(sequence);
                    result.send_replace(Some(Ok(outcome)));
                    Ok(())
                }
                Err(error) => {
                    result.send_replace(Some(Err(error.clone())));
                    Err(error)
                }
            }
        });
        self.active = Some(ActiveTurn {
            cancel: cancel.clone(),
            outcome: result_rx.clone(),
            task,
        });
        let id = accepted_rx.await.map_err(|_| Error::Worker)??;
        Ok(TurnHandle {
            id,
            cancel,
            cursor: cursor_rx,
            outcome: result_rx,
        })
    }

    pub async fn history(&self, session: SessionId) -> Result<Vec<Message>> {
        records::history(self.store()?, self.workspace.clone(), session).await
    }

    /// At most 256 events; continue from the returned last sequence to drain a backlog.
    pub async fn events(
        &self,
        session: SessionId,
        after: EventSequence,
    ) -> Result<Vec<DurableEvent>> {
        records::events(self.store()?, self.workspace.clone(), session, after).await
    }

    pub async fn turn_outcome(
        &self,
        session: SessionId,
        turn: TurnId,
    ) -> Result<Option<TurnOutcome>> {
        records::outcome(self.store()?, self.workspace.clone(), session, turn).await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        let mut result = Ok(());
        if let Some(active) = self.active.take() {
            active.cancel.send_replace(true);
            result = active.task.await.map_err(|_| Error::Worker).and_then(|r| r);
        }
        self.store.take();
        if let Some(worker) = self.worker.take() {
            tokio::task::spawn_blocking(move || worker.0.join())
                .await
                .map_err(|_| Error::Worker)?
                .map_err(|_| Error::Worker)?;
        }
        result
    }

    /// Join current work without requesting cancellation. Observation is optional.
    pub async fn wait_idle(&mut self) -> Result<()> {
        if let Some(active) = self.active.as_mut() {
            let result = (&mut active.task).await.map_err(|_| Error::Worker)?;
            self.active.take();
            result?;
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.send_replace(true);
        }
    }
}

async fn cancelled(mut signal: watch::Receiver<bool>) {
    loop {
        if *signal.borrow_and_update() {
            return;
        }
        if signal.changed().await.is_err() {
            return;
        }
    }
}
