mod config;
mod context;
mod domain;
mod engine;
mod journal;
mod provider;
mod records;
mod store;
mod turn_loop;
mod workspace;
mod workspace_lock;

pub use config::{Config, Policy};
pub use domain::*;
pub use engine::{Engine, TurnHandle};
pub use journal::UsageRecord;
pub use provider::*;
pub use workspace::Instruction;

#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("unsupported or unrecognized database schema")]
    Schema,
    #[error("workspace or session has unresolved work")]
    Busy,
    #[error("session binding does not match this engine")]
    Binding,
    #[error("record not found")]
    NotFound,
    #[error("provider protocol error: {0}")]
    Protocol(String),
    #[error("persistence failed: {0}; no completion was acknowledged")]
    Persistence(String),
    #[error("engine is shut down")]
    Shutdown,
    #[error("engine worker stopped unexpectedly")]
    Worker,
    #[error("scoped inspection failed: {0}")]
    Scope(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        match value.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Persistence(value.to_string()),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Persistence(value.to_string())
    }
}
