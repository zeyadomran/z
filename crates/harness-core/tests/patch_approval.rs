use harness_core::*;
#[path = "support/patch.rs"]
mod support;
use support::{edit, results, script};

#[tokio::test]
async fn denial_staleness_and_restart_never_reuse_approval_or_replay_completed_calls() {
    for branch in ["inspect", "deny", "stale", "restart", "cancel", "malformed"] {
        let root = tempfile::tempdir().unwrap();
        let w = root.path().join("workspace");
        std::fs::create_dir(&w).unwrap();
        std::fs::write(w.join("read.txt"), "original read").unwrap();
        let mut config = Config::new(w.clone(), root.path().join("data"));
        config.policy = if branch == "inspect" {
            Policy::Inspect
        } else {
            Policy::Supervised
        };
        let calls = vec![
            ToolCall {
                id: "read".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"read.txt"}"#.into(),
            },
            edit("first", "new.txt", "must_not_exist", "", "new\n"),
            if branch == "malformed" {
                ToolCall {
                    id: "bad".into(),
                    name: "read_file".into(),
                    arguments: "{bad".into(),
                }
            } else {
                edit("later", "later.txt", "must_not_exist", "", "later\n")
            },
        ];
        let mut engine = Engine::open(config.clone(), script(calls)).await.unwrap();
        let session = engine.create_session().await.unwrap();
        let schema = engine
            .protocol_history(session)
            .await
            .unwrap()
            .epoch
            .tool_schema;
        assert!(schema.contains("apply_patch"));
        let mut turn = engine.submit(session, "edit".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        if matches!(branch, "inspect" | "malformed") {
            assert_eq!(outcome.state, TurnState::Completed);
        } else {
            assert_eq!(outcome.state, TurnState::AwaitingApproval);
            let preview = engine.preview_patch(session, turn.id).await.unwrap();
            std::fs::write(w.join("read.txt"), "changed read; must not replay").unwrap();
            match branch {
                "cancel" => {
                    engine.abandon(session, turn.id).await.unwrap();
                }
                "restart" => {
                    engine.shutdown().await.unwrap();
                    engine = Engine::open(
                        config.clone(),
                        ScriptedProvider::new(vec![ScriptStep::reply("edit", "done")]),
                    )
                    .await
                    .unwrap();
                    assert!(matches!(
                        engine
                            .decide_patch(&preview.approval_id, ApprovalDecision::Approve)
                            .await,
                        Err(Error::Binding)
                    ));
                    let mut resumed = engine.resume(session, turn.id).await.unwrap();
                    assert_eq!(
                        resumed.outcome().await.unwrap().state,
                        TurnState::AwaitingApproval
                    );
                    let fresh = engine.preview_patch(session, turn.id).await.unwrap();
                    assert_ne!(fresh.approval_id, preview.approval_id);
                    let mut denied = engine
                        .decide_patch(&fresh.approval_id, ApprovalDecision::Deny)
                        .await
                        .unwrap();
                    assert_eq!(denied.outcome().await.unwrap().state, TurnState::Completed);
                }
                _ => {
                    if branch == "stale" {
                        std::fs::write(w.join("new.txt"), "external create").unwrap();
                    }
                    let mut resumed = engine
                        .decide_patch(
                            &preview.approval_id,
                            if branch == "deny" {
                                ApprovalDecision::Deny
                            } else {
                                ApprovalDecision::Approve
                            },
                        )
                        .await
                        .unwrap();
                    assert_eq!(resumed.outcome().await.unwrap().state, TurnState::Completed);
                }
            }
        }
        assert!(!w.join("later.txt").exists());
        if branch == "stale" {
            assert_eq!(
                std::fs::read_to_string(w.join("new.txt")).unwrap(),
                "external create"
            );
        } else {
            assert!(!w.join("new.txt").exists());
        }
        let history = engine.protocol_history(session).await.unwrap();
        let results = results(&history);
        if branch != "malformed" {
            assert_eq!(results[0]["data"]["text"], "original read");
        }
        assert_eq!(history.epoch.tool_schema, schema);
        engine.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn exact_edit_validation_rejects_mixed_ambiguous_oversized_and_changed_inputs() {
    use sha2::{Digest, Sha256};
    for branch in [
        "mixed",
        "ambiguous",
        "large_file",
        "large_result",
        "large_arguments",
        "changed_hash",
    ] {
        let root = tempfile::tempdir().unwrap();
        let w = root.path().join("workspace");
        std::fs::create_dir(&w).unwrap();
        let bytes = match branch {
            "mixed" => b"before\r\nother\n".to_vec(),
            "ambiguous" => b"before before\n".to_vec(),
            "large_file" => vec![b'x'; 1024 * 1024 + 1],
            "large_result" => {
                let mut b = b"before".to_vec();
                b.resize(1024 * 1024, b'x');
                b
            }
            _ => b"before\n".to_vec(),
        };
        std::fs::write(w.join("file.txt"), &bytes).unwrap();
        let digest = if branch == "changed_hash" {
            "0".repeat(64)
        } else {
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let replacement = if branch == "large_arguments" {
            "x".repeat(256 * 1024)
        } else {
            "replacement".into()
        };
        let mut config = Config::new(w.clone(), root.path().join("data"));
        config.policy = Policy::Supervised;
        config.max_output_bytes = 1024 * 1024;
        let mut e = Engine::open(
            config,
            script(vec![edit(
                "invalid",
                "file.txt",
                &digest,
                "before",
                &replacement,
            )]),
        )
        .await
        .unwrap();
        let session = e.create_session().await.unwrap();
        let mut turn = e.submit(session, "edit".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        assert_ne!(outcome.state, TurnState::AwaitingApproval);
        assert!(e.preview_patch(session, turn.id).await.is_err());
        assert_eq!(std::fs::read(w.join("file.txt")).unwrap(), bytes);
        assert_eq!(std::fs::read_dir(&w).unwrap().count(), 1);
        e.shutdown().await.unwrap();
    }
}
