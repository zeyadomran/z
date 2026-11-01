use harness_core::{Config, Engine, Error, ScriptFrame, ScriptStep, ScriptedProvider};
use tokio::sync::oneshot;

#[tokio::test]
async fn schema_mismatch_never_rewrites_existing_data() {
    for foreign in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let config = Config::new(workspace, root.path().join("data"));
        let mut engine = Engine::open(config.clone(), ScriptedProvider::new(vec![]))
            .await
            .unwrap();
        let session = engine.create_session().await.unwrap();
        engine.shutdown().await.unwrap();
        let path = config.data_dir.join("harness.sqlite");
        let connection = rusqlite::Connection::open(&path).unwrap();
        if foreign {
            connection.pragma_update(None, "application_id", 0).unwrap();
            connection.pragma_update(None, "user_version", 0).unwrap();
        } else {
            connection.pragma_update(None, "user_version", 999).unwrap();
        }
        connection.close().unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            Engine::open(config.clone(), ScriptedProvider::new(vec![])).await,
            Err(Error::Schema)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // Restore the fixture's version, then prove the public record remains readable.
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "application_id", 0x5a484152_i64)
            .unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        connection.close().unwrap();
        let mut reopened = Engine::open(config, ScriptedProvider::new(vec![]))
            .await
            .unwrap();
        assert!(reopened.history(session).await.unwrap().is_empty());
        reopened.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn workspace_exclusion_and_session_binding_survive_independent_connections() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let other_workspace = root.path().join("other");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&other_workspace).unwrap();
    let config = Config::new(workspace, root.path().join("data"));
    let (entered, entry) = oneshot::channel();
    let (_release, release) = oneshot::channel();
    let mut first = Engine::open(
        config.clone(),
        ScriptedProvider::new(vec![ScriptStep {
            expected_prompt: "wait".into(),
            frames: vec![
                ScriptFrame::Wait { entered, release },
                ScriptFrame::Complete,
            ],
        }]),
    )
    .await
    .unwrap();
    let first_session = first.create_session().await.unwrap();
    let mut turn = first.submit(first_session, "wait".into()).await.unwrap();
    entry.await.unwrap();
    let mut second = Engine::open(config.clone(), ScriptedProvider::new(vec![]))
        .await
        .unwrap();
    let second_session = second.create_session().await.unwrap();
    assert!(matches!(
        second.submit(second_session, "blocked".into()).await,
        Err(Error::Busy)
    ));
    assert!(second.history(second_session).await.unwrap().is_empty());
    let mut other = Engine::open(
        Config {
            workspace: other_workspace,
            ..config
        },
        ScriptedProvider::new(vec![]),
    )
    .await
    .unwrap();
    assert!(matches!(
        other.history(first_session).await,
        Err(Error::Binding)
    ));
    turn.cancel();
    turn.outcome().await.unwrap();
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    other.shutdown().await.unwrap();
}
