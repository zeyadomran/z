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
    OpenRouterConfig::new("fixture/model".into(), "fixture/spot".into())
}
#[tokio::test]
async fn fragmented_wire_roundtrip_preserves_reasoning_ids_and_usage_snapshot() {
    let details = json!([{"type":"reasoning.encrypted","id":"opaque-1","data":"unchanged signed data","format":"unknown","index":1}]);
    let mut first = String::from(": keepalive\r\n\r\n");
    first += &event(
        json!({"role":"assistant","content":"Inspecting","reasoning_details":details}),
        Value::Null,
    );
    for _ in 0..4096 {
        first += &event(
            json!({"reasoning":"think", "reasoning_details":[{"type":"reasoning.text","index":0,"text":"think","signature":null,"format":"openai-responses-v1","id":"reasoning-trace"}]}),
            Value::Null,
        );
    }
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","index":0,"signature":"signed"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","index":0,"signature":"signed"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","index":0,"text":"second","signature":"signature-2"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","text":"un"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","text":"indexed"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.text","signature":"signature-3"}]}),
        Value::Null,
    );
    first += &event(
        json!({"reasoning_details":[{"type":"reasoning.encrypted","index":1,"id":"opaque-2","data":"second blob"}]}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":"call-","type":"function","function":{"name":"read_","arguments":"{\"pa"}}]}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":null,"type":null,"function":{"name":null,"arguments":null}}]}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":"1","function":{"name":"file","arguments":"th\":\"a.txt\"}"}}]}),
        Value::Null,
    );
    first += &event(
        json!({"tool_calls":[{"index":0,"id":"call-1","function":{"name":"read_file","arguments":null}}]}),
        json!("tool_calls"),
    );
    first += &format!(
        "data: {}\n\n",
        json!({"id":"gen-fixture","model":"fixture/model","choices":[{"index":0,"delta":{"reasoning_details":[]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":15,"completion_tokens":6,"cost":0.001}})
    );
    first += "data: [DONE]\n\n";
    first = first.replace(",\"provider\":\"Fixture\"", "");
    let second = event(json!({"content":"done","refusal":""}), json!("stop")) + "data: [DONE]\n\n";
    let (base, server) = serve(vec![Wire::sse(first), Wire::sse(second)]);
    let mut c = config();
    c.model = "~fixture/latest".into();
    let mut p = OpenRouterProvider::loopback_fixture(base, c).await.unwrap();
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
    let details = json!([details[0], {"type":"reasoning.text","index":0,"text":"think".repeat(4096),"signature":"signed","format":"openai-responses-v1","id":"reasoning-trace"}, {"type":"reasoning.text","index":0,"text":"second","signature":"signature-2"},{"type":"reasoning.text","text":"unindexed","signature":"signature-3"}, {"type":"reasoning.encrypted","index":1,"id":"opaque-2","data":"second blob"}]);
    assert_eq!(m.opaque["reasoning_details"], details);
    assert_eq!(m.opaque["route"], "fixture/spot");
    assert!(m.opaque.get("reasoning").is_none());
    assert!(rx.borrow().bytes < 22000);
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
    assert_eq!(bodies[0]["model"], "fixture/model");
    assert_eq!(bodies[1]["provider"]["only"], json!(["fixture/spot"]));
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
        (
            Wire {
                status: 404,
                kind: "application/json",
                body: "{}".into(),
                headers: String::new(),
            },
            StopReason::NoUpstream,
        ),
        (
            Wire {
                status: 404,
                kind: "application/json",
                body: json!({"error":{"code":"not_found"}}).to_string(),
                headers: String::new(),
            },
            StopReason::NoUpstream,
        ),
        (
            Wire::sse(
                event(
                    json!({"reasoning_details":[{"type":"reasoning.encrypted","data":"opaque"}]}),
                    json!("stop"),
                )
                .replace(",\"provider\":\"Fixture\"", "")
                    + "data: [DONE]\n\n",
            ),
            StopReason::Protocol,
        ),
        (
            Wire::sse(
                event(
                    json!({"tool_calls":[{"index":0,"id":"original","function":{"name":"list_files","arguments":"{}"}}]}),
                    Value::Null,
                ) + &event(
                    json!({"tool_calls":[{"index":0,"id":"different","function":{"name":"list_files"}}]}),
                    json!("tool_calls"),
                ) + "data: [DONE]\n\n",
            ),
            StopReason::Protocol,
        ),
        (
            Wire::sse(
                "data: {\"provider\":\"openai\",\"error\":{\"code\":\"server_error\"}}\n\n".into(),
            ),
            StopReason::Unavailable,
        ),
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
    let mut c = config();
    c.upstreams.push("fixture-other".into());
    let mut p = OpenRouterProvider::loopback_fixture(base, c).await.unwrap();
    for reason in reasons {
        let (tx, _) = watch::channel(Progress {
            upstream: Some("Fixture".into()),
            ..Progress::default()
        });
        let r = match p.respond(request(), 65536, tx).await {
            Err(Error::Protocol(_)) if reason == StopReason::Protocol => {
                ProviderResponse::stopped(StopReason::Protocol)
            }
            result => result.unwrap(),
        };
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
    for invalid in ["output", "route", "ambiguous", "prefix"] {
        let (base, server) = serve(vec![]);
        let mut c = config();
        match invalid {
            "route" => c.upstreams = vec!["missing-endpoint".into()],
            "ambiguous" => c.upstreams.push("fixture/us".into()),
            "prefix" => c.upstreams = vec!["fixture".into()],
            _ => c.max_output_tokens = 8192,
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

#[tokio::test]
async fn completed_invalid_arguments_return_repairable_results() {
    let calls = json!([
        {"index":0,"id":"invalid","type":"function","function":{"name":"read_file","arguments":"{bad"}},
        {"index":1,"id":"empty","type":"function","function":{"name":"list_files","arguments":""}}
    ]);
    let (base, server) = serve(vec![
        Wire::sse(event(json!({"tool_calls":calls}), json!("tool_calls")) + "data: [DONE]\n\n"),
        Wire::sse(event(json!({"content":"repaired"}), json!("stop")) + "data: [DONE]\n\n"),
    ]);
    let provider = OpenRouterProvider::loopback_fixture(base, config())
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("visible.txt"), "visible").unwrap();
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
    assert_eq!(outcome.reason, StopReason::Final);
    let history = engine.protocol_history(session).await.unwrap();
    let ProtocolMessage::AssistantData(message) = &history.messages[1] else {
        panic!("missing completed calls")
    };
    assert_eq!(message.calls[0].arguments, "{bad");
    assert_eq!(message.calls[1].arguments, "{}");
    let ProtocolMessage::ToolResult { result, .. } = &history.messages[2] else {
        panic!("missing error result")
    };
    assert_eq!(
        serde_json::from_str::<Value>(result).unwrap()["error"],
        "scoped inspection failed: invalid tool arguments"
    );
    let ProtocolMessage::ToolResult { result, .. } = &history.messages[3] else {
        panic!("missing list result")
    };
    assert_eq!(
        serde_json::from_str::<Value>(result).unwrap()["data"]["entries"],
        json!(["visible.txt"])
    );
    engine.shutdown().await.unwrap();
    let bodies = tokio::task::spawn_blocking(move || server.join().unwrap())
        .await
        .unwrap();
    assert_eq!(
        bodies[1]["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{bad"
    );
}
