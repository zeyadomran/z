use serde::{Deserialize, Serialize};

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub struct $name(pub i64);
    };
}
id!(SessionId);
id!(TurnId);
id!(ContextEpochId);
id!(EventSequence);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    Running,
    AwaitingApproval,
    SuspendedLimit,
    SuspendedRecovery,
    Interrupted,
    Completed,
    Failed,
    Refused,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Final,
    Cancelled,
    Deadline,
    OutputLimit,
    Refused,
    Protocol,
    Transport,
    ToolsUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnOutcome {
    pub turn_id: TurnId,
    pub state: TurnState,
    pub reason: StopReason,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    TurnStarted { turn_id: TurnId },
    AssistantCompleted { turn_id: TurnId },
    TurnStopped { outcome: TurnOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableEvent {
    pub sequence: EventSequence,
    pub event: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}
