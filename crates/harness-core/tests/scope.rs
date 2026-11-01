use harness_core::*;
use serde_json::{Value, json};
#[tokio::test]
async fn scoped_tools_deny_aliases_and_preserve_instruction_provenance() {
    let root = tempfile::tempdir().unwrap();
    let w = root.path().join("workspace");
    std::fs::create_dir_all(w.join("nested")).unwrap();
    std::fs::write(w.join("AGENTS.md"), "root rule").unwrap();
    std::fs::write(w.join("nested/AGENTS.md"), "nested rule").unwrap();
    std::fs::write(w.join("nested/visible.txt"), "needle\n").unwrap();
    std::fs::write(w.join(".ENV"), "secret needle").unwrap();
    std::fs::hard_link(w.join(".ENV"), w.join("alias.txt")).unwrap();
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
    let mut engine = Engine::open(
        Config::new(w.clone(), root.path().join("data")),
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
        json!(["AGENTS.md", "nested/AGENTS.md", "nested/visible.txt"])
    );
    let nested = &results.iter().find(|(id, _)| *id == "nested").unwrap().1;
    assert_eq!(nested["data"]["text"], "needle");
    assert_eq!(nested["instructions"][1]["origin"], "nested/AGENTS.md");
    assert_eq!(nested["instructions"][1]["content"], "nested rule");
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("secret needle")
    );
    engine.shutdown().await.unwrap();
}
