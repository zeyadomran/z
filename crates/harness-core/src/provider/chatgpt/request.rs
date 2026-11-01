use super::*;
pub(super) fn body(r: ProviderRequest, model: &str, account: &str) -> Result<Value> {
    ScriptedProvider::new(vec![]).validate_request(&r)?;
    let schemas: Value = serde_json::from_str(&r.epoch.tool_schema)?;
    let tools = schemas
        .as_array()
        .ok_or_else(protocol)?
        .iter()
        .map(|t| {
            if t["type"] != "function" {
                return Err(protocol());
            }
            let function = t["function"].as_object().ok_or_else(protocol)?;
            if function
                .keys()
                .any(|k| !matches!(k.as_str(), "name" | "description" | "parameters"))
            {
                return Err(protocol());
            }
            let mut function = function.clone();
            function.insert("type".into(), json!("function"));
            Ok(Value::Object(function))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut input = Vec::new();
    for message in r.messages {
        match message {
            ProtocolMessage::User(text) => input.push(json!({"role":"user","content":text})),
            ProtocolMessage::ToolResult { call_id, result } => {
                input.push(json!({"type":"function_call_output","call_id":call_id,"output":result}))
            }
            ProtocolMessage::AssistantData(message) => {
                if message.opaque["format"] != "chatgpt-responses-v1"
                    || message.opaque["account"] != account
                {
                    return Err(protocol());
                }
                let output = message.opaque["output"].as_array().ok_or_else(protocol)?;
                input.extend(output.iter().cloned());
            }
            ProtocolMessage::Assistant { .. } => return Err(protocol()),
        }
    }
    Ok(
        json!({"model":model,"instructions":r.epoch.prefix,"input":input,"tools":[{"type":"namespace","name":"workspace","description":"Approved local workspace operations.","tools":tools}],"store":false,"stream":true}),
    )
}
