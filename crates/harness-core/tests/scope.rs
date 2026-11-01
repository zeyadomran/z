use harness_core::*;
use serde_json::{Value, json};
#[tokio::test]
async fn scoped_tools_deny_aliases_and_preserve_instruction_provenance() {
    let root = tempfile::tempdir().unwrap();
    let w = root.path().join("workspace");
    std::fs::create_dir_all(w.join("nested")).unwrap();
    std::fs::write(w.join("AGENTS.md"), "root rule").unwrap();
    let nested_rule = "nested rule ".repeat(200);
    std::fs::write(w.join("nested/AGENTS.md"), &nested_rule).unwrap();
    std::fs::write(w.join("nested/visible.txt"), "needle\n").unwrap();
    std::fs::write(w.join("oversized.txt"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    std::fs::write(w.join("unsupported.txt"), [0xff, 0xfe]).unwrap();
    std::fs::write(w.join(".ENV"), "secret needle").unwrap();
    std::fs::hard_link(w.join(".ENV"), w.join("alias.txt")).unwrap();
    let deep = std::iter::repeat_n("deep", 16)
        .collect::<Vec<_>>()
        .join("/");
    std::fs::create_dir_all(w.join(&deep)).unwrap();
    std::fs::write(w.join(&deep).join("hidden.txt"), "needle too deep").unwrap();
    #[cfg(windows)]
    let _locked = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::write(w.join("locked.txt"), "locked needle").unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(w.join("locked.txt"))
            .unwrap()
    };
    for path in ["id_ecdsa", "id_dsa", "_netrc", ".envrc", ".git/config"] {
        std::fs::create_dir_all(w.join(path).parent().unwrap()).unwrap();
        std::fs::write(w.join(path), "secret needle").unwrap();
    }
    #[cfg(windows)]
    {
        std::fs::create_dir(root.path().join("outside")).unwrap();
        std::fs::write(root.path().join("outside/private.txt"), "outside secret").unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(w.join("escape"))
            .arg(root.path().join("outside"))
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "owned junction fixture must be created"
        );
    }
    let paths = [
        "../outside",
        ".ENV",
        "alias.txt",
        "nested/visible.txt:stream",
        "CON.txt",
        "nested/visible.txt.",
        "escape/private.txt",
        "id_ecdsa",
        "id_dsa",
        "_netrc",
        ".envrc",
        ".git/config",
    ];
    let mut frames = Vec::new();
    let mut ids = Vec::new();
    for name in ["read_file", "list_files", "search_text"] {
        for (n, path) in paths.iter().enumerate() {
            let id = format!("{name}-{n}");
            ids.push(id.clone());
            let mut args = json!({"path":path});
            if name == "search_text" {
                args["query"] = json!("needle");
            }
            frames.push(ScriptFrame::ToolCall(
                serde_json::to_string(&ToolCall {
                    id,
                    name: name.into(),
                    arguments: args.to_string(),
                })
                .unwrap(),
            ));
        }
    }
    for (id, name, args) in [
        ("list", "list_files", json!({})),
        ("nested", "read_file", json!({"path":"nested/visible.txt"})),
        ("search", "search_text", json!({"query":"needle"})),
    ] {
        frames.push(ScriptFrame::ToolCall(
            serde_json::to_string(&ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: args.to_string(),
            })
            .unwrap(),
        ));
    }
    frames.push(ScriptFrame::Complete);
    let config = Config::new(w.clone(), root.path().join("data"));
    let mut engine = Engine::open(
        config.clone(),
        ScriptedProvider::new(vec![
            ScriptStep {
                expected_prompt: "inspect".into(),
                frames,
            },
            ScriptStep::reply("inspect", "done"),
        ]),
    )
    .await
    .unwrap();
    let session = engine.create_session().await.unwrap();
    let prefix = engine.protocol_history(session).await.unwrap().epoch.prefix;
    let mut turn = engine.submit(session, "inspect".into()).await.unwrap();
    assert_eq!(turn.outcome().await.unwrap().state, TurnState::Completed);
    let request = engine.protocol_history(session).await.unwrap();
    assert_eq!(request.epoch.prefix, prefix);
    let results = request
        .messages
        .iter()
        .filter_map(|m| {
            if let ProtocolMessage::ToolResult { call_id, result } = m {
                Some((call_id, serde_json::from_str::<Value>(result).unwrap()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for id in ids {
        assert!(
            results
                .iter()
                .find(|(i, _)| **i == id)
                .unwrap()
                .1
                .get("error")
                .is_some(),
            "{id}"
        );
    }
    let list = &results.iter().find(|(id, _)| *id == "list").unwrap().1;
    assert_eq!(
        list["data"]["entries"],
        json!([
            "AGENTS.md",
            "nested/AGENTS.md",
            "nested/visible.txt",
            "oversized.txt",
            "unsupported.txt"
        ])
    );
    let search = &results.iter().find(|(id, _)| *id == "search").unwrap().1["data"];
    assert_eq!(search["matches"].as_array().unwrap().len(), 1);
    let scan_skips = if cfg!(windows) { 4 } else { 2 };
    assert_eq!(list["data"]["skipped_count"], scan_skips);
    assert_eq!(list["data"]["truncated"], true);
    assert_eq!(search["skipped_count"], scan_skips + 2);
    assert_eq!(search["truncated"], true);
    let skips = search["skipped_files"].as_array().unwrap();
    for (path, reason) in [
        ("oversized.txt", "1 MiB"),
        ("unsupported.txt", "encoding"),
        (deep.as_str(), "depth"),
        ("alias.txt", "denied"),
    ] {
        assert!(
            skips
                .iter()
                .any(|skip| skip["path"] == path
                    && skip["reason"].as_str().unwrap().contains(reason)),
            "{path}: {skips:?}"
        );
    }
    #[cfg(windows)]
    assert!(skips.iter().any(|skip| skip["path"] == "locked.txt"));
    assert!(!skips.iter().any(|skip| skip["path"] == ".ENV"
        || skip["path"] == "id_ecdsa"
        || skip["path"] == ".git"));
    let nested = &results.iter().find(|(id, _)| *id == "nested").unwrap().1;
    assert_eq!(nested["data"]["text"], "needle");
    assert_eq!(nested["instructions"], json!([]));
    assert_eq!(list["instructions"].as_array().unwrap().len(), 1);
    assert_eq!(list["instructions"][0]["origin"], "nested/AGENTS.md");
    assert_eq!(list["instructions"][0]["content"], nested_rule);
    assert_eq!(
        results.iter().find(|(id, _)| *id == "search").unwrap().1["instructions"],
        json!([])
    );
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("secret needle")
    );
    engine.shutdown().await.unwrap();
    let original_hash = list["instructions"][0]["sha256"].clone();
    // A persisted snapshot is not charged again after reopening. This budget
    // fits the read, but cannot fit the already-delivered 2400-byte instruction.
    let mut bounded = config.clone();
    bounded.max_output_bytes = 1024;
    let mut reopened = Engine::open(
        bounded,
        ScriptedProvider::new(vec![
            ScriptStep {
                expected_prompt: "unchanged".into(),
                frames: vec![
                    ScriptFrame::ToolCall(
                        serde_json::to_string(&ToolCall {
                            id: "unchanged".into(),
                            name: "read_file".into(),
                            arguments: r#"{"path":"nested/visible.txt"}"#.into(),
                        })
                        .unwrap(),
                    ),
                    ScriptFrame::Complete,
                ],
            },
            ScriptStep::reply("unchanged", "done"),
        ]),
    )
    .await
    .unwrap();
    let mut next = reopened.submit(session, "unchanged".into()).await.unwrap();
    assert_eq!(next.outcome().await.unwrap().state, TurnState::Completed);
    let unchanged = reopened.protocol_history(session).await.unwrap();
    let result = unchanged
        .messages
        .iter()
        .find_map(|m| match m {
            ProtocolMessage::ToolResult { call_id, result } if call_id == "unchanged" => {
                Some(serde_json::from_str::<Value>(result).unwrap())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(result["instructions"], json!([]));
    assert_eq!(result["data"]["text"], "needle");
    reopened.shutdown().await.unwrap();
    std::fs::write(w.join("nested/AGENTS.md"), "changed nested rule").unwrap();
    std::fs::create_dir(w.join("new")).unwrap();
    std::fs::write(w.join("new/AGENTS.md"), "new discovery").unwrap();
    let mut reopened = Engine::open(
        config.clone(),
        ScriptedProvider::new(vec![
            ScriptStep {
                expected_prompt: "again".into(),
                frames: vec![
                    ScriptFrame::ToolCall(
                        serde_json::to_string(&ToolCall {
                            id: "discover".into(),
                            name: "list_files".into(),
                            arguments: "{}".into(),
                        })
                        .unwrap(),
                    ),
                    ScriptFrame::ToolCall(
                        serde_json::to_string(&ToolCall {
                            id: "repeat".into(),
                            name: "list_files".into(),
                            arguments: "{}".into(),
                        })
                        .unwrap(),
                    ),
                    ScriptFrame::Complete,
                ],
            },
            ScriptStep::reply("again", "done"),
        ]),
    )
    .await
    .unwrap();
    let mut next = reopened.submit(session, "again".into()).await.unwrap();
    assert_eq!(next.outcome().await.unwrap().state, TurnState::Completed);
    let later = reopened.protocol_history(session).await.unwrap();
    assert_eq!(later.epoch.prefix, prefix);
    assert_eq!(&later.messages[..request.messages.len()], &request.messages);
    let discoveries = later
        .messages
        .iter()
        .filter_map(|m| match m {
            ProtocolMessage::ToolResult { call_id, result }
                if call_id == "discover" || call_id == "repeat" =>
            {
                Some(serde_json::from_str::<Value>(result).unwrap())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(discoveries[0]["instructions"].as_array().unwrap().len(), 2);
    assert_eq!(
        discoveries[0]["instructions"][0]["content"],
        "changed nested rule"
    );
    assert_ne!(discoveries[0]["instructions"][0]["sha256"], original_hash);
    assert_eq!(discoveries[0]["instructions"][1]["origin"], "new/AGENTS.md");
    assert_eq!(discoveries[1]["instructions"], json!([]));
    reopened.shutdown().await.unwrap();
    let mut supervised = config;
    supervised.policy = Policy::Supervised;
    let mut patches = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        patches.push(ScriptStep {expected_prompt:"patch denied paths".into(),frames:vec![
            ScriptFrame::ToolCall(serde_json::to_string(&ToolCall {id:format!("patch-{i}"),name:"apply_patch".into(),arguments:json!({"path":path,"expected_sha256":"must_not_exist","old_text":"","new_text":"must not write"}).to_string()}).unwrap()),ScriptFrame::Complete,
        ]});
    }
    patches.push(ScriptStep::reply("patch denied paths", "done"));
    let mut writer = Engine::open(supervised, ScriptedProvider::new(patches))
        .await
        .unwrap();
    let mut patch_turn = writer
        .submit(session, "patch denied paths".into())
        .await
        .unwrap();
    assert_eq!(
        patch_turn.outcome().await.unwrap().state,
        TurnState::Completed
    );
    let history = writer.protocol_history(session).await.unwrap();
    let patch_results: Vec<_> = history
        .messages
        .iter()
        .filter_map(|m| match m {
            ProtocolMessage::ToolResult { call_id, result } if call_id.starts_with("patch-") => {
                Some(serde_json::from_str::<Value>(result).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(patch_results.len(), paths.len());
    for result in patch_results {
        assert!(
            result["error"].as_str().unwrap().contains("denied"),
            "{result}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(w.join(".ENV")).unwrap(),
        "secret needle"
    );
    writer.shutdown().await.unwrap();
}
