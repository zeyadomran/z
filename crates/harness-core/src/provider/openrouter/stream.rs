use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) struct Stream {
    buffer: Vec<u8>,
    data: Vec<u8>,
    calls: BTreeMap<usize, ToolCall>,
    message: AssistantMessage,
    finish: Option<String>,
    refused: bool,
    bytes: usize,
    limit: usize,
    model: String,
    canonical: String,
    routes: BTreeMap<String, String>,
    progress: watch::Sender<Progress>,
}
fn protocol() -> Error {
    Error::Protocol("invalid OpenRouter stream; no partial calls executed".into())
}
impl Stream {
    pub fn new(
        limit: usize,
        model: String,
        canonical: String,
        routes: BTreeMap<String, String>,
        progress: watch::Sender<Progress>,
    ) -> Self {
        Self {
            buffer: vec![],
            data: vec![],
            calls: BTreeMap::new(),
            message: AssistantMessage::default(),
            finish: None,
            refused: false,
            bytes: 0,
            limit,
            model,
            canonical,
            routes,
            progress,
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Option<ProviderResponse>> {
        // Stream envelopes are not model payload. Memory is bounded by the
        // frame/buffer caps below; the engine deadline bounds total consumption.
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n' || *b == b'\r') {
            if self.buffer[end] == b'\r' && end + 1 == self.buffer.len() {
                break;
            }
            let width = if self.buffer[end] == b'\r' && self.buffer.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            let line = self.buffer[..end].to_vec();
            self.buffer.drain(..end + width);
            if line.is_empty() {
                if !self.data.is_empty() {
                    self.data.pop();
                    let data = std::mem::take(&mut self.data);
                    if let Some(out) = self.event(&data)? {
                        if out.message.is_some() && !self.buffer.iter().all(u8::is_ascii_whitespace)
                        {
                            return Err(protocol());
                        }
                        return Ok(Some(out));
                    }
                }
            } else if let Some(data) = line.strip_prefix(b"data:") {
                self.data
                    .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                self.data.push(b'\n');
            } else if line == b"data" {
                self.data.push(b'\n');
            }
            // SSE comments, id/retry/event fields carry no executable content.
            if self.data.len() > 512 * 1024 {
                return Err(protocol());
            }
        }
        if self.buffer.len() > 512 * 1024 {
            return Err(protocol());
        }
        Ok(None)
    }
    fn event(&mut self, data: &[u8]) -> Result<Option<ProviderResponse>> {
        if data == b"[DONE]" {
            return self.complete().map(Some);
        }
        let v: Value = serde_json::from_slice(data).map_err(|_| protocol())?;
        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            if !usage.is_object() {
                return Err(protocol());
            }
            self.progress.send_modify(|p| p.usage = Some(usage.clone()));
        }
        if v.get("error").is_some_and(|e| !e.is_null()) {
            return Ok(Some(ProviderResponse::stopped(error_reason(
                v["error"]["code"].as_u64().unwrap_or(0) as u16,
                &v,
            ))));
        }
        for (key, target) in [("id", 0), ("model", 1), ("provider", 2)] {
            if let Some(value) = v.get(key).and_then(Value::as_str) {
                if value.len() > 256 {
                    return Err(protocol());
                }
                if target == 1 && value != self.model && value != self.canonical {
                    return Err(protocol());
                }
                if target == 2 && !self.routes.values().any(|r| r == value) {
                    return Err(protocol());
                }
                let mut conflict = false;
                self.progress.send_modify(|p| {
                    let slot = match target {
                        0 => &mut p.generation_id,
                        1 => &mut p.model,
                        _ => &mut p.upstream,
                    };
                    if slot.as_deref().is_some_and(|old| old != value) {
                        conflict = true;
                    } else {
                        *slot = Some(value.into());
                    }
                });
                if conflict {
                    return Err(protocol());
                }
            }
        }
        let choices = v["choices"].as_array().ok_or_else(protocol)?;
        if choices.is_empty() {
            return Ok(None);
        }
        if choices.len() != 1 || choices[0]["index"] != 0 {
            return Err(protocol());
        }
        let choice = &choices[0];
        let delta = choice["delta"].as_object().ok_or_else(protocol)?;
        if delta.keys().any(|key| {
            !matches!(
                key.as_str(),
                "role"
                    | "content"
                    | "tool_calls"
                    | "reasoning"
                    | "reasoning_details"
                    | "refusal"
                    | "annotations"
            )
        }) {
            return Err(protocol());
        }
        if let Some(role) = delta.get("role").filter(|v| !v.is_null())
            && role != "assistant"
        {
            return Err(protocol());
        }
        let content = delta
            .get("content")
            .filter(|v| !v.is_null())
            .map(|v| v.as_str().ok_or_else(protocol))
            .transpose()?
            .unwrap_or("");
        let has_payload = !content.is_empty()
            || ["tool_calls", "reasoning_details", "reasoning", "refusal"]
                .iter()
                .any(|k| {
                    delta.get(*k).is_some_and(|v| {
                        !v.is_null() && v != "" && !v.as_array().is_some_and(Vec::is_empty)
                    })
                });
        if self.finish.is_some() && has_payload {
            return Err(protocol());
        }
        self.message.text.push_str(content);
        if let Some(refusal) = delta.get("refusal").filter(|v| !v.is_null()) {
            if !refusal.is_string() {
                return Err(protocol());
            }
            self.refused |= refusal.as_str().is_some_and(|s| !s.is_empty());
        }
        super::reasoning::append(&mut self.message.opaque, delta)?;
        if let Some(calls) = delta.get("tool_calls").filter(|v| !v.is_null()) {
            for call in calls.as_array().ok_or_else(protocol)? {
                let index = call["index"]
                    .as_u64()
                    .filter(|n| *n < 60)
                    .ok_or_else(protocol)? as usize;
                if let Some(kind) = call.get("type").filter(|v| !v.is_null())
                    && kind != "function"
                {
                    return Err(protocol());
                }
                if call
                    .as_object()
                    .ok_or_else(protocol)?
                    .keys()
                    .any(|k| !matches!(k.as_str(), "index" | "id" | "type" | "function"))
                {
                    return Err(protocol());
                }
                let assembled = self.calls.entry(index).or_insert(ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: String::new(),
                });
                let sealed = serde_json::from_str::<Value>(&assembled.arguments).is_ok();
                for (value, target) in [
                    (call.get("id"), &mut assembled.id),
                    (
                        call.get("function").and_then(|f| f.get("name")),
                        &mut assembled.name,
                    ),
                ] {
                    if let Some(value) = value.filter(|v| !v.is_null()) {
                        let fragment = value.as_str().ok_or_else(protocol)?;
                        if fragment.is_empty() || fragment == target {
                            continue;
                        }
                        if sealed && !target.is_empty() {
                            return Err(protocol());
                        }
                        if !target.is_empty() && fragment.starts_with(target.as_str()) {
                            *target = fragment.into();
                        } else {
                            target.push_str(fragment);
                        }
                    }
                }
                if let Some(value) = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .filter(|v| !v.is_null())
                {
                    assembled
                        .arguments
                        .push_str(value.as_str().ok_or_else(protocol)?);
                }
                if assembled.id.len() > 256
                    || assembled.name.len() > 128
                    || assembled.arguments.len() > 256 * 1024
                {
                    return Err(protocol());
                }
            }
        }
        // Budget the assembled payload, not repeated per-chunk envelopes or the
        // compatibility reasoning string duplicated by reasoning_details.
        self.bytes = self.message.text.len()
            + self
                .calls
                .values()
                .map(|c| c.id.len() + c.name.len() + c.arguments.len())
                .sum::<usize>()
            + if self.message.opaque.is_object() {
                serde_json::to_vec(&self.message.opaque)?.len()
            } else {
                0
            };
        self.progress.send_modify(|p| {
            p.text = self.message.text.clone();
            p.bytes = self.bytes;
        });
        if self.bytes > self.limit {
            return Ok(Some(ProviderResponse::stopped(StopReason::OutputLimit)));
        }
        if let Some(finish) = choice.get("finish_reason").filter(|v| !v.is_null()) {
            let finish = finish.as_str().ok_or_else(protocol)?;
            if let Some(old) = &self.finish {
                if old != finish || v.get("usage").is_none() {
                    return Err(protocol());
                }
            } else {
                self.finish = Some(finish.into());
            }
        }
        Ok(None)
    }
    fn complete(&mut self) -> Result<ProviderResponse> {
        let progress = self.progress.borrow().clone();
        if progress.generation_id.is_none() || progress.model.is_none() {
            return Err(protocol());
        }
        let reason = match self.finish.as_deref() {
            Some("stop") if self.calls.is_empty() => {
                if self.refused {
                    StopReason::Refused
                } else {
                    StopReason::Final
                }
            }
            Some("tool_calls") if !self.calls.is_empty() && !self.refused => StopReason::Tools,
            Some("length") => StopReason::OutputLimit,
            Some("content_filter") => StopReason::Refused,
            Some("error") => StopReason::Protocol,
            _ => return Err(protocol()),
        };
        if !matches!(reason, StopReason::Final | StopReason::Tools) {
            return Ok(ProviderResponse::stopped(reason));
        }
        let mut ids = BTreeSet::new();
        for (expected, (index, call)) in self.calls.iter().enumerate() {
            if expected != *index
                || call.id.is_empty()
                || call.name.is_empty()
                || !ids.insert(&call.id)
            {
                return Err(protocol());
            }
        }
        // An empty argument string denotes no supplied parameters. Malformed
        // nonempty JSON remains raw for a structured, repairable workspace error.
        for call in self.calls.values_mut() {
            if call.arguments.is_empty() {
                call.arguments = "{}".into();
                self.bytes += 2;
            }
        }
        if self.bytes > self.limit {
            return Ok(ProviderResponse::stopped(StopReason::OutputLimit));
        }
        self.progress.send_modify(|p| p.bytes = self.bytes);
        self.message.calls = std::mem::take(&mut self.calls).into_values().collect();
        if self.message.opaque.is_object() {
            let route = if let Some(upstream) = progress.upstream {
                self.routes
                    .iter()
                    .find(|(_, name)| **name == upstream)
                    .map(|(tag, _)| tag)
            } else if self.routes.len() == 1 {
                self.routes.keys().next()
            } else {
                None
            }
            .ok_or_else(protocol)?;
            self.message.opaque["route"] = json!(route);
        }
        Ok(ProviderResponse {
            reason,
            message: Some(std::mem::take(&mut self.message)),
            retry_after: None,
        })
    }
}
pub(super) fn error_reason(status: u16, value: &Value) -> StopReason {
    let typed = value["error"]["metadata"]["error_type"]
        .as_str()
        .or_else(|| value["error"]["code"].as_str());
    match typed {
        Some("not_found") if status == 404 => StopReason::NoUpstream,
        Some("context_length_exceeded" | "context_length" | "input_too_long") => {
            StopReason::ContextLimit
        }
        Some(
            "insufficient_quota"
            | "insufficient_credits"
            | "payment_required"
            | "token_limit_exceeded",
        ) => StopReason::Credits,
        Some("content_filter" | "content_policy_violation" | "refusal") => StopReason::Refused,
        Some("invalid_request" | "invalid_request_error") => StopReason::InvalidRequest,
        Some("rate_limit_exceeded") => StopReason::Throttled,
        Some("max_tokens_exceeded") => StopReason::OutputLimit,
        Some("authentication" | "permission_denied") => StopReason::Authentication,
        Some(
            "invalid_prompt"
            | "not_found"
            | "precondition_failed"
            | "payload_too_large"
            | "unprocessable"
            | "string_too_long",
        ) => StopReason::InvalidRequest,
        Some("timeout") => StopReason::RetryableTransport,
        Some(
            "server"
            | "server_error"
            | "overloaded"
            | "provider_overloaded"
            | "provider_unavailable",
        ) => StopReason::Unavailable,
        Some(_) => StopReason::Protocol,
        None => match status {
            401 | 403 => StopReason::Authentication,
            402 => StopReason::Credits,
            408 => StopReason::RetryableTransport,
            429 => StopReason::Throttled,
            500 | 502 | 504 => StopReason::Unavailable,
            404 | 503 => StopReason::NoUpstream,
            400 | 422 => StopReason::InvalidRequest,
            _ => StopReason::Protocol,
        },
    }
}
