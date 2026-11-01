use harness_core::*;
use std::io::{BufRead, BufReader};
#[test]
fn crash_fixture_child() {
    let Some(root) = std::env::var_os("HARNESS_OWNED_CRASH_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
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
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tokio::time::resume();
        // Kill only after the public durable cursor confirms active accounting.
        loop {
            turn.changed().await.unwrap();
            if e.events(session, EventSequence(0)).await.unwrap().iter().any(|event|
                matches!(event.event, EventKind::ActiveTimeCheckpoint { active_ms, .. } if active_ms >= 5000)
            ) { break; }
        }
        println!("READY {} {}", session.0, turn.id.0);
        turn.outcome().await.unwrap();
    });
}
#[tokio::test]
async fn process_lock_crosses_data_dirs_and_orphan_requires_explicit_resume() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let mut alternate = Engine::open(
        Config::new(root.path().join("workspace"), root.path().join("alternate")),
        ScriptedProvider::new(vec![]),
    )
    .await
    .unwrap();
    let alt = alternate.create_session().await.unwrap();
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
    assert!(matches!(
        alternate.submit(alt, "blocked".into()).await,
        Err(Error::Busy)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    // Released OS lock still cannot bypass an unresolved turn in the other store.
    let error = alternate
        .submit(alt, "still blocked".into())
        .await
        .err()
        .unwrap();
    let message = error.to_string();
    assert!(
        message.contains(&format!("--session {}", ids[0])),
        "{message}"
    );
    assert!(
        message.contains(&format!("--resume-turn {}", ids[1])),
        "{message}"
    );
    assert!(message.contains("harness abandon"), "{message}");
    let printed_path = message
        .split("--data-dir \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(printed_path).unwrap(),
        std::fs::canonicalize(root.path().join("data")).unwrap()
    );
    let Error::UnresolvedWork(work) = error else {
        panic!("expected actionable recovery")
    };
    assert_eq!(work.session_id, SessionId(ids[0]));
    assert_eq!(work.turn_id, TurnId(ids[1]));
    assert_eq!(
        std::fs::canonicalize(work.data_dir).unwrap(),
        std::fs::canonicalize(root.path().join("data")).unwrap()
    );
    let database = root.path().join("data/harness.sqlite");
    let moved = root.path().join("data/owned-missing.sqlite");
    std::fs::rename(&database, &moved).unwrap();
    let missing_error = alternate
        .submit(alt, "missing unresolved store".into())
        .await
        .err()
        .unwrap();
    assert!(missing_error.to_string().contains("original --data-dir"));
    let missing_message = missing_error.to_string();
    let printed_path = missing_message
        .split("--data-dir \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(printed_path).unwrap(),
        std::fs::canonicalize(root.path().join("data")).unwrap()
    );
    assert!(
        matches!(missing_error, Error::RecoveryStoreUnavailable { data_dir } if data_dir.ends_with("data"))
    );
    let missing = Engine::open(
        Config::new(root.path().join("workspace"), root.path().join("data")),
        ScriptedProvider::new(vec![]),
    )
    .await;
    let refused = matches!(missing, Err(Error::RecoveryStoreUnavailable { .. }));
    if let Ok(mut accidentally_created) = missing {
        accidentally_created.shutdown().await.unwrap();
        std::fs::remove_file(&database).unwrap();
    }
    std::fs::rename(&moved, &database).unwrap();
    assert!(
        refused,
        "opening the original path must not recreate a missing unresolved store"
    );
    // Control the wall-clock boundary: the last attempt started hours ago.
    // Offline recovery wait must not consume the active-work budget.
    let c = rusqlite::Connection::open(&database).unwrap();
    c.execute(
        "UPDATE model_attempts SET started_unix_ms=started_unix_ms-7200000",
        [],
    )
    .unwrap();
    c.close().unwrap();
    alternate.shutdown().await.unwrap();
    let mut resumed_config = Config::new(root.path().join("workspace"), root.path().join("data"));
    resumed_config.max_output_bytes = 512 * 1024; // Unknown crashed-stream output is conservatively reserved.
    resumed_config.turn_timeout = std::time::Duration::from_secs(5);
    let mut reopened = Engine::open(
        resumed_config.clone(),
        ScriptedProvider::new(vec![ScriptStep::reply("wait", "resumed")]),
    )
    .await
    .unwrap();
    let session = SessionId(ids[0]);
    let turn = TurnId(ids[1]);
    let bypass = reopened.create_session().await.unwrap();
    assert!(matches!(
        reopened.submit(bypass, "blocked".into()).await,
        Err(Error::UnresolvedWork(work)) if work.session_id == session && work.turn_id == turn
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
        resumed.outcome().await.unwrap().reason,
        StopReason::Deadline
    );
    assert_eq!(reopened.usage(session).await.unwrap().len(), 1);
    assert!(
        reopened
            .events(session, EventSequence(0))
            .await
            .unwrap()
            .iter()
            .any(|event| matches!(
                event.event,
                EventKind::ActiveTimeRecovered {
                    uncertain_ms: 1000,
                    ..
                }
            ))
    );
    reopened.shutdown().await.unwrap();
    resumed_config.turn_timeout = std::time::Duration::from_secs(1200);
    let mut reopened = Engine::open(
        resumed_config,
        ScriptedProvider::new(vec![ScriptStep::reply("wait", "resumed")]),
    )
    .await
    .unwrap();
    let mut resumed = reopened.resume(session, turn).await.unwrap();
    assert_eq!(
        resumed.outcome().await.unwrap().text.as_deref(),
        Some("resumed")
    );
    assert_eq!(reopened.usage(session).await.unwrap().len(), 2);
    reopened.shutdown().await.unwrap();
}
