use harness_core::*;
use rusqlite::Connection;
#[tokio::test]
async fn failed_intent_or_result_commit_stops_work_and_reconciles_closed_history() {
    for state in ["intent", "resolved"] {
        let root = tempfile::tempdir().unwrap();
        let w = root.path().join("workspace");
        std::fs::create_dir(&w).unwrap();
        std::fs::write(w.join("file.txt"), "original").unwrap();
        let config = Config::new(w.clone(), root.path().join("data"));
        let call = ToolCall {
            id: "read".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"file.txt"}"#.into(),
        };
        let mut engine = Engine::open(
            config.clone(),
            ScriptedProvider::new(vec![ScriptStep {
                expected_prompt: "inspect".into(),
                frames: vec![
                    ScriptFrame::ToolCall(
                        serde_json::to_string(&ToolCall {
                            id: "kept".into(),
                            ..call.clone()
                        })
                        .unwrap(),
                    ),
                    ScriptFrame::ToolCall(serde_json::to_string(&call).unwrap()),
                    ScriptFrame::ToolCall(
                        serde_json::to_string(&ToolCall {
                            id: "unstarted".into(),
                            ..call.clone()
                        })
                        .unwrap(),
                    ),
                    ScriptFrame::Complete,
                ],
            }]),
        )
        .await
        .unwrap();
        let session = engine.create_session().await.unwrap();
        // A real SQLite trigger aborts exactly the selected write transaction.
        // This establishes commit failure, not disk-full or power-loss behavior.
        let c = Connection::open(config.data_dir.join("harness.sqlite")).unwrap();
        c.execute_batch(&format!("CREATE TRIGGER owned_failure BEFORE UPDATE ON tool_calls WHEN NEW.state='{state}' AND NEW.provider_id='read' BEGIN SELECT RAISE(ABORT,'owned persistence failure'); END;")).unwrap();
        let mut turn = engine.submit(session, "inspect".into()).await.unwrap();
        let error = turn.outcome().await.unwrap_err();
        assert!(
            matches!(&error,Error::Persistence(message) if message.contains("owned persistence failure"))
        );
        assert_eq!(engine.usage(session).await.unwrap().len(), 1);
        assert_eq!(
            engine
                .protocol_history(session)
                .await
                .unwrap()
                .messages
                .len(),
            2
        );
        assert!(engine.shutdown().await.is_err());
        c.execute_batch("DROP TRIGGER owned_failure").unwrap();
        c.close().unwrap();
        std::fs::write(w.join("file.txt"), "changed; must not be reread").unwrap();
        let mut reopened = Engine::open(
            config.clone(),
            ScriptedProvider::new(vec![ScriptStep::reply(
                "inspect",
                "Recovered without replay.",
            )]),
        )
        .await
        .unwrap();
        if state == "intent" {
            let mut resumed = reopened.resume(session, turn.id).await.unwrap();
            assert_eq!(resumed.outcome().await.unwrap().state, TurnState::Completed);
        } else {
            let c = Connection::open(config.data_dir.join("harness.sqlite")).unwrap();
            c.execute_batch("CREATE TRIGGER owned_cancel_failure BEFORE UPDATE ON turns WHEN NEW.state='cancelled' BEGIN SELECT RAISE(ABORT,'owned cancellation failure'); END;").unwrap();
            assert!(matches!(
                reopened.abandon(session, turn.id).await,
                Err(Error::Persistence(_))
            ));
            assert_eq!(
                reopened
                    .protocol_history(session)
                    .await
                    .unwrap()
                    .messages
                    .len(),
                2
            );
            assert_eq!(reopened.turn_outcome(session, turn.id).await.unwrap(), None);
            c.execute_batch("DROP TRIGGER owned_cancel_failure")
                .unwrap();
            c.close().unwrap();
            assert_eq!(
                reopened.abandon(session, turn.id).await.unwrap().state,
                TurnState::Cancelled
            );
            assert_eq!(reopened.usage(session).await.unwrap().len(), 1);
        }
        let history = reopened.protocol_history(session).await.unwrap();
        ScriptedProvider::new(vec![])
            .validate_request(&history)
            .unwrap();
        let ProtocolMessage::ToolResult { result: kept, .. } = &history.messages[2] else {
            panic!("completed result missing")
        };
        assert!(kept.contains("original"));
        let ProtocolMessage::ToolResult { result, .. } = &history.messages[3] else {
            panic!("batch not closed")
        };
        assert!(result.contains(if state == "intent" {
            "interrupted inspection"
        } else {
            "abandoned inspection"
        }));
        assert!(!result.contains("changed;"));
        if state == "resolved" {
            let ProtocolMessage::ToolResult { result, .. } = &history.messages[4] else {
                panic!("unstarted result missing")
            };
            assert!(result.contains("cancelled before inspection"));
        }
        reopened.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn authoritative_final_response_is_recovered_without_replacement_request() {
    let root = tempfile::tempdir().unwrap();
    let w = root.path().join("workspace");
    std::fs::create_dir(&w).unwrap();
    let config = Config::new(w, root.path().join("data"));
    let mut engine = Engine::open(
        config.clone(),
        ScriptedProvider::new(vec![ScriptStep::reply("first", "authoritative answer")]),
    )
    .await
    .unwrap();
    let session = engine.create_session().await.unwrap();
    let c = Connection::open(config.data_dir.join("harness.sqlite")).unwrap();
    c.execute_batch("CREATE TRIGGER owned_failure BEFORE UPDATE ON turns WHEN NEW.state='completed' BEGIN SELECT RAISE(ABORT,'owned final commit failure'); END;").unwrap();
    let mut turn = engine.submit(session, "first".into()).await.unwrap();
    assert!(matches!(turn.outcome().await, Err(Error::Persistence(_))));
    assert!(engine.shutdown().await.is_err());
    c.execute_batch("DROP TRIGGER owned_failure").unwrap();
    c.close().unwrap();
    let mut reopened = Engine::open(
        config,
        ScriptedProvider::new(vec![ScriptStep::reply("second", "next answer")]),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.abandon(session, turn.id).await.unwrap().state,
        TurnState::Completed
    );
    let mut next = reopened.submit(session, "second".into()).await.unwrap();
    assert_eq!(next.outcome().await.unwrap().state, TurnState::Completed);
    let recovered = reopened
        .turn_outcome(session, turn.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.state, TurnState::Completed);
    assert_eq!(recovered.text.as_deref(), Some("authoritative answer"));
    assert_eq!(reopened.usage(session).await.unwrap().len(), 2);
    reopened.shutdown().await.unwrap();
}
