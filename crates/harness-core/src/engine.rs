use crate::{
    journal, records,
    store::{Store, StoreWorker},
    turn_loop::TurnRun,
    workspace_lock::WorkspaceLock,
    *,
};
use std::sync::Arc;
use tokio::{
    sync::{Mutex, oneshot, watch},
    task::JoinHandle,
};
pub struct Engine {
    config: Config,
    workspace: String,
    binding: ProviderBinding,
    store: Option<Store>,
    worker: Option<StoreWorker>,
    provider: Arc<Mutex<Box<dyn Provider>>>,
    active: Option<ActiveTurn>,
}
struct ActiveTurn {
    cancel: watch::Sender<bool>,
    outcome: watch::Receiver<Option<Result<TurnOutcome>>>,
    task: JoinHandle<Result<()>>,
}
pub struct TurnHandle {
    pub id: TurnId,
    cancel: watch::Sender<bool>,
    cursor: watch::Receiver<EventSequence>,
    outcome: watch::Receiver<Option<Result<TurnOutcome>>>,
    progress: watch::Receiver<Progress>,
}
impl TurnHandle {
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }
    pub fn latest_sequence(&self) -> EventSequence {
        *self.cursor.borrow()
    }
    pub async fn changed(&mut self) -> Result<EventSequence> {
        self.cursor.changed().await.map_err(|_| Error::Shutdown)?;
        Ok(*self.cursor.borrow_and_update())
    }
    /// Coalesced bounded presentation; no consumer acknowledgment gates execution.
    pub fn progress_receiver(&self) -> watch::Receiver<Progress> {
        self.progress.clone()
    }
    pub async fn outcome(&mut self) -> Result<TurnOutcome> {
        loop {
            if let Some(o) = self.outcome.borrow().clone() {
                return o;
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
    pub async fn open(config: Config, provider: impl Provider) -> Result<Self> {
        let config = tokio::task::spawn_blocking(move || config.validate())
            .await
            .map_err(|_| Error::Worker)??;
        let workspace = config.workspace_identity()?;
        let binding = provider.binding();
        let (store, worker) = Store::open(config.data_dir.join("harness.sqlite")).await?;
        Ok(Self {
            config,
            workspace,
            binding,
            store: Some(store),
            worker: Some(worker),
            provider: Arc::new(Mutex::new(Box::new(provider))),
            active: None,
        })
    }
    fn store(&self) -> Result<&Store> {
        self.store.as_ref().ok_or(Error::Shutdown)
    }
    pub async fn create_session(&mut self) -> Result<SessionId> {
        let config = self.config.clone();
        let (prefix, instructions) = tokio::task::spawn_blocking(move || context::prefix(&config))
            .await
            .map_err(|_| Error::Worker)??;
        records::create_session(
            self.store()?,
            self.workspace.clone(),
            self.binding.clone(),
            prefix,
            instructions,
        )
        .await
    }
    pub async fn submit(&mut self, session: SessionId, prompt: String) -> Result<TurnHandle> {
        if prompt.trim().is_empty() || prompt.len() > self.config.max_prompt_bytes {
            return Err(Error::Configuration(
                "prompt is empty or exceeds its byte limit".into(),
            ));
        }
        self.launch(session, Some(prompt), None).await
    }
    /// Explicit user action resumes consumed budgets; raising Config limits is a
    /// host decision. Completed read results are never replayed.
    pub async fn resume(&mut self, session: SessionId, turn: TurnId) -> Result<TurnHandle> {
        self.launch(session, None, Some(turn)).await
    }
    async fn launch(
        &mut self,
        session: SessionId,
        prompt: Option<String>,
        resume: Option<TurnId>,
    ) -> Result<TurnHandle> {
        self.store()?;
        if let Some(a) = &self.active
            && !a.task.is_finished()
            && a.outcome.borrow().is_none()
        {
            return Err(Error::Busy);
        }
        if let Some(a) = self.active.take() {
            a.task.await.map_err(|_| Error::Worker)??;
        }
        let (cancel, cancellation) = watch::channel(false);
        let (cursor, cursor_rx) = watch::channel(EventSequence(0));
        let (result, result_rx) = watch::channel(None);
        let (progress, progress_rx) = watch::channel(Progress::default());
        let (accepted, accepted_rx) = oneshot::channel();
        let store = self.store()?.clone();
        let provider = self.provider.clone();
        let config = self.config.clone();
        let workspace = self.workspace.clone();
        let binding = self.binding.clone();
        let task = tokio::spawn(async move {
            let prepare = async {
                let lock = WorkspaceLock::acquire(&config, &workspace).await?;
                journal::reconcile(&store, workspace.clone()).await?;
                let (turn, request, seq) = if let Some(turn) = resume {
                    let r = journal::resume(
                        &store,
                        workspace,
                        session,
                        turn,
                        binding,
                        config.limits_json()?,
                    )
                    .await?;
                    (turn, r, EventSequence(0))
                } else {
                    records::start(
                        &store,
                        workspace,
                        session,
                        prompt.ok_or(Error::Worker)?,
                        config.limits_json()?,
                        binding,
                    )
                    .await?
                };
                Ok::<_, Error>((lock, turn, request, seq))
            }
            .await;
            let (lock, turn, request, seq) = match prepare {
                Ok(v) => v,
                Err(e) => {
                    let _ = accepted.send(Err(e));
                    return Ok(());
                }
            };
            cursor.send_replace(seq);
            let _ = accepted.send(Ok(turn));
            let outcome = TurnRun {
                store,
                provider,
                config,
                session,
                turn,
                request,
                cancellation,
                cursor,
                progress,
            }
            .run()
            .await;
            // Release OS ownership only after all durable work has settled.
            drop(lock);
            result.send_replace(Some(outcome.clone()));
            outcome.map(|_| ())
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
            progress: progress_rx,
        })
    }
    pub async fn history(&self, session: SessionId) -> Result<Vec<Message>> {
        records::history(
            self.store()?,
            self.workspace.clone(),
            session,
            self.binding.clone(),
        )
        .await
    }
    pub async fn events(
        &self,
        session: SessionId,
        after: EventSequence,
    ) -> Result<Vec<DurableEvent>> {
        records::events(
            self.store()?,
            self.workspace.clone(),
            session,
            after,
            self.binding.clone(),
        )
        .await
    }
    pub async fn turn_outcome(
        &self,
        session: SessionId,
        turn: TurnId,
    ) -> Result<Option<TurnOutcome>> {
        records::outcome(
            self.store()?,
            self.workspace.clone(),
            session,
            turn,
            self.binding.clone(),
        )
        .await
    }
    pub async fn usage(&self, session: SessionId) -> Result<Vec<journal::UsageRecord>> {
        journal::usage(
            self.store()?,
            self.workspace.clone(),
            session,
            self.binding.clone(),
        )
        .await
    }
    pub async fn protocol_history(&self, session: SessionId) -> Result<ProviderRequest> {
        let w = self.workspace.clone();
        let b = self.binding.clone();
        self.store()?
            .call(move |c| {
                records::binding(c, session, &w, &b)?;
                records::request(c, session)
            })
            .await
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        let mut result = Ok(());
        if let Some(a) = self.active.take() {
            a.cancel.send_replace(true);
            result = a.task.await.map_err(|_| Error::Worker).and_then(|r| r);
        }
        self.store.take();
        if let Some(w) = self.worker.take() {
            tokio::task::spawn_blocking(move || w.0.join())
                .await
                .map_err(|_| Error::Worker)?
                .map_err(|_| Error::Worker)?;
        }
        result
    }
    pub async fn wait_idle(&mut self) -> Result<()> {
        if let Some(a) = self.active.as_mut() {
            let r = (&mut a.task).await.map_err(|_| Error::Worker)?;
            self.active.take();
            r?;
        }
        Ok(())
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(a) = &self.active {
            a.cancel.send_replace(true);
        }
    }
}
