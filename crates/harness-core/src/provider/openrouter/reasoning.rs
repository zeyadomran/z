use super::*;
use serde_json::Map;

fn invalid() -> Error {
    Error::Protocol("inconsistent OpenRouter reasoning detail".into())
}

pub(super) fn append(opaque: &mut Value, delta: &Map<String, Value>) -> Result<()> {
    if let Some(details) = delta.get("reasoning_details").filter(|v| !v.is_null()) {
        let details = details.as_array().ok_or_else(invalid)?;
        if !details.is_empty() {
            if !opaque.is_object() {
                *opaque = json!({});
            }
            // The detailed representation is authoritative for continuation.
            opaque
                .as_object_mut()
                .ok_or_else(invalid)?
                .remove("reasoning");
            if opaque.get("reasoning_details").is_none() {
                opaque["reasoning_details"] = json!([]);
            }
            let assembled = opaque["reasoning_details"]
                .as_array_mut()
                .ok_or_else(invalid)?;
            for detail in details {
                let fields = detail.as_object().ok_or_else(invalid)?;
                let payload = match detail["type"].as_str() {
                    Some("reasoning.text") => Some("text"),
                    Some("reasoning.summary") => Some("summary"),
                    _ => None,
                };
                // Indices are optional and can be reused across logical blocks.
                // Only consecutive compatible fragments merge; a signature seals
                // its text block. Never search backwards across sealed/opaque data.
                if let Some(last) = assembled.last()
                    && payload == Some("text")
                    && last["type"] == detail["type"]
                    && detail["text"].as_str().is_none_or(str::is_empty)
                    && detail["signature"].as_str().is_some_and(|s| !s.is_empty())
                    && fields
                        .iter()
                        .all(|(k, v)| v.is_null() || last.get(k) == Some(v))
                {
                    continue;
                }
                let previous = assembled.last_mut().filter(|last| {
                    last["type"] == detail["type"]
                        && (payload != Some("text")
                            || last["signature"].as_str().is_none_or(str::is_empty))
                        && ["id", "format"].iter().all(|key| {
                            last.get(*key)
                                .filter(|v| !v.is_null())
                                .zip(detail.get(*key).filter(|v| !v.is_null()))
                                .is_none_or(|(a, b)| a == b)
                        })
                });
                if let (Some(payload), Some(previous)) = (payload, previous) {
                    for (key, value) in fields {
                        if value.is_null() || key == "index" {
                            continue;
                        }
                        if key == payload {
                            let fragment = value.as_str().ok_or_else(invalid)?;
                            let old = previous
                                .get(key)
                                .filter(|v| !v.is_null())
                                .map(|v| v.as_str().ok_or_else(invalid))
                                .transpose()?
                                .unwrap_or("");
                            previous[key] = json!(format!("{old}{fragment}"));
                        } else if previous
                            .get(key)
                            .is_some_and(|old| !old.is_null() && old != value)
                        {
                            return Err(invalid());
                        } else {
                            previous[key] = value.clone();
                        }
                    }
                } else {
                    // Encrypted/unknown opaque types remain separate and intact.
                    assembled.push(detail.clone());
                }
            }
        }
    }
    if let Some(reasoning) = delta.get("reasoning").filter(|v| !v.is_null()) {
        let text = reasoning.as_str().ok_or_else(invalid)?;
        if !text.is_empty() && opaque.get("reasoning_details").is_none() {
            let old = opaque
                .get("reasoning")
                .and_then(Value::as_str)
                .unwrap_or("");
            let full = format!("{old}{text}");
            if !opaque.is_object() {
                *opaque = json!({});
            }
            opaque["reasoning"] = json!(full);
        }
    }
    Ok(())
}
