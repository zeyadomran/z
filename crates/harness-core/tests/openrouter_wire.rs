use harness_core::{openrouter::*, *};
use serde_json::{Value, json};
use tokio::sync::watch;
#[path = "support/wire.rs"]
mod wire;
use wire::*;
fn request() -> ProviderRequest {
    ProviderRequest{epoch:Epoch{id:ContextEpochId(1),prefix:"frozen".into(),tool_schema:r#"[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}]"#.into()},messages:vec![ProtocolMessage::User("inspect".into())]}
}
fn config() -> OpenRouterConfig {
    OpenRouterConfig::new("fixture/model".into(), "fixture".into())
}
#[tokio::test]
async fn fragmented_wire_roundtrip_preserves_reasoning_ids_and_usage_snapshot() {
    let details = json!([{"type":"reasoning.encrypted","id":"opaque-1","data":"unchanged signed data","format":"unknown","index":0}]);
    let mut first = String::from(": keepalive\r\n\r\n");
    first += &event(
        json!({"role":"assistant","content":"Inspecting","reasoning_details":details}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":"call-","type":"function","function":{"name":"read_","arguments":"{\"pa"}}]}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":"1","function":{"name":"file","arguments":"th\":\"a.txt\"}"}}]}),
        json!("tool_calls"),
    );
    first += &format!(
        "data: {}\n\n",
        json!({"id":"gen-fixture","model":"fixture/model","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":15,"completion_tokens":6,"cost":0.001}})
    );
    first += "data: [DONE]\n\n";
    let second = event(json!({"content":"done"}), json!("stop")) + "data: [DONE]\n\n";
    let (base, server) = serve(vec![Wire::sse(first), Wire::sse(second)]);
    let mut p = OpenRouterProvider::loopback_fixture(base, config())
        .await
        .unwrap();
    let (tx, rx) = watch::channel(Progress::default());
    let mut req = request();
    let r = p.respond(req.clone(), 65536, tx.clone()).await.unwrap();
    assert_eq!(r.reason, StopReason::Tools);
    let m = r.message.unwrap();
    assert_eq!(
        m.calls,
        vec![ToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into()
        }]
    );
    assert_eq!(m.opaque["reasoning_details"], details);
    assert_eq!(rx.borrow().usage.as_ref().unwrap()["prompt_tokens"], 15);
    assert_eq!(rx.borrow().text, "Inspecting");
    req.messages.push(ProtocolMessage::AssistantData(m));
    req.messages.push(ProtocolMessage::ToolResult {
        call_id: "call-1".into(),
        result: "actual tool result".into(),
    });
    let r = p.respond(req, 65536, tx).await.unwrap();
    assert_eq!(r.reason, StopReason::Final);
    let bodies = tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
    assert_eq!(bodies[1]["messages"][2]["reasoning_details"], details);
    assert_eq!(bodies[1]["messages"][3]["tool_call_id"], "call-1");
    assert_eq!(bodies[1]["provider"]["require_parameters"], true);
    assert_eq!(bodies[1]["provider"]["allow_fallbacks"], false);
    assert_eq!(bodies[1]["provider"]["data_collection"], "deny");
    assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
}
#[tokio::test]
async fn wire_terminal_and_account_failures_never_authorize_partial_calls() {
    let partial = event(
        json!({"tool_calls":[{"index":0,"id":"p","type":"function","function":{"name":"read_file","arguments":"{"}}]}),
        Value::Null,
    );
    let cases = vec![
        (Wire::sse(partial.clone()), StopReason::Transport),
        (
            Wire::sse(partial.clone() + "data: {\"error\":{\"code\":402}}\n\n"),
            StopReason::Credits,
        ),
        (
            Wire::sse(event(json!({"content":"partial"}), json!("length")) + "data: [DONE]\n\n"),
            StopReason::OutputLimit,
        ),
        (
            Wire::sse(event(json!({"refusal":"no"}), json!("stop")) + "data: [DONE]\n\n"),
            StopReason::Refused,
        ),
        (
            Wire {
                status: 401,
                kind: "application/json",
                body: "{}".into(),
                headers: String::new(),
            },
            StopReason::Authentication,
        ),
        (
            Wire {
                status: 429,
                kind: "application/json",
                body: "{}".into(),
                headers: "Retry-After: 7\r\n".into(),
            },
            StopReason::Throttled,
        ),
        (
            Wire {
                status: 503,
                kind: "application/json",
                body: "{}".into(),
                headers: String::new(),
            },
            StopReason::NoUpstream,
        ),
        (
            Wire {
                status: 400,
                kind: "application/json",
                body: json!({"error":{"metadata":{"error_type":"context_length_exceeded"}}})
                    .to_string(),
                headers: String::new(),
            },
            StopReason::ContextLimit,
        ),
    ];
    let reasons = cases.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>();
    let (base, server) = serve(cases.into_iter().map(|(w, _)| w).collect());
    let mut p = OpenRouterProvider::loopback_fixture(base, config())
        .await
        .unwrap();
    for reason in reasons {
        let (tx, _) = watch::channel(Progress::default());
        let r = p.respond(request(), 65536, tx).await.unwrap();
        assert_eq!(r.reason, reason);
        assert!(r.message.is_none());
        if reason == StopReason::Throttled {
            assert_eq!(r.retry_after, Some(std::time::Duration::from_secs(7)));
        }
    }
    tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn recognized_retries_discard_partial_calls_and_persist_each_usage_snapshot() {
    for exhausted in [false, true] {
        let mut transient = event(
            json!({"tool_calls":[{"index":0,"id":"partial","type":"function","function":{"name":"read_file","arguments":"{"}}]}),
            Value::Null,
        );
        transient += &format!(
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":10}})
        );
        transient += &format!(
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":12}})
        );
        transient += "data: {\"error\":{\"code\":429,\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}}}\n\n";
        let count = if exhausted { 3 } else { 1 };
        let mut responses = (0..count)
            .map(|_| Wire {
                status: 200,
                kind: "text/event-stream",
                body: transient.clone(),
                headers: "Retry-After: 0\r\n".into(),
            })
            .collect::<Vec<_>>();
        if !exhausted {
            responses.push(Wire::sse(
                event(json!({"content":"recovered"}), json!("stop")) + "data: [DONE]\n\n",
            ));
        }
        let (base, server) = serve(responses);
        let provider = OpenRouterProvider::loopback_fixture(base, config())
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let w = root.path().join("workspace");
        std::fs::create_dir(&w).unwrap();
        let mut engine = Engine::open(Config::new(w, root.path().join("data")), provider)
            .await
            .unwrap();
        let session = engine.create_session().await.unwrap();
        let mut turn = engine.submit(session, "inspect".into()).await.unwrap();
        let outcome = turn.outcome().await.unwrap();
        assert_eq!(
            outcome.reason,
            if exhausted {
                StopReason::Throttled
            } else {
                StopReason::Final
            }
        );
        let usage = engine.usage(session).await.unwrap();
        assert_eq!(usage.len(), if exhausted { 3 } else { 2 });
        for attempt in usage.iter().take(count) {
            assert_eq!(attempt.raw_usage.as_ref().unwrap()["prompt_tokens"], 12);
        }
        let events = engine.events(session, EventSequence(0)).await.unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.event, EventKind::ToolProposed { .. }))
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, EventKind::AttemptDiscarded { .. }))
                .count(),
            if exhausted { 2 } else { 1 }
        );
        engine.shutdown().await.unwrap();
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn catalog_validation_rejects_incompatible_routes_before_inference() {
    for wrong_route in [false, true] {
        let (base, server) = serve(vec![]);
        let mut c = config();
        if wrong_route {
            c.upstreams = vec!["missing-endpoint".into()];
        } else {
            c.max_output_tokens = 8192;
        }
        assert!(matches!(
            OpenRouterProvider::loopback_fixture(base, c).await,
            Err(Error::Configuration(_))
        ));
        assert!(
            tokio::task::spawn_blocking(move || server.join().unwrap())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
