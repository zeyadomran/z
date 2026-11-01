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
        let inspection = ToolCall {
            id: "inspection".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"nested/file.txt"}"#.into(),
        };
        let mut script = vec![];
        if state == TurnState::Refused {
            std::fs::create_dir(workspace.join("nested")).unwrap();
            std::fs::write(workspace.join("nested/file.txt"), "safe file").unwrap();
            std::fs::write(workspace.join("nested/AGENTS.md"), "nested instruction").unwrap();
            script.push(ScriptStep {
                expected_prompt: "hello".into(),
                frames: vec![
                    ScriptFrame::ToolCall(serde_json::to_string(&inspection).unwrap()),
                    ScriptFrame::Complete,
                ],
            });
        }
        script.push(ScriptStep {
            expected_prompt: "hello".into(),
            frames,
        });
        let config = Config::new(workspace.clone(), root.path().join("data"));
        let mut engine = Engine::open(config.clone(), ScriptedProvider::new(script))
            .await
            .unwrap();
        let session = engine.create_session().await.unwrap();
        let mut turn = engine.submit(session, "hello".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        assert_eq!(outcome.state, state);
        assert_eq!(outcome.reason, reason);
        assert_eq!(outcome.text, None);
        let audit = engine.history(session).await.unwrap();
        assert_eq!(
            audit[0],
            Message {
                role: Role::User,
                text: "hello".into()
            }
        );
        if state == TurnState::Refused {
            assert_eq!(audit.len(), 3);
            assert_eq!(audit[2].role, Role::Tool);
            assert!(audit[2].text.contains("nested instruction"));
        } else {
            assert_eq!(audit.len(), 1);
            assert_eq!(std::fs::read_dir(workspace).unwrap().count(), 0);
        }
        engine.shutdown().await.unwrap();
        let mut expected = if state == TurnState::Refused {
            vec![]
        } else {
            vec![ProtocolMessage::User("hello".into())]
        };
        expected.push(ProtocolMessage::User("safe follow-up".into()));
        let mut frames = vec![ScriptFrame::ExpectHistory(expected)];
        if state == TurnState::Refused {
            frames.push(ScriptFrame::ToolCall(
                serde_json::to_string(&inspection).unwrap(),
            ));
        } else {
            frames.push(ScriptFrame::Text("ok".into()));
        }
        frames.push(ScriptFrame::Complete);
        let mut script = vec![ScriptStep {
            expected_prompt: "safe follow-up".into(),
            frames,
        }];
        if state == TurnState::Refused {
            script.push(ScriptStep::reply("safe follow-up", "ok"));
        }
        let mut reopened = Engine::open(config, ScriptedProvider::new(script))
            .await
            .unwrap();
        assert_eq!(reopened.history(session).await.unwrap(), audit);
        assert_eq!(
            reopened.turn_outcome(session, turn.id).await.unwrap(),
            Some(outcome)
        );
        let mut next = reopened
            .submit(session, "safe follow-up".into())
            .await
            .unwrap();
        assert_eq!(next.outcome().await.unwrap().state, TurnState::Completed);
        if state == TurnState::Refused {
            let request = reopened.protocol_history(session).await.unwrap();
            let ProtocolMessage::ToolResult { result, .. } = &request.messages[2] else {
                panic!("closed tool result missing")
            };
            let result: serde_json::Value = serde_json::from_str(result).unwrap();
            assert_eq!(result["instructions"][0]["content"], "nested instruction");
        }
        reopened.shutdown().await.unwrap();
    }
}
