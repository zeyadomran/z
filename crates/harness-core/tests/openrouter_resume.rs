use harness_core::{openrouter::*, *};
use serde_json::json;
#[path = "support/wire.rs"]
mod wire;
use wire::*;
fn config() -> OpenRouterConfig {
    OpenRouterConfig::new("fixture/model".into(), "fixture/spot".into())
}

#[tokio::test]
async fn explicit_output_token_increase_resumes_saved_turn() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let host = Config::new(workspace, root.path().join("data"));
    let mut ids = None;
    for cap in [1024, 2048] {
        let body = if cap == 1024 {
            event(json!({}), json!("length"))
        } else {
            event(json!({"content":"continued"}), json!("stop"))
        } + "data: [DONE]\n\n";
        let (base, server) = serve(vec![Wire::sse(body)]);
        let mut selected = config();
        selected.max_output_tokens = cap;
        let provider = OpenRouterProvider::loopback_fixture(base, selected)
            .await
            .unwrap();
        let mut engine = Engine::open(host.clone(), provider).await.unwrap();
        let (session, mut turn) = if let Some((session, turn)) = ids {
            (session, engine.resume(session, turn).await.unwrap())
        } else {
            let session = engine.create_session().await.unwrap();
            (
                session,
                engine.submit(session, "inspect".into()).await.unwrap(),
            )
        };
        let outcome = turn.outcome().await.unwrap();
        if cap == 1024 {
            assert_eq!(outcome.state, TurnState::SuspendedLimit);
            assert_eq!(outcome.reason, StopReason::OutputLimit);
            ids = Some((session, turn.id));
        } else {
            assert_eq!(outcome.text.as_deref(), Some("continued"));
            assert_eq!(engine.usage(session).await.unwrap().len(), 2);
            let caps: Vec<_> = engine
                .usage(session)
                .await
                .unwrap()
                .into_iter()
                .map(|u| u.metadata.unwrap().output_token_limit.unwrap())
                .collect();
            assert_eq!(caps, vec![1024, 2048]);
        }
        engine.shutdown().await.unwrap();
        let requests = tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
        assert_eq!(requests[0]["max_tokens"], cap);
    }
}
#[tokio::test]
async fn credential_replacement_cannot_read_or_continue_the_previous_session() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let host = Config::new(workspace, root.path().join("data"));
    let (base, server) = serve(vec![Wire::sse(
        event(json!({"content":"saved"}), json!("stop")) + "data: [DONE]\n\n",
    )]);
    let provider = OpenRouterProvider::loopback_fixture(base, config())
        .await
        .unwrap();
    let mut engine = Engine::open(host.clone(), provider).await.unwrap();
    let session = engine.create_session().await.unwrap();
    assert_eq!(
        engine
            .submit(session, "inspect".into())
            .await
            .unwrap()
            .outcome()
            .await
            .unwrap()
            .reason,
        StopReason::Final
    );
    let history = engine.history(session).await.unwrap();
    engine.shutdown().await.unwrap();
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
    for revision in [1, 0] {
        let (base, server) = serve(vec![]);
        let provider = OpenRouterProvider::loopback_fixture_key_revision(base, config(), revision)
            .await
            .unwrap();
        let mut engine = Engine::open(host.clone(), provider).await.unwrap();
        if revision == 1 {
            assert!(matches!(engine.history(session).await, Err(Error::Binding)));
            assert!(matches!(
                engine.submit(session, "blocked".into()).await,
                Err(Error::Binding)
            ));
        } else {
            assert_eq!(engine.history(session).await.unwrap(), history);
        }
        engine.shutdown().await.unwrap();
        assert!(
            tokio::task::spawn_blocking(move || server.join().unwrap())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
