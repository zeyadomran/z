#![cfg(windows)]
use harness_core::{chatgpt::*, *};
use serde_json::{Value, json};
#[path = "support/chatgpt.rs"]
mod fixture;
use fixture::*;
pub fn config() -> ChatGptConfig {
    ChatGptConfig {
        profile: "one".into(),
        model: "fixture-model".into(),
        context_tokens: 128000,
        response_headroom: 4096,
        limits_source: "owned offline fixture; not live qualification".into(),
    }
}
pub fn event(v: Value) -> String {
    format!("data: {v}\r\n\r\n")
}
pub fn completed(mut output: Value) -> String {
    let mut s = String::new();
    for (i, item) in output.as_array().unwrap().iter().enumerate() {
        s += &event(json!({"type":"response.output_item.done","output_index":i,"item":item}));
    }
    // Only the completed item carries the opaque value; the terminal envelope
    // is intentionally insufficient to reconstruct it on its own.
    for item in output.as_array_mut().unwrap() {
        if item["type"] == "reasoning" {
            item.as_object_mut().unwrap().remove("encrypted_content");
        }
    }
    s + &event(
        json!({"type":"response.completed","response":{"id":"response-fixture","model":"fixture-model","status":"completed","output":output,"usage":{"input_tokens":20,"output_tokens":8,"total_tokens":28,"input_tokens_details":{"cached_tokens":4},"output_tokens_details":{"reasoning_tokens":3}}}}),
    )
}
pub fn answer() -> Value {
    json!([{"type":"message","id":"msg-1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"The answer is 42.","annotations":[]}]}])
}

impl Reply {
    fn sse(body: String) -> Self {
        Self {
            status: 200,
            body,
            kind: "text/event-stream",
        }
    }
}
#[tokio::test]
async fn direct_wire_uses_real_engine_and_reopens_account_bound_opaque_history() {
    let output = json!([{"type":"reasoning","id":"reason-1","encrypted_content":"completed opaque state","summary":[]},{"type":"function_call","id":"item-1","namespace":"workspace","name":"read_file","call_id":"read-1","arguments":"{\"path\":\"answer.txt\"}","status":"completed"}]);
    let server = Server::start(vec![
        Reply::sse(event(json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"reason-1","encrypted_content":"incomplete","summary":[]}}))+&completed(output.clone())),
        Reply::sse(completed(answer())),
        Reply::sse(completed(answer())),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let auth = server.auth(root.path());
    server.login(&auth, "one").await.unwrap();
    let mut invalid = config();
    invalid.context_tokens = 0;
    assert!(ChatGptProvider::new(auth.clone(), invalid).await.is_err());
    let mut absent = config();
    absent.model = "not-in-this-account".into();
    assert!(ChatGptProvider::new(auth.clone(), absent).await.is_err());
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("answer.txt"), "42").unwrap();
    let c = Config::new(workspace.clone(), root.path().join("data"));
    let provider = ChatGptProvider::new(auth, config()).await.unwrap();
    let mut engine = Engine::open(c.clone(), provider).await.unwrap();
    let session = engine.create_session().await.unwrap();
    let outcome = engine
        .submit(session, "inspect".into())
        .await
        .unwrap()
        .outcome()
        .await
        .unwrap();
    assert_eq!(outcome.state, TurnState::Completed);
    assert_eq!(outcome.text.as_deref(), Some("The answer is 42."));
    let history = engine.protocol_history(session).await.unwrap();
    let ProtocolMessage::AssistantData(message) = &history.messages[1] else {
        panic!()
    };
    assert_eq!(message.opaque["output"], output);
    let ProtocolMessage::ToolResult { call_id, result } = &history.messages[2] else {
        panic!()
    };
    assert_eq!(call_id, "read-1");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(result).unwrap()["data"]["text"],
        "42"
    );
    assert_eq!(
        engine.usage(session).await.unwrap()[0]
            .raw_usage
            .as_ref()
            .unwrap()["total_tokens"],
        28
    );
    engine.shutdown().await.unwrap();
    std::fs::write(workspace.join("answer.txt"), "changed").unwrap();
    let provider = ChatGptProvider::new(server.auth(root.path()), config())
        .await
        .unwrap();
    let mut engine = Engine::open(c.clone(), provider).await.unwrap();
    assert_eq!(
        engine.protocol_history(session).await.unwrap().messages,
        history.messages
    );
    assert_eq!(
        engine
            .submit(session, "continue".into())
            .await
            .unwrap()
            .outcome()
            .await
            .unwrap()
            .state,
        TurnState::Completed
    );
    engine.shutdown().await.unwrap();
    server
        .login(&server.auth(root.path()), "two")
        .await
        .unwrap();
    let mut other = config();
    other.profile = "two".into();
    let provider = ChatGptProvider::new(server.auth(root.path()), other)
        .await
        .unwrap();
    let mut engine = Engine::open(c, provider).await.unwrap();
    assert!(matches!(
        engine.protocol_history(session).await,
        Err(Error::Binding)
    ));
    engine.shutdown().await.unwrap();
    {
        let s = server.state.lock().unwrap();
        let first = &s.requests[0].1;
        let second = &s.requests[1].1;
        assert_eq!(
            first
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["input", "instructions", "model", "store", "stream", "tools"]
        );
        assert_eq!(first["store"], false);
        assert_eq!(first["stream"], true);
        assert_eq!(first["tools"][0]["type"], "namespace");
        assert_eq!(first["tools"][0]["name"], "workspace");
        assert_eq!(first["tools"], second["tools"]);
        assert_eq!(first["instructions"], second["instructions"]);
        assert_eq!(second["input"][1], output[0]);
        assert_eq!(second["input"][2], output[1]);
        assert_eq!(second["input"][3]["call_id"], "read-1");
        assert_eq!(second["input"][3]["type"], "function_call_output");
    }
    server.close().await;
}
#[tokio::test]
async fn incomplete_conflicting_or_quota_streams_execute_nothing_and_suspend_truthfully() {
    let reasoning =
        json!({"type":"reasoning","id":"reason-1","encrypted_content":"done-only","summary":[]});
    let tool = json!({"type":"function_call","id":"item-1","namespace":"workspace","name":"read_file","call_id":"read-1","arguments":"{\"path\":\"answer.txt\"}","status":"completed"});
    let terminal = |output| {
        event(
            json!({"type":"response.completed","response":{"id":"response-fixture","model":"fixture-model","status":"completed","output":output}}),
        )
    };
    let partial = event(json!({"type":"response.output_item.added","output_index":0,"item":tool}));
    let cases = vec![
        (partial.clone(), StopReason::Transport),
        (
            partial.clone()
                + &event(
                    json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}}),
                ),
            StopReason::SubscriptionLimit,
        ),
        (terminal(json!([reasoning, tool])), StopReason::Protocol),
        (
            event(json!({"type":"response.output_item.done","output_index":0,"item":reasoning}))
                + &terminal(
                    json!([{"type":"reasoning","id":"reason-1","encrypted_content":"conflict","summary":[]},tool]),
                ),
            StopReason::Protocol,
        ),
        (
            completed(
                json!([{"type":"custom_tool_call","id":"custom","call_id":"read-1","name":"read_file","input":"answer.txt"}]),
            ),
            StopReason::Protocol,
        ),
        (
            partial
                + &event(
                    json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
                ),
            StopReason::OutputLimit,
        ),
    ];
    for (wire, reason) in cases {
        let server = Server::start(vec![Reply::sse(wire)]).await;
        let root = tempfile::tempdir().unwrap();
        let auth = server.auth(root.path());
        server.login(&auth, "one").await.unwrap();
        let provider = ChatGptProvider::new(auth, config()).await.unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("answer.txt"), "42").unwrap();
        let mut engine = Engine::open(Config::new(workspace, root.path().join("data")), provider)
            .await
            .unwrap();
        let session = engine.create_session().await.unwrap();
        let outcome = engine
            .submit(session, "inspect".into())
            .await
            .unwrap()
            .outcome()
            .await
            .unwrap();
        assert_eq!(outcome.reason, reason);
        assert!(
            !engine
                .events(session, EventSequence(0))
                .await
                .unwrap()
                .iter()
                .any(|e| matches!(
                    e.event,
                    EventKind::ToolProposed { .. } | EventKind::ToolFinished { .. }
                ))
        );
        if reason == StopReason::SubscriptionLimit {
            assert_eq!(outcome.state, TurnState::SuspendedRecovery);
            assert_eq!(
                engine.usage(session).await.unwrap()[0]
                    .metadata
                    .as_ref()
                    .unwrap()
                    .diagnostic
                    .as_ref()
                    .unwrap()["code"],
                "subscription_sharing_usage_limit_exceeded"
            );
        }
        engine.shutdown().await.unwrap();
        server.close().await;
    }
}
#[tokio::test]
async fn one_401_refresh_uses_the_engine_attempt_budget_and_does_not_loop_signin() {
    let rejection = || {
        Reply::json(
            401,
            json!({"error":{"code":"subscription_sharing_invalid_user"}}),
        )
    };
    let server = Server::start(vec![
        rejection(),
        rejection(),
        Reply::sse(completed(answer())),
    ])
    .await;
    let root = tempfile::tempdir().unwrap();
    let auth = server.auth(root.path());
    server.login(&auth, "one").await.unwrap();
    let provider = ChatGptProvider::new(auth, config()).await.unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let c = Config::new(workspace, root.path().join("data"));
    let mut engine = Engine::open(c.clone(), provider).await.unwrap();
    let session = engine.create_session().await.unwrap();
    let outcome = engine
        .submit(session, "inspect".into())
        .await
        .unwrap()
        .outcome()
        .await
        .unwrap();
    assert_eq!(outcome.reason, StopReason::AccountDiagnosis);
    assert_eq!(outcome.state, TurnState::SuspendedRecovery);
    assert_eq!(engine.usage(session).await.unwrap().len(), 2);
    engine.shutdown().await.unwrap();
    {
        let s = server.state.lock().unwrap();
        assert_eq!(s.refreshes, 1);
        assert_ne!(s.requests[0].0, s.requests[1].0);
    }
    let provider = ChatGptProvider::new(server.auth(root.path()), config())
        .await
        .unwrap();
    let mut engine = Engine::open(c, provider).await.unwrap();
    assert_eq!(
        engine
            .resume(session, outcome.turn_id)
            .await
            .unwrap()
            .outcome()
            .await
            .unwrap()
            .state,
        TurnState::Completed
    );
    assert_eq!(engine.usage(session).await.unwrap().len(), 3);
    engine.shutdown().await.unwrap();
    server.close().await;
}
