use super::*;
use crate::Error;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tokio::sync::oneshot;
#[derive(Debug)]
pub enum ScriptFrame {
    Text(String),
    Complete,
    Refused,
    ToolCall(String),
    Disconnect,
    ExpectHistory(Vec<ProtocolMessage>),
    Opaque(Value),
    Wait {
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}
#[derive(Debug)]
pub struct ScriptStep {
    pub expected_prompt: String,
    pub frames: Vec<ScriptFrame>,
}
impl ScriptStep {
    pub fn reply(prompt: impl Into<String>, reply: impl Into<String>) -> Self {
        Self {
            expected_prompt: prompt.into(),
            frames: vec![ScriptFrame::Text(reply.into()), ScriptFrame::Complete],
        }
    }
}
pub struct ScriptedProvider {
    steps: VecDeque<ScriptStep>,
    epochs: BTreeMap<i64, Epoch>,
}
impl ScriptedProvider {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        Self {
            steps: steps.into(),
            epochs: BTreeMap::new(),
        }
    }
    pub fn validate_request(&mut self, request: &ProviderRequest) -> Result<()> {
        validate(request)?;
        if self
            .epochs
            .get(&request.epoch.id.0)
            .is_some_and(|e| e != &request.epoch)
        {
            return Err(invalid());
        }
        self.epochs
            .insert(request.epoch.id.0, request.epoch.clone());
        Ok(())
    }
}
fn invalid() -> Error {
    Error::Protocol("invalid history or changed epoch input".into())
}
pub(super) fn validate(request: &ProviderRequest) -> Result<()> {
    if request.epoch.id.0 <= 0 || request.epoch.prefix.is_empty() || request.messages.is_empty() {
        return Err(invalid());
    }
    if !serde_json::from_str::<Value>(&request.epoch.tool_schema)
        .map_err(|_| invalid())?
        .is_array()
    {
        return Err(invalid());
    }
    let mut pending = BTreeSet::new();
    let mut all = BTreeSet::new();
    for m in &request.messages {
        let calls = match m {
            ProtocolMessage::User(_) if pending.is_empty() => continue,
            ProtocolMessage::Assistant { calls, .. } if pending.is_empty() => {
                calls.iter().collect::<Vec<_>>()
            }
            ProtocolMessage::AssistantData(m) if pending.is_empty() => {
                m.calls.iter().map(|c| &c.id).collect()
            }
            ProtocolMessage::ToolResult { call_id, .. } if pending.remove(call_id) => continue,
            _ => return Err(invalid()),
        };
        for id in calls {
            if id.is_empty() || !all.insert(id) {
                return Err(invalid());
            }
            pending.insert(id);
        }
    }
    if !pending.is_empty() {
        return Err(invalid());
    }
    Ok(())
}
impl Provider for ScriptedProvider {
    fn binding(&self) -> ProviderBinding {
        ProviderBinding::scripted()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_tokens: 128_000,
            output_tokens: 4096,
        }
    }
    fn respond<'a>(
        &'a mut self,
        request: ProviderRequest,
        limit: usize,
        progress: watch::Sender<Progress>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            self.validate_request(&request)?;
            let step = self.steps.pop_front().ok_or_else(invalid)?;
            let prompt = request.messages.iter().rev().find_map(|m| {
                if let ProtocolMessage::User(t) = m {
                    Some(t)
                } else {
                    None
                }
            });
            if prompt != Some(&step.expected_prompt) {
                return Err(invalid());
            }
            let mut message = AssistantMessage::default();
            let mut terminal = None;
            for frame in step.frames {
                if terminal.is_some() {
                    return Err(invalid());
                }
                match frame {
                    ScriptFrame::Text(delta) => {
                        if message.text.len().saturating_add(delta.len()) > limit {
                            return Ok(ProviderResponse::stopped(StopReason::OutputLimit));
                        }
                        message.text.push_str(&delta);
                        progress.send_modify(|p| {
                            p.text = message.text.clone();
                            p.bytes = p.text.len();
                        });
                    }
                    ScriptFrame::ToolCall(json) => match serde_json::from_str::<ToolCall>(&json) {
                        Ok(call) => message.calls.push(call),
                        Err(_) => {
                            return Ok(ProviderResponse::stopped(StopReason::ToolsUnavailable));
                        }
                    },
                    ScriptFrame::Opaque(value) => message.opaque = value,
                    ScriptFrame::ExpectHistory(history) => {
                        if request.messages != history {
                            return Err(invalid());
                        }
                    }
                    ScriptFrame::Complete => {
                        terminal = Some(if message.calls.is_empty() {
                            StopReason::Final
                        } else {
                            StopReason::Tools
                        })
                    }
                    ScriptFrame::Refused => terminal = Some(StopReason::Refused),
                    ScriptFrame::Disconnect => {
                        return Ok(ProviderResponse::stopped(StopReason::Transport));
                    }
                    ScriptFrame::Wait { entered, release } => {
                        let _ = entered.send(());
                        release.await.map_err(|_| invalid())?;
                    }
                }
            }
            let reason = terminal.ok_or_else(invalid)?;
            let complete = matches!(reason, StopReason::Final | StopReason::Tools);
            Ok(ProviderResponse {
                reason,
                message: complete.then_some(message),
                retry_after: None,
            })
        })
    }
}
