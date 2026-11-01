use harness_core::*;
use rusqlite::Connection;
use std::io::{BufRead, BufReader};
#[cfg(windows)]
#[path = "support/patch_acl.rs"]
mod native_acl;
#[path = "support/patch.rs"]
mod support;

#[test]
fn patch_crash_child() {
    let Some(root) = std::env::var_os("OWNED_PATCH_CRASH") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let boundary = std::env::var("OWNED_PATCH_BOUNDARY").unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let mut config=Config::new(root.join("workspace"),root.join("data"));config.policy=Policy::Supervised;
        let mut e=Engine::open(config.clone(),support::script(vec![
            ToolCall{id:"read".into(),name:"read_file".into(),arguments:r#"{"path":"file.txt"}"#.into()},
            support::edit("edit","file.txt","9160d4be34c8695bd172a76c7c7966587ea5a4d991ad22c87b2b91af54aa9ebb","before","after"),
            support::edit("unstarted","later.txt","must_not_exist","","later\n"),
        ])).await.unwrap();
        let session=e.create_session().await.unwrap();
        let mut turn=e.submit(session,"edit".into()).await.unwrap();
        assert_eq!(turn.outcome().await.unwrap().state,TurnState::AwaitingApproval);
        let preview=e.preview_patch(session,turn.id).await.unwrap();
        let c=Connection::open(config.data_dir.join("harness.sqlite")).unwrap();
        let trigger=match boundary.as_str() {
            "intent"=>"CREATE TRIGGER owned_failure BEFORE INSERT ON patch_attempts BEGIN SELECT RAISE(ABORT,'owned patch boundary'); END;",
            "staged"=>"CREATE TRIGGER owned_failure BEFORE UPDATE ON patch_attempts WHEN NEW.state='staged' BEGIN SELECT RAISE(ABORT,'owned patch boundary'); END;",
            _=>"CREATE TRIGGER owned_failure BEFORE UPDATE ON tool_calls WHEN NEW.provider_id='edit' AND NEW.state='resolved' BEGIN SELECT RAISE(ABORT,'owned patch boundary'); END;",
        };
        c.execute_batch(trigger).unwrap();
        let mut approved=e.decide_patch(&preview.approval_id,ApprovalDecision::Approve).await.unwrap();
        assert!(matches!(approved.outcome().await,Err(Error::Persistence(message)) if message.contains("owned patch boundary")));
        // The exact failed commit is observed before the parent kills this owned
        // process. Intent/artifacts are real, and no artificial sleep is needed.
        println!("READY {} {}",session.0,turn.id.0);
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn interrupted_patch_reconciles_bytes_and_metadata_without_replaying_effects() {
    for branch in [
        "intent",
        "staged",
        "applied",
        "metadata",
        "content",
        "abandon",
        "native_merge",
    ] {
        let root = tempfile::tempdir().unwrap();
        let w = root.path().join("workspace");
        #[cfg(windows)]
        if matches!(branch, "applied" | "native_merge") {
            native_acl::legacy_parent(&w);
        } else {
            std::fs::create_dir(&w).unwrap();
        }
        #[cfg(not(windows))]
        std::fs::create_dir(&w).unwrap();
        std::fs::write(w.join("file.txt"), "before\n").unwrap();
        let sentinel = w.join(".harness-patch-user-owned-stage");
        std::fs::write(&sentinel, "never remove").unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "patch_crash_child", "--nocapture"])
            .env("OWNED_PATCH_CRASH", root.path())
            .env("OWNED_PATCH_BOUNDARY", branch)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let ids = tokio::task::spawn_blocking(move || {
            BufReader::new(stdout)
                .lines()
                .map(|l| l.unwrap())
                .find_map(|l| {
                    l.strip_prefix("READY ").map(|s| {
                        s.split_whitespace()
                            .map(|v| v.parse::<i64>().unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .expect("owned child did not reach boundary")
        })
        .await
        .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let session = SessionId(ids[0]);
        let turn = TurnId(ids[1]);
        let c = Connection::open(root.path().join("data/harness.sqlite")).unwrap();
        c.execute_batch("DROP TRIGGER owned_failure").unwrap();
        #[cfg(windows)]
        if branch == "native_merge" {
            let serialized: String = c
                .query_row("SELECT intent FROM patch_attempts", [], |r| r.get(0))
                .unwrap();
            let intent: serde_json::Value = serde_json::from_str(&serialized).unwrap();
            assert_eq!(
                intent["expected_metadata"],
                intent["prepared"]["before_metadata"]
            );
            assert_eq!(
                intent["expected_metadata"]["security"],
                native_acl::raw_acl(&w.join("file.txt"))
            );
            let backup = std::path::Path::new(intent["backup"].as_str().unwrap());
            assert_eq!(std::fs::read(backup).unwrap(), b"before\n");
        }
        if matches!(branch, "applied" | "staged") {
            // Old builds serialized spare SDDL buffer NULs. Reopen must compare
            // their logical strings without rewriting the durable evidence.
            let serialized: String = c
                .query_row("SELECT intent FROM patch_attempts", [], |r| r.get(0))
                .unwrap();
            let mut intent: serde_json::Value = serde_json::from_str(&serialized).unwrap();
            for pointer in [
                "/prepared/before_metadata/security",
                "/prepared/parent_metadata/security",
                "/expected_metadata/security",
            ] {
                let Some(security) = intent.pointer_mut(pointer) else {
                    assert_eq!(branch, "staged"); // No intended create metadata yet.
                    continue;
                };
                *security =
                    serde_json::Value::String(format!("{}\0\0", security.as_str().unwrap()));
            }
            c.execute("UPDATE patch_attempts SET intent=?1", [intent.to_string()])
                .unwrap();
        }
        c.close().unwrap();
        #[cfg(windows)]
        if branch == "native_merge" {
            native_acl::replace_without_restore(&w.join("file.txt"));
        }
        #[cfg(windows)]
        if branch == "metadata" {
            native_acl::acl(&w.join("file.txt"), true);
        }
        if branch == "content" {
            std::fs::write(w.join("file.txt"), "user change\n").unwrap();
        }
        let mut config = Config::new(w.clone(), root.path().join("data"));
        config.policy = Policy::Supervised;
        let mut e = Engine::open(
            config,
            ScriptedProvider::new(vec![ScriptStep::reply("edit", "done")]),
        )
        .await
        .unwrap();
        if branch == "abandon" {
            assert_eq!(
                e.abandon(session, turn).await.unwrap().state,
                TurnState::Cancelled
            );
        } else {
            let mut resumed = e.resume(session, turn).await.unwrap();
            let result = resumed.outcome().await.unwrap();
            if matches!(branch, "metadata" | "content" | "native_merge") {
                assert_eq!(result.state, TurnState::SuspendedRecovery);
                assert!(e.preview_patch(session, turn).await.is_err());
                e.abandon(session, turn).await.unwrap();
            } else {
                assert_eq!(result.state, TurnState::AwaitingApproval);
                let fresh = e.preview_patch(session, turn).await.unwrap();
                assert_eq!(
                    fresh.call_id,
                    if branch == "applied" {
                        "unstarted"
                    } else {
                        "edit"
                    }
                );
                let mut denied = e
                    .decide_patch(&fresh.approval_id, ApprovalDecision::Deny)
                    .await
                    .unwrap();
                assert_eq!(denied.outcome().await.unwrap().state, TurnState::Completed);
            }
        }
        let result = support::results(&e.protocol_history(session).await.unwrap());
        assert_eq!(result[0]["data"]["text"], "before");
        if matches!(branch, "applied" | "abandon") {
            assert_eq!(result[1]["status"], "succeeded");
            assert_eq!(result[1]["reconciled"], true);
        }
        if matches!(branch, "metadata" | "content" | "native_merge") {
            assert_eq!(result[1]["status"], "abandoned");
        }
        assert_eq!(
            std::fs::read_to_string(w.join("file.txt")).unwrap(),
            match branch {
                "intent" | "staged" => "before\n",
                "content" => "user change\n",
                _ => "after\n",
            }
        );
        assert!(!w.join("later.txt").exists());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "never remove");
        if branch == "native_merge" {
            assert!(w.join("file.txt.owned-merge-backup").exists());
        }
        e.shutdown().await.unwrap();
    }
}
