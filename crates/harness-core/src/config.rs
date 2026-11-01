use crate::{Error, Result};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace: PathBuf,
    pub data_dir: PathBuf,
    pub turn_timeout: Duration,
    pub max_prompt_bytes: usize,
    pub max_output_bytes: usize,
}

impl Config {
    pub fn new(workspace: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            workspace,
            data_dir,
            turn_timeout: Duration::from_secs(1200),
            max_prompt_bytes: 64 * 1024,
            max_output_bytes: 256 * 1024,
        }
    }

    pub(crate) fn validate(mut self) -> Result<Self> {
        if self.turn_timeout.is_zero()
            || self.turn_timeout > Duration::from_secs(86400)
            || !(1..=1024 * 1024).contains(&self.max_prompt_bytes)
            || !(1..=16 * 1024 * 1024).contains(&self.max_output_bytes)
        {
            return Err(Error::Configuration(
                "limits must be positive and bounded (24h, 1 MiB prompt, 16 MiB output)".into(),
            ));
        }
        self.workspace = std::fs::canonicalize(&self.workspace).map_err(|_| {
            Error::Configuration("workspace must be an existing local directory".into())
        })?;
        if !self.workspace.is_dir() || network_path(&self.workspace) {
            return Err(Error::Configuration(
                "workspace must be a local directory".into(),
            ));
        }
        if !self.data_dir.is_absolute() || network_path(&self.data_dir) {
            return Err(Error::Configuration(
                "data directory must be an absolute local path".into(),
            ));
        }
        // Resolve an existing ancestor before creation, including junction aliases.
        let mut ancestor = self.data_dir.as_path();
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or_else(|| Error::Configuration("invalid data directory".into()))?;
        }
        let resolved = std::fs::canonicalize(ancestor)
            .map_err(|_| Error::Configuration("cannot resolve data directory".into()))?;
        if resolved.starts_with(&self.workspace) || network_path(&resolved) {
            return Err(Error::Configuration(
                "data directory must be outside the workspace".into(),
            ));
        }
        std::fs::create_dir_all(&self.data_dir)
            .map_err(|_| Error::Configuration("cannot create data directory".into()))?;
        self.data_dir = std::fs::canonicalize(&self.data_dir)
            .map_err(|_| Error::Configuration("cannot resolve data directory".into()))?;
        if self.data_dir.starts_with(&self.workspace) || network_path(&self.data_dir) {
            return Err(Error::Configuration(
                "data directory must be outside the workspace".into(),
            ));
        }
        Ok(self)
    }

    pub(crate) fn workspace_identity(&self) -> Result<String> {
        let value = self
            .workspace
            .to_str()
            .ok_or_else(|| Error::Configuration("workspace must be UTF-8".into()))?;
        #[cfg(windows)]
        let value = value.to_lowercase();
        Ok(value.to_string())
    }

    pub(crate) fn limits_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&serde_json::json!({
            "turn_timeout_ms": self.turn_timeout.as_millis(),
            "max_prompt_bytes": self.max_prompt_bytes,
            "max_output_bytes": self.max_output_bytes,
            "max_model_attempts": 1,
            "max_tool_calls": 0
        }))?)
    }
}

fn network_path(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\").to_uppercase();
    text.starts_with("\\\\?\\UNC\\") || (text.starts_with("\\\\") && !text.starts_with("\\\\?\\"))
}
