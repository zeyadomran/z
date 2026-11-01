use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) struct Stream {
    buffer: Vec<u8>,
    data: Vec<u8>,
    done: BTreeMap<usize, Value>,
    added: BTreeMap<usize, (String, String)>,
    model: String,
    account: String,
    limit: usize,
    progress: watch::Sender<Progress>,
    response_id: Option<String>,
    text: String,
}
impl Stream {
    pub fn new(
        model: String,
        account: String,
        limit: usize,
        progress: watch::Sender<Progress>,
    ) -> Self {
        Self {
            buffer: vec![],
            data: vec![],
            done: BTreeMap::new(),
            added: BTreeMap::new(),
            model,
            account,
            limit,
            progress,
            response_id: None,
            text: String::new(),
        }
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Option<ProviderResponse>> {
        self.buffer.extend_from_slice(chunk);
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
                    let v: Value = serde_json::from_slice(&data).map_err(|_| protocol())?;
                    if let Some(out) = self.event(v)? {
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
            }
            if self.data.len() > self.limit.saturating_mul(2).max(65536) {
                return Ok(Some(ProviderResponse::stopped(StopReason::OutputLimit)));
            }
        }
        if self.buffer.len() > self.limit.saturating_mul(2).max(65536) {
            return Ok(Some(ProviderResponse::stopped(StopReason::OutputLimit)));
        }
        Ok(None)
    }
    fn event(&mut self, v: Value) -> Result<Option<ProviderResponse>> {
        let kind = v["type"].as_str().ok_or_else(protocol)?;
        if let Some(response) = v.get("response") {
            if let Some(id) = response.get("id") {
                let id = id
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or_else(protocol)?;
                if self.response_id.as_deref().is_some_and(|old| old != id) {
                    return Err(protocol());
                }
                self.response_id = Some(id.into());
                self.progress
                    .send_modify(|p| p.generation_id = Some(id.into()));
            }
            if let Some(model) = response.get("model") {
                if model != &self.model {
                    return Err(protocol());
                }
                self.progress
                    .send_modify(|p| p.model = Some(self.model.clone()));
            }
            if let Some(usage) = response.get("usage").filter(|u| !u.is_null()) {
                if !usage.is_object() {
                    return Err(protocol());
                }
                self.progress.send_modify(|p| p.usage = Some(usage.clone()));
            }
        }
        match kind {
            "response.output_item.added" | "response.output_item.done" => {
                let index = v["output_index"]
                    .as_u64()
                    .filter(|i| *i < 128)
                    .ok_or_else(protocol)? as usize;
                let item = &v["item"];
                let id = item["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or_else(protocol)?;
                let ty = item["type"].as_str().ok_or_else(protocol)?;
                if !matches!(ty, "reasoning" | "message" | "function_call") {
                    return Err(protocol());
                }
                if self
                    .added
                    .get(&index)
                    .is_some_and(|(old, t)| old != id || t != ty)
                {
                    return Err(protocol());
                }
                if kind.ends_with(".added") {
                    if self.added.insert(index, (id.into(), ty.into())).is_some()
                        || self.done.contains_key(&index)
                    {
                        return Err(protocol());
                    }
                } else if self.done.insert(index, item.clone()).is_some() {
                    return Err(protocol());
                }
            }
            "response.output_text.delta" => {
                self.text
                    .push_str(v["delta"].as_str().ok_or_else(protocol)?);
                self.progress.send_modify(|p| p.text = self.text.clone());
            }
            "response.completed" => return self.complete(&v["response"]).map(Some),
            "response.failed" | "error" => {
                let error = if kind == "error" {
                    if v.get("error").is_some() {
                        v.clone()
                    } else {
                        json!({"error":v})
                    }
                } else {
                    v["response"].clone()
                };
                errors::diagnostic(&self.progress, 200, &error, None, "stream");
                return Ok(Some(ProviderResponse::stopped(errors::reason(200, &error))));
            }
            "response.incomplete" => {
                return Ok(Some(ProviderResponse::stopped(
                    if v["response"]["incomplete_details"]["reason"] == "max_output_tokens" {
                        StopReason::OutputLimit
                    } else {
                        StopReason::Protocol
                    },
                )));
            }
            "response.created"
            | "response.in_progress"
            | "response.queued"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.output_text.done"
            | "response.output_text.annotation.added"
            | "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
            | "response.refusal.delta"
            | "response.refusal.done" => {}
            _ => return Err(protocol()),
        }
        let size = self.text.len()
            + self
                .done
                .values()
                .map(|v| v.to_string().len())
                .sum::<usize>();
        self.progress.send_modify(|p| p.bytes = size);
        if size > self.limit {
            return Ok(Some(ProviderResponse::stopped(StopReason::OutputLimit)));
        }
        Ok(None)
    }
    fn complete(&mut self, r: &Value) -> Result<ProviderResponse> {
        if r["status"] != "completed"
            || r["model"] != self.model
            || r["id"].as_str() != self.response_id.as_deref()
            || self.response_id.is_none()
        {
            return Err(protocol());
        }
        let mut output = r["output"].as_array().ok_or_else(protocol)?.clone();
        if self
            .done
            .keys()
            .chain(self.added.keys())
            .any(|i| *i >= output.len())
        {
            return Err(protocol());
        }
        let mut ids = BTreeSet::new();
        let mut calls = BTreeSet::new();
        let mut message = AssistantMessage::default();
        let mut refused = false;
        for (i, item) in output.iter_mut().enumerate() {
            let id = item["id"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256)
                .ok_or_else(protocol)?
                .to_owned();
            if !ids.insert(id.clone()) {
                return Err(protocol());
            }
            let ty = item["type"].as_str().ok_or_else(protocol)?.to_owned();
            if self
                .added
                .get(&i)
                .is_some_and(|(old, t)| old != &id || t != &ty)
            {
                return Err(protocol());
            }
            if let Some(done) = self.done.get(&i) {
                if done["id"] != id || done["type"] != ty {
                    return Err(protocol());
                }
                if ty == "reasoning" {
                    let opaque = done["encrypted_content"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or_else(protocol)?;
                    if item
                        .get("encrypted_content")
                        .filter(|v| !v.is_null())
                        .is_some_and(|v| v != opaque)
                    {
                        return Err(protocol());
                    }
                    // All other completed reasoning fields must agree as well.
                    let mut terminal = item.clone();
                    terminal["encrypted_content"] = json!(opaque);
                    if &terminal != done {
                        return Err(protocol());
                    }
                    *item = done.clone();
                } else if done != item {
                    return Err(protocol());
                }
            } else if ty == "reasoning" {
                return Err(protocol());
            }
            match ty.as_str() {
                "reasoning" => {}
                "function_call" => {
                    if item.get("status").is_some_and(|s| s != "completed")
                        || item.get("namespace").is_some_and(|n| n != "workspace")
                    {
                        return Err(protocol());
                    }
                    let call = item["call_id"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 256)
                        .ok_or_else(protocol)?;
                    if !calls.insert(call.to_owned()) {
                        return Err(protocol());
                    }
                    let name = item["name"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 128)
                        .ok_or_else(protocol)?;
                    let arguments = item["arguments"]
                        .as_str()
                        .filter(|s| s.len() <= 256 * 1024)
                        .ok_or_else(protocol)?;
                    message.calls.push(ToolCall {
                        id: call.into(),
                        name: name.into(),
                        arguments: arguments.into(),
                    });
                }
                "message" => {
                    if item["role"] != "assistant" || item["status"] != "completed" {
                        return Err(protocol());
                    }
                    for part in item["content"].as_array().ok_or_else(protocol)? {
                        match part["type"].as_str() {
                            Some("output_text") => message
                                .text
                                .push_str(part["text"].as_str().ok_or_else(protocol)?),
                            Some("refusal") => refused = true,
                            _ => return Err(protocol()),
                        }
                    }
                }
                _ => return Err(protocol()),
            }
        }
        let size = serde_json::to_vec(&output)?.len();
        self.progress.send_modify(|p| {
            p.bytes = p.bytes.max(size);
            p.text = message.text.clone();
        });
        if size > self.limit {
            return Ok(ProviderResponse::stopped(StopReason::OutputLimit));
        }
        if refused {
            return Ok(ProviderResponse::stopped(StopReason::Refused));
        }
        if message.calls.is_empty() && message.text.is_empty() {
            return Err(protocol());
        }
        message.opaque =
            json!({"format":"chatgpt-responses-v1","account":self.account,"output":output});
        Ok(ProviderResponse {
            reason: if message.calls.is_empty() {
                StopReason::Final
            } else {
                StopReason::Tools
            },
            message: Some(message),
            retry_after: None,
        })
    }
}
