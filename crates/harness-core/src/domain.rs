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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedWork {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub data_dir: std::path::PathBuf,
}

impl std::fmt::Display for UnresolvedWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unresolved workspace work: use original --data-dir \"{}\"; resume with --session {} --resume-turn {}, or use harness abandon --session {} --turn {} with the same workspace and data directory",
            self.data_dir.display(),
            self.session_id.0,
            self.turn_id.0,
            self.session_id.0,
            self.turn_id.0
        )
    }
}

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
    ApprovalRequired,
    RecoveryRequired,
    Final,
    Cancelled,
    Deadline,
    OutputLimit,
    Refused,
    Protocol,
    Transport,
    ToolsUnavailable,
    Tools,
    ContextLimit,
    RequestLimit,
    ToolLimit,
    Authentication,
    Credits,
    Throttled,
    Unavailable,
    NoUpstream,
    InvalidRequest,
    RetryableTransport,
    Interrupted,
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
    ApprovalRequested {
        turn_id: TurnId,
        call_id: String,
    },
    ApprovalResolved {
        turn_id: TurnId,
        call_id: String,
        approval_id: String,
        approved: bool,
    },
    TurnStarted {
        turn_id: TurnId,
    },
    AssistantCompleted {
        turn_id: TurnId,
    },
    TurnStopped {
        outcome: TurnOutcome,
    },
    AttemptStarted {
        turn_id: TurnId,
        attempt: i64,
    },
    AttemptDiscarded {
        turn_id: TurnId,
    },
    ToolProposed {
        turn_id: TurnId,
        call_id: String,
    },
    ToolFinished {
        turn_id: TurnId,
        call_id: String,
    },
    BatchClosed {
        turn_id: TurnId,
    },
    ActiveTimeRecovered {
        turn_id: TurnId,
        /// Conservative reserve since the last durable active checkpoint.
        uncertain_ms: i64,
    },
    ActiveTimeCheckpoint {
        turn_id: TurnId,
        active_ms: i64,
    },
    Usage {
        turn_id: TurnId,
        raw: Option<serde_json::Value>,
    },
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
    Tool,
}
