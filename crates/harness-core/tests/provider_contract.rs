use harness_core::*;

#[test]
fn scripted_protocol_requires_closed_pairing_and_frozen_epochs() {
    let mut provider = ScriptedProvider::new(vec![]);
    let mut request = ProviderRequest {
        epoch: Epoch {
            id: ContextEpochId(1),
            prefix: "fixed".into(),
            tool_schema: "[{\"name\":\"read_file\"}]".into(),
        },
        messages: vec![
            ProtocolMessage::User("inspect".into()),
            ProtocolMessage::Assistant {
                text: "".into(),
                calls: vec!["a".into(), "b".into()],
            },
        ],
    };
    assert!(provider.validate_request(&request).is_err());
    request.messages.push(ProtocolMessage::ToolResult {
        call_id: "a".into(),
        result: "ok".into(),
    });
    assert!(provider.validate_request(&request).is_err());
    let mut wrong = request.clone();
    wrong
        .messages
        .push(ProtocolMessage::User("interleaved".into()));
    assert!(provider.validate_request(&wrong).is_err());
    for call_id in ["a", "foreign"] {
        let mut wrong = request.clone();
        wrong.messages.push(ProtocolMessage::ToolResult {
            call_id: call_id.into(),
            result: "ok".into(),
        });
        assert!(provider.validate_request(&wrong).is_err());
    }
    request.messages.push(ProtocolMessage::ToolResult {
        call_id: "b".into(),
        result: "denied".into(),
    });
    provider.validate_request(&request).unwrap();
    for change_prefix in [true, false] {
        let mut changed = request.clone();
        if change_prefix {
            changed.epoch.prefix = "changed".into();
        } else {
            changed.epoch.tool_schema = "[]".into();
        }
        assert!(provider.validate_request(&changed).is_err());
    }
    provider.validate_request(&request).unwrap();
}

#[tokio::test]
async fn invalid_or_executable_responses_never_become_completed_messages() {
    let cases = [
        (
            vec![ScriptFrame::Text("partial".into())],
            StopReason::Protocol,
            TurnState::Failed,
        ),
        (
            vec![ScriptFrame::Complete, ScriptFrame::Text("too late".into())],
            StopReason::Protocol,
            TurnState::Failed,
        ),
        (
            vec![ScriptFrame::Text("partial".into()), ScriptFrame::Disconnect],
            StopReason::Transport,
            TurnState::Failed,
        ),
        (
            vec![ScriptFrame::ToolCall("run_command".into())],
            StopReason::ToolsUnavailable,
            TurnState::Failed,
        ),
        (
            vec![ScriptFrame::Refused],
            StopReason::Refused,
            TurnState::Refused,
        ),
    ];
    for (frames, reason, state) in cases {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut engine = Engine::open(
            Config::new(workspace.clone(), root.path().join("data")),
            ScriptedProvider::new(vec![ScriptStep {
                expected_prompt: "hello".into(),
                frames,
            }]),
        )
        .await
        .unwrap();
        let session = engine.create_session().await.unwrap();
        let mut turn = engine.submit(session, "hello".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        assert_eq!(outcome.state, state);
        assert_eq!(outcome.reason, reason);
        assert_eq!(outcome.text, None);
        assert_eq!(
            engine.history(session).await.unwrap(),
            vec![Message {
                role: Role::User,
                text: "hello".into()
            }]
        );
        assert_eq!(std::fs::read_dir(workspace).unwrap().count(), 0);
        engine.shutdown().await.unwrap();
    }
}
