//! Own public-client registration. Secrets never enter engine records or Debug.
mod catalog;
mod flow;
mod identity;
mod lifecycle;
mod store;
pub(crate) mod transport;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
use crate::{Error, Result};
pub use flow::Login;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use store::Vault;
use transport::Remote;
pub use transport::{Catalog, Model};

pub const PRODUCT_NAME: &str = "harness";
pub const ISSUER: &str = "https://auth.openai.com";
pub const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const RESOURCE: &str = "https://api.openai.com/v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub issuer: String,
    pub client_id: String,
    pub subject: String,
}
impl Identity {
    pub fn binding(&self) -> String {
        crate::workspace::hash(serde_json::to_string(self).expect("identity").as_bytes())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountStatus {
    pub profile: String,
    pub client_id: Option<String>,
    pub identity: Option<Identity>,
    pub state: String,
    pub direct_use: bool,
    pub expires_at: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    id_token: String,
    token_type: String,
    scope: String,
    received_at: u64,
    expires_at: u64,
    earliest_refresh_at: Value,
    nonce: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    accepted_logout: Option<String>,
    client_id: Option<String>,
    identity: Option<Identity>,
    generation: u64,
    state: String,
    tokens: Option<Tokens>,
}
impl Record {
    fn direct(&self) -> bool {
        self.tokens
            .as_ref()
            .is_some_and(|t| t.scope.split_whitespace().any(|s| s == DIRECT_SCOPE))
    }
    fn status(&self, profile: &str) -> AccountStatus {
        AccountStatus {
            profile: profile.into(),
            client_id: self.client_id.clone(),
            identity: self.identity.clone(),
            state: self.state.clone(),
            direct_use: self.direct(),
            expires_at: self.tokens.as_ref().map(|t| t.expires_at),
        }
    }
}
/// No endpoint overrides for live credentials. The fixture vault is separately
/// marked and cannot be opened by the production constructor (or vice versa).
#[derive(Clone)]
pub struct ChatGptAuth {
    vault: Vault,
    remote: Remote,
}
impl ChatGptAuth {
    pub fn local() -> Result<Self> {
        let root = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| auth_error("Windows LOCALAPPDATA unavailable"))?;
        Self::open(
            PathBuf::from(root)
                .join("z-harness")
                .join("credentials.chatgpt"),
            Remote::production()?,
        )
    }
    fn open(path: PathBuf, remote: Remote) -> Result<Self> {
        Ok(Self {
            vault: Vault::open(path, remote.fixture)?,
            remote,
        })
    }
    /// Only accepts direct loopback HTTP and an owned fixture directory. It never
    /// reads the live vault; fixture records cannot be used at production origins.
    pub fn loopback_fixture(path: PathBuf, base: String) -> Result<Self> {
        Self::open(path, Remote::fixture(base)?)
    }
    pub fn host_id(&self) -> Result<String> {
        self.vault.host()
    }
    pub fn status(&self, profile: &str) -> Result<AccountStatus> {
        let guard = self.vault.lock(profile)?;
        let mut record = guard.read()?;
        if self.vault.logout_version(profile)? != record.accepted_logout {
            record.tokens = None;
            record.state = "logout_requested".into();
        }
        Ok(record.status(profile))
    }
    pub fn profiles(&self) -> Result<Vec<AccountStatus>> {
        self.vault
            .profiles()?
            .iter()
            .map(|p| self.status(p))
            .collect()
    }
    pub(crate) fn api(&self) -> &str {
        &self.remote.api
    }
    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.remote.client
    }
}
fn auth_error(message: &str) -> Error {
    Error::Configuration(format!("ChatGPT: {message}"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
