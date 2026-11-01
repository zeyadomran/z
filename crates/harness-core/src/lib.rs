mod config;
mod domain;
mod engine;
mod provider;
mod records;
mod store;

pub use config::Config;
pub use domain::*;
pub use engine::{Engine, TurnHandle};
pub use provider::{
    Epoch, ProtocolMessage, ProviderRequest, ScriptFrame, ScriptStep, ScriptedProvider,
};

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
