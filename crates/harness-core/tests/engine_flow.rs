use harness_core::*;
use serde_json::json;
#[cfg(windows)]
#[path = "support/patch_acl.rs"]
mod native_acl;
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
    #[cfg(windows)]
    let original_acl = native_acl::acl(&workspace.join("answer.txt"), true);
    #[cfg(windows)]
    let original_attributes = {
        use std::os::windows::fs::MetadataExt;
        std::fs::metadata(workspace.join("answer.txt"))
            .unwrap()
            .file_attributes()
    };
    let mut config = Config::new(workspace.clone(), root.path().join("data"));
    config.policy = Policy::Supervised;
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
        ToolCall {
            id: "patch-3".into(),
            name: "apply_patch".into(),
            arguments: json!({"path":"answer.txt","expected_sha256":"6bc986c4a9e68fb55c134289596a694812fe3a304643960531ff863eb7536b1b","old_text":"42","new_text":"43"}).to_string(),
        },
        ToolCall {
            id: "create-4".into(), name: "apply_patch".into(),
            arguments: json!({"path":"fresh.txt","expected_sha256":"must_not_exist","old_text":"","new_text":""}).to_string(),
        },
        ToolCall {
            id: "lf-edit-5".into(), name: "apply_patch".into(),
            arguments: json!({"path":"fresh.txt","expected_sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855","old_text":"","new_text":"updated\n"}).to_string(),
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
    assert_eq!(outcome.state, TurnState::AwaitingApproval);
    assert_eq!(outcome.reason, StopReason::ApprovalRequired);
    assert_eq!(
        std::fs::read(workspace.join("answer.txt")).unwrap(),
        b"\xef\xbb\xbfthe answer is 42\r\n"
    );
    let preview = engine.preview_patch(session, turn.id).await.unwrap();
    assert_eq!(preview.call_id, "patch-3");
    assert_eq!(preview.arguments["old_text"], "42");
    let mut approved = engine
        .decide_patch(&preview.approval_id, ApprovalDecision::Approve)
        .await
        .unwrap();
    assert_eq!(
        approved.outcome().await.unwrap().state,
        TurnState::AwaitingApproval
    );
    for call in ["create-4", "lf-edit-5"] {
        let p = engine.preview_patch(session, turn.id).await.unwrap();
        assert_eq!(p.call_id, call);
        approved = engine
            .decide_patch(&p.approval_id, ApprovalDecision::Approve)
            .await
            .unwrap();
        let result = approved.outcome().await.unwrap();
        let expected = if call == "create-4" {
            TurnState::AwaitingApproval
        } else {
            TurnState::Completed
        };
        if result.state != expected {
            // Keep native-runner failures actionable. All evidence belongs to
            // this small owned fixture; no user workspace or credential data.
            let database =
                rusqlite::Connection::open(config.data_dir.join("harness.sqlite")).unwrap();
            let mut query = database
                .prepare("SELECT state,evidence,intent FROM patch_attempts ORDER BY id")
                .unwrap();
            let evidence = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            eprintln!("patch {call}: {result:?}; recorded attempts: {evidence:?}");
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                eprintln!(
                    "fresh.txt actual ACL: {}; attributes: {}; bytes: {:?}",
                    native_acl::acl(&workspace.join("fresh.txt"), false),
                    std::fs::metadata(workspace.join("fresh.txt"))
                        .unwrap()
                        .file_attributes(),
                    std::fs::read(workspace.join("fresh.txt")).unwrap()
                );
            }
        }
        assert_eq!(result.state, expected);
    }
    assert_eq!(
        std::fs::read(workspace.join("fresh.txt")).unwrap(),
        b"updated\n"
    );
    let outcome = approved.outcome().await.unwrap();
    assert_eq!(
        std::fs::read(workspace.join("answer.txt")).unwrap(),
        b"\xef\xbb\xbfthe answer is 43\r\n"
    );
    assert_eq!(outcome.state, TurnState::Completed);
    assert_eq!(outcome.text.as_deref(), Some("The answer is 42."));
    #[cfg(windows)]
    assert_eq!(
        native_acl::acl(&workspace.join("answer.txt"), false),
        original_acl
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(workspace.join("answer.txt"))
                .unwrap()
                .file_attributes(),
            original_attributes
        );
    }
    let request = engine.protocol_history(session).await.unwrap();
    assert_eq!(request.messages.len(), 8);
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
    assert_eq!(
        tool_order,
        vec!["read-1", "search-2", "patch-3", "create-4", "lf-edit-5"]
    );
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
    assert_eq!(approved.latest_sequence(), events.last().unwrap().sequence);
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
