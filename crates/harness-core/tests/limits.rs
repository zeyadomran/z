use harness_core::*;
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn configured_limits_suspend_without_persisting_partial_response() {
    for deadline in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut config = Config::new(workspace, root.path().join("data"));
        config.max_output_bytes = 3;
        let (entered, entry) = oneshot::channel();
        let (_release, release) = oneshot::channel();
        let frames = if deadline {
            vec![
                ScriptFrame::Wait { entered, release },
                ScriptFrame::Complete,
            ]
        } else {
            vec![ScriptFrame::Text("four".into()), ScriptFrame::Complete]
        };
        let mut engine = Engine::open(
            config.clone(),
            ScriptedProvider::new(vec![ScriptStep {
                expected_prompt: "hello".into(),
                frames,
            }]),
        )
        .await
        .unwrap();
        let session = engine.create_session().await.unwrap();
        let mut turn = engine.submit(session, "hello".into()).await.unwrap();
        if deadline {
            entry.await.unwrap();
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(1201)).await;
        }
        let outcome = turn.outcome().await.unwrap();
        if deadline {
            tokio::time::resume();
        }
        assert_eq!(outcome.state, TurnState::SuspendedLimit);
        assert_eq!(
            outcome.reason,
            if deadline {
                StopReason::Deadline
            } else {
                StopReason::OutputLimit
            }
        );
        assert_eq!(outcome.text, None);
        engine.shutdown().await.unwrap();
        let mut reopened = Engine::open(config, ScriptedProvider::new(vec![]))
            .await
            .unwrap();
        assert_eq!(
            reopened.turn_outcome(session, turn.id).await.unwrap(),
            Some(outcome)
        );
        let new_session = reopened.create_session().await.unwrap();
        assert!(matches!(
            reopened
                .submit(new_session, "cannot bypass suspension".into())
                .await,
            Err(Error::Busy)
        ));
        reopened.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_configuration_and_prompt_are_rejected_before_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let config = Config::new(workspace.clone(), root.path().join("data"));
    let cases = [
        Config {
            max_prompt_bytes: 0,
            ..config.clone()
        },
        Config {
            max_output_bytes: 0,
            ..config.clone()
        },
        Config {
            turn_timeout: Duration::ZERO,
            ..config.clone()
        },
        Config {
            data_dir: workspace.join("data"),
            ..config.clone()
        },
    ];
    for invalid in cases {
        assert!(matches!(
            Engine::open(invalid, ScriptedProvider::new(vec![])).await,
            Err(Error::Configuration(_))
        ));
    }
    assert!(!config.data_dir.exists());
    assert!(!workspace.join("data").exists());
    let mut engine = Engine::open(config, ScriptedProvider::new(vec![]))
        .await
        .unwrap();
    let session = engine.create_session().await.unwrap();
    assert!(matches!(
        engine.submit(session, " ".into()).await,
        Err(Error::Configuration(_))
    ));
    assert!(engine.history(session).await.unwrap().is_empty());
    engine.shutdown().await.unwrap();
    assert!(matches!(
        engine.create_session().await,
        Err(Error::Shutdown)
    ));
    engine.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_tool_and_context_caps_preserve_closed_batches_across_resume() {
    for kind in ["requests", "tools", "context"] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("a.txt"), "saved result").unwrap();
        let mut config = Config::new(workspace.clone(), root.path().join("data"));
        match kind {
            "requests" => config.max_model_attempts = 1,
            "tools" => config.max_tool_calls = 1,
            _ => config.context_tokens = 4096,
        }
        let calls = [
            ToolCall {
                id: "a".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a.txt"}"#.into(),
            },
            ToolCall {
                id: "b".into(),
                name: "list_files".into(),
                arguments: "{}".into(),
            },
        ];
        let mut frames = calls
            .iter()
            .map(|c| ScriptFrame::ToolCall(serde_json::to_string(c).unwrap()))
            .collect::<Vec<_>>();
        frames.push(ScriptFrame::Complete);
        let mut engine = Engine::open(
            config.clone(),
            ScriptedProvider::new(vec![ScriptStep {
                expected_prompt: "inspect".into(),
                frames,
            }]),
        )
        .await
        .unwrap();
        let session = engine.create_session().await.unwrap();
        let mut turn = engine.submit(session, "inspect".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        assert_eq!(outcome.state, TurnState::SuspendedLimit);
        assert_eq!(
            outcome.reason,
            match kind {
                "requests" => StopReason::RequestLimit,
                "tools" => StopReason::ToolLimit,
                _ => StopReason::ContextLimit,
            }
        );
        let old = engine.protocol_history(session).await.unwrap();
        ScriptedProvider::new(vec![])
            .validate_request(&old)
            .unwrap();
        assert_eq!(
            engine.usage(session).await.unwrap().len(),
            if kind == "context" { 0 } else { 1 }
        );
        engine.shutdown().await.unwrap();
        std::fs::write(workspace.join("a.txt"), "new bytes").unwrap();
        config.max_model_attempts = 30;
        config.max_tool_calls = 60;
        config.context_tokens = 128000;
        let mut reopened = Engine::open(
            config,
            ScriptedProvider::new(vec![ScriptStep {
                expected_prompt: "inspect".into(),
                frames: vec![
                    ScriptFrame::ExpectHistory(old.messages),
                    ScriptFrame::Text("resumed".into()),
                    ScriptFrame::Complete,
                ],
            }]),
        )
        .await
        .unwrap();
        let mut resumed = reopened.resume(session, turn.id).await.unwrap();
        assert_eq!(resumed.outcome().await.unwrap().state, TurnState::Completed);
        assert_eq!(
            reopened.usage(session).await.unwrap().len(),
            if kind == "context" { 1 } else { 2 }
        );
        reopened.shutdown().await.unwrap();
    }
}
