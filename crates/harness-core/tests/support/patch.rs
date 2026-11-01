use harness_core::*;
use serde_json::{Value, json};
pub fn edit(id: &str, path: &str, expected: &str, old: &str, new: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "apply_patch".into(),
        arguments: json!({"path":path,"expected_sha256":expected,"old_text":old,"new_text":new})
            .to_string(),
    }
}
pub fn script(calls: Vec<ToolCall>) -> ScriptedProvider {
    let mut frames: Vec<_> = calls
        .iter()
        .map(|c| ScriptFrame::ToolCall(serde_json::to_string(c).unwrap()))
        .collect();
    frames.push(ScriptFrame::Complete);
    ScriptedProvider::new(vec![
        ScriptStep {
            expected_prompt: "edit".into(),
            frames,
        },
        ScriptStep::reply("edit", "done"),
    ])
}
pub fn results(history: &ProviderRequest) -> Vec<Value> {
    ScriptedProvider::new(vec![])
        .validate_request(history)
        .unwrap();
    history
        .messages
        .iter()
        .filter_map(|m| {
            if let ProtocolMessage::ToolResult { result, .. } = m {
                Some(serde_json::from_str(result).unwrap())
            } else {
                None
            }
        })
        .collect()
}
