use harness_core::{
    Config, Engine, ScriptFrame, ScriptStep, ScriptedProvider, StopReason, TurnState,
};
use tokio::sync::oneshot;

// Each branch uses an explicit provider-entry handshake, never a scheduler sleep.
#[tokio::test]
async fn cancellation_does_not_depend_on_observer_consumption() {
    for trigger in ["cancel", "observer_loss", "shutdown", "engine_drop"] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let config = Config::new(workspace, root.path().join("data"));
        let (entered, entry) = oneshot::channel();
        let (_release, release) = oneshot::channel();
        let provider = ScriptedProvider::new(vec![ScriptStep {
            expected_prompt: "wait".into(),
            frames: vec![
                ScriptFrame::Wait { entered, release },
                ScriptFrame::Complete,
            ],
        }]);
        let mut engine = Engine::open(config.clone(), provider).await.unwrap();
        let session = engine.create_session().await.unwrap();
        let mut handle = engine.submit(session, "wait".into()).await.unwrap();
        entry.await.unwrap();
        let id = handle.id;
        // Leave cursor unread throughout. Outcome delivery has its own retained channel.
        match trigger {
            "cancel" => {
                handle.cancel();
                assert_eq!(handle.outcome().await.unwrap().state, TurnState::Cancelled);
                engine.shutdown().await.unwrap();
            }
            "observer_loss" => {
                drop(handle);
                tokio::time::timeout(std::time::Duration::from_secs(10), engine.wait_idle())
                    .await
                    .expect("observer loss must cancel without shutdown")
                    .unwrap();
                engine.shutdown().await.unwrap();
            }
            "shutdown" => {
                engine.shutdown().await.unwrap();
                assert_eq!(handle.outcome().await.unwrap().state, TurnState::Cancelled);
            }
            "engine_drop" => {
                drop(engine);
                assert_eq!(handle.outcome().await.unwrap().state, TurnState::Cancelled);
            }
            _ => unreachable!(),
        }
        let mut reopened = Engine::open(config, ScriptedProvider::new(vec![]))
            .await
            .unwrap();
        let outcome = reopened.turn_outcome(session, id).await.unwrap().unwrap();
        assert_eq!(outcome.state, TurnState::Cancelled);
        assert_eq!(outcome.reason, StopReason::Cancelled);
        assert_eq!(reopened.history(session).await.unwrap().len(), 1);
        reopened.shutdown().await.unwrap();
    }
}
