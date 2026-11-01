use crate::{ContextEpochId, Error, Result, StopReason};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tokio::sync::oneshot;

/// Exact frozen input for a local protocol fixture, not a live wire adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Epoch {
    pub id: ContextEpochId,
    pub prefix: String,
    pub tool_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolMessage {
    User(String),
    Assistant { text: String, calls: Vec<String> },
    ToolResult { call_id: String, result: String },
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub epoch: Epoch,
    pub messages: Vec<ProtocolMessage>,
}

#[derive(Debug)]
pub enum ScriptFrame {
    Text(String),
    Complete,
    Refused,
    ToolCall(String),
    Disconnect,
    /// Explicit external synchronization for cancellation/deadline fixtures.
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

pub(crate) struct Response {
    pub reason: StopReason,
    pub text: Option<String>,
}

impl ScriptedProvider {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        Self {
            steps: steps.into(),
            epochs: BTreeMap::new(),
        }
    }

    /// Validate closed tool batches and exact epoch continuity before consuming a script.
    pub fn validate_request(&mut self, request: &ProviderRequest) -> Result<()> {
        let invalid = || Error::Protocol("invalid history or changed epoch input".into());
        if request.epoch.id.0 <= 0 || request.epoch.prefix.is_empty() {
            return Err(invalid());
        }
        let schemas: serde_json::Value =
            serde_json::from_str(&request.epoch.tool_schema).map_err(|_| invalid())?;
        if !schemas.is_array() {
            return Err(invalid());
        }
        if let Some(epoch) = self.epochs.get(&request.epoch.id.0)
            && epoch != &request.epoch
        {
            return Err(invalid());
        }
        let mut pending = BTreeSet::new();
        let mut all_calls = BTreeSet::new();
        if request.messages.is_empty() {
            return Err(invalid());
        }
        for message in &request.messages {
            match message {
                ProtocolMessage::User(_) if pending.is_empty() => {}
                ProtocolMessage::Assistant { calls, .. } if pending.is_empty() => {
                    for call in calls {
                        if call.is_empty() || !all_calls.insert(call) {
                            return Err(invalid());
                        }
                        pending.insert(call);
                    }
                }
                ProtocolMessage::ToolResult { call_id, .. } if pending.remove(call_id) => {}
                _ => return Err(invalid()),
            }
        }
        if !pending.is_empty() {
            return Err(invalid());
        }
        self.epochs
            .insert(request.epoch.id.0, request.epoch.clone());
        Ok(())
    }

    pub(crate) async fn respond(
        &mut self,
        request: ProviderRequest,
        output_limit: usize,
    ) -> Result<Response> {
        self.validate_request(&request)?;
        let step = self
            .steps
            .pop_front()
            .ok_or_else(|| Error::Protocol("script exhausted".into()))?;
        if request.messages.last() != Some(&ProtocolMessage::User(step.expected_prompt)) {
            return Err(Error::Protocol("script prompt mismatch".into()));
        }
        let mut text = String::new();
        let mut terminal = None;
        for frame in step.frames {
            if terminal.is_some() {
                return Err(Error::Protocol("content after terminal frame".into()));
            }
            match frame {
                ScriptFrame::Text(delta) => {
                    if text.len().saturating_add(delta.len()) > output_limit {
                        return Ok(Response {
                            reason: StopReason::OutputLimit,
                            text: None,
                        });
                    }
                    text.push_str(&delta);
                }
                ScriptFrame::Complete => terminal = Some(StopReason::Final),
                ScriptFrame::Refused => terminal = Some(StopReason::Refused),
                ScriptFrame::ToolCall(_) => {
                    return Ok(Response {
                        reason: StopReason::ToolsUnavailable,
                        text: None,
                    });
                }
                ScriptFrame::Disconnect => {
                    return Ok(Response {
                        reason: StopReason::Transport,
                        text: None,
                    });
                }
                ScriptFrame::Wait { entered, release } => {
                    let _ = entered.send(());
                    release
                        .await
                        .map_err(|_| Error::Protocol("script gate disconnected".into()))?;
                }
            }
        }
        let reason = terminal.ok_or_else(|| Error::Protocol("missing terminal frame".into()))?;
        Ok(Response {
            text: (reason == StopReason::Final).then_some(text),
            reason,
        })
    }
}
