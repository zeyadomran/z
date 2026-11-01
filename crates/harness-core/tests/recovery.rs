use harness_core::*;
use std::io::{BufRead, BufReader};
#[test]
fn crash_fixture_child() {
    let Some(root) = std::env::var_os("HARNESS_OWNED_CRASH_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let (entered, entry) = tokio::sync::oneshot::channel();
        let (_release, release) = tokio::sync::oneshot::channel();
        let mut e = Engine::open(
            Config::new(root.join("workspace"), root.join("data")),
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
        let session = e.create_session().await.unwrap();
        let mut turn = e.submit(session, "wait".into()).await.unwrap();
        entry.await.unwrap();
        println!("READY {} {}", session.0, turn.id.0);
        turn.outcome().await.unwrap();
    });
}
#[tokio::test]
async fn process_lock_crosses_data_dirs_and_orphan_requires_explicit_resume() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_fixture_child", "--nocapture"])
        .env("HARNESS_OWNED_CRASH_FIXTURE", root.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let ids = tokio::task::spawn_blocking(move || {
        for line in BufReader::new(stdout).lines() {
            let line = line.unwrap();
            if let Some(ids) = line.strip_prefix("READY ") {
                return ids
                    .split_whitespace()
                    .map(|v| v.parse::<i64>().unwrap())
                    .collect::<Vec<_>>();
            }
        }
        panic!("fixture exited before provider entry")
    })
    .await
    .unwrap();
    let mut alternate = Engine::open(
        Config::new(root.path().join("workspace"), root.path().join("alternate")),
        ScriptedProvider::new(vec![]),
    )
    .await
    .unwrap();
    let alt = alternate.create_session().await.unwrap();
    assert!(matches!(
        alternate.submit(alt, "blocked".into()).await,
        Err(Error::Busy)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    // Released OS lock still cannot bypass an unresolved turn in the other store.
    assert!(matches!(
        alternate.submit(alt, "still blocked".into()).await,
        Err(Error::Busy)
    ));
    alternate.shutdown().await.unwrap();
    let mut resumed_config = Config::new(root.path().join("workspace"), root.path().join("data"));
    resumed_config.max_output_bytes = 512 * 1024; // Unknown crashed-stream output is conservatively reserved.
    let mut reopened = Engine::open(
        resumed_config,
        ScriptedProvider::new(vec![ScriptStep::reply("wait", "resumed")]),
    )
    .await
    .unwrap();
    let session = SessionId(ids[0]);
    let turn = TurnId(ids[1]);
    let bypass = reopened.create_session().await.unwrap();
    assert!(matches!(
        reopened.submit(bypass, "blocked".into()).await,
        Err(Error::Busy)
    ));
    assert_eq!(
        reopened
            .turn_outcome(session, turn)
            .await
            .unwrap()
            .unwrap()
            .state,
        TurnState::Interrupted
    );
    let mut resumed = reopened.resume(session, turn).await.unwrap();
    assert_eq!(
        resumed.outcome().await.unwrap().text.as_deref(),
        Some("resumed")
    );
    assert_eq!(reopened.usage(session).await.unwrap().len(), 2);
    reopened.shutdown().await.unwrap();
}
