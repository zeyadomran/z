use harness_core::*;
use serde_json::json;
#[tokio::test]
async fn completed_turn_and_history_survive_reopen() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(
        workspace.join("answer.txt"),
        b"\xef\xbb\xbfthe answer is 42\r\n",
    )
    .unwrap();
    let config = Config::new(workspace.clone(), root.path().join("data"));
    let calls = vec![
        ToolCall {
            id: "read-1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"answer.txt"}"#.into(),
        },
        ToolCall {
            id: "search-2".into(),
            name: "search_text".into(),
            arguments: r#"{"query":"42"}"#.into(),
        },
    ];
    let opaque =
        json!({"reasoning_details":[{"type":"reasoning.encrypted","data":"opaque-signature"}]});
    let mut frames = calls
        .iter()
        .map(|c| ScriptFrame::ToolCall(serde_json::to_string(c).unwrap()))
        .collect::<Vec<_>>();
    frames.push(ScriptFrame::Opaque(opaque.clone()));
    frames.push(ScriptFrame::Complete);
    let script = ScriptedProvider::new(vec![
        ScriptStep {
            expected_prompt: "inspect".into(),
            frames,
        },
        ScriptStep::reply("inspect", "The answer is 42."),
    ]);
    let mut engine = Engine::open(config.clone(), script).await.unwrap();
    let session = engine.create_session().await.unwrap();
    let mut turn = engine.submit(session, "inspect".into()).await.unwrap();
    let outcome = turn.outcome().await.unwrap();
    assert_eq!(outcome.state, TurnState::Completed);
    assert_eq!(outcome.text.as_deref(), Some("The answer is 42."));
    let request = engine.protocol_history(session).await.unwrap();
    assert_eq!(request.messages.len(), 5);
    assert_eq!(
        request.messages[1],
        ProtocolMessage::AssistantData(AssistantMessage {
            text: String::new(),
            calls,
            opaque
        })
    );
    let ProtocolMessage::ToolResult { call_id, result } = &request.messages[2] else {
        panic!("read result missing")
    };
    assert_eq!(call_id, "read-1");
    let read: serde_json::Value = serde_json::from_str(result).unwrap();
    assert_eq!(read["data"]["text"], "the answer is 42");
    assert_eq!(
        read["data"]["sha256"],
        "6bc986c4a9e68fb55c134289596a694812fe3a304643960531ff863eb7536b1b"
    );
    assert_eq!(read["data"]["bom"], true);
    assert_eq!(read["data"]["newline"], "crlf");
    let ProtocolMessage::ToolResult { call_id, result } = &request.messages[3] else {
        panic!("search result missing")
    };
    assert_eq!(call_id, "search-2");
    let search: serde_json::Value = serde_json::from_str(result).unwrap();
    assert_eq!(
        search["data"]["matches"],
        json!([{"path":"answer.txt","line":1,"text":"the answer is 42"}])
    );
    let events = engine.events(session, EventSequence(0)).await.unwrap();
    let tool_order = events
        .iter()
        .filter_map(|e| {
            if let EventKind::ToolFinished { call_id, .. } = &e.event {
                Some(call_id.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_order, vec!["read-1", "search-2"]);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, EventKind::BatchClosed { .. }))
    );
    assert_eq!(
        events.last().unwrap().event,
        EventKind::TurnStopped {
            outcome: outcome.clone()
        }
    );
    assert_eq!(turn.latest_sequence(), events.last().unwrap().sequence);
    assert_eq!(engine.usage(session).await.unwrap().len(), 2);
    let history = engine.history(session).await.unwrap();
    engine.shutdown().await.unwrap();
    // Changing a previously inspected file cannot rerun closed calls during resume.
    std::fs::write(workspace.join("answer.txt"), "changed after checkpoint").unwrap();
    let mut expected = request.messages.clone();
    expected.push(ProtocolMessage::User("again".into()));
    let mut reopened = Engine::open(
        config,
        ScriptedProvider::new(vec![ScriptStep {
            expected_prompt: "again".into(),
            frames: vec![
                ScriptFrame::ExpectHistory(expected),
                ScriptFrame::Text("Still here.".into()),
                ScriptFrame::Complete,
            ],
        }]),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.protocol_history(session).await.unwrap().messages,
        request.messages
    );
    assert_eq!(reopened.history(session).await.unwrap(), history);
    assert_eq!(
        reopened.turn_outcome(session, turn.id).await.unwrap(),
        Some(outcome)
    );
    assert_eq!(
        reopened.events(session, EventSequence(0)).await.unwrap(),
        events
    );
    let mut next = reopened.submit(session, "again".into()).await.unwrap();
    assert_eq!(
        next.outcome().await.unwrap().text.as_deref(),
        Some("Still here.")
    );
    assert_eq!(
        reopened.protocol_history(session).await.unwrap().epoch,
        request.epoch
    );
    reopened.shutdown().await.unwrap();
}
