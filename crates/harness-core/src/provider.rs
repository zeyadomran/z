use crate::{ContextEpochId, Result, StopReason};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};
use tokio::sync::watch;
mod scripted;
pub use scripted::{ScriptFrame, ScriptStep, ScriptedProvider};
pub mod chatgpt;
pub mod openrouter;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Epoch {
    pub id: ContextEpochId,
    pub prefix: String,
    pub tool_schema: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// Adapter-owned continuation payload, never translated by the engine.
    pub opaque: Value,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolMessage {
    User(String),
    Assistant { text: String, calls: Vec<String> },
    AssistantData(AssistantMessage),
    ToolResult { call_id: String, result: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub epoch: Epoch,
    pub messages: Vec<ProtocolMessage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBinding {
    pub provider: String,
    pub account_ref: String,
    pub model: String,
    pub options: String,
}
impl ProviderBinding {
    pub fn scripted() -> Self {
        Self {
            provider: "scripted".into(),
            account_ref: "local".into(),
            model: "script-v1".into(),
            options: "{}".into(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Capabilities {
    pub context_tokens: usize,
    pub output_tokens: usize,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    /// Engine-owned request identity, retained even when text is coalesced.
    #[serde(default)]
    pub attempt: usize,
    #[serde(default)]
    pub discarded_attempts: usize,
    /// Explicit host-selected provider output cap for this attempt, not identity.
    #[serde(default)]
    pub output_token_limit: Option<usize>,
    pub text: String,
    pub usage: Option<Value>,
    pub generation_id: Option<String>,
    pub model: Option<String>,
    pub upstream: Option<String>,
    pub bytes: usize,
    /// Sanitized provider evidence only, never response text or credentials.
    #[serde(default)]
    pub diagnostic: Option<Value>,
    /// Local model admission configuration; never an upstream output cap.
    #[serde(default)]
    pub context_budget: Option<Value>,
}
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub reason: StopReason,
    pub message: Option<AssistantMessage>,
    pub retry_after: Option<std::time::Duration>,
}
impl ProviderResponse {
    pub fn stopped(reason: StopReason) -> Self {
        Self {
            reason,
            message: None,
            retry_after: None,
        }
    }
}
/// Each adapter owns its wire schema and credentials. Core sees normalized tools
/// plus an opaque payload bound to the immutable provider/model/options identity.
pub trait Provider: Send + 'static {
    fn binding(&self) -> ProviderBinding;
    fn capabilities(&self) -> Capabilities;
    fn respond<'a>(
        &'a mut self,
        request: ProviderRequest,
        output_limit: usize,
        progress: watch::Sender<Progress>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;
}
impl Provider for Box<dyn Provider> {
    fn binding(&self) -> ProviderBinding {
        (**self).binding()
    }
    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
    fn respond<'a>(
        &'a mut self,
        r: ProviderRequest,
        n: usize,
        p: watch::Sender<Progress>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        (**self).respond(r, n, p)
    }
}
