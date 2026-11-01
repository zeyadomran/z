use super::*;
use reqwest::{Client, Url, redirect::Policy};
use std::time::Duration;
#[derive(Clone)]
pub(super) struct Remote {
    pub client: Client,
    pub api: String,
    auth: String,
    pub fixture: bool,
    pub keys: Option<(jsonwebtoken::jwk::JwkSet, u64)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub slug: String,
    pub display_name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub identity: Identity,
    pub retrieved_at: u64,
    pub models: Vec<Model>,
}
#[derive(Deserialize)]
pub(super) struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub revocation_endpoint: String,
}
pub(super) struct Reply {
    pub status: u16,
    pub body: Value,
}
impl Remote {
    fn new(auth: String, api: String, fixture: bool) -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| auth_error("HTTPS client unavailable"))?;
        Ok(Self {
            client,
            api,
            auth,
            fixture,
            keys: None,
        })
    }
    pub fn production() -> Result<Self> {
        Self::new(ISSUER.into(), RESOURCE.into(), false)
    }
    pub fn fixture(base: String) -> Result<Self> {
        let u = Url::parse(&base).map_err(|_| auth_error("invalid fixture origin"))?;
        if u.scheme() != "http"
            || u.host_str() != Some("127.0.0.1")
            || u.path() != "/"
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
        {
            return Err(auth_error("fixture requires direct 127.0.0.1 origin"));
        }
        let base = base.trim_end_matches('/').to_owned();
        Self::new(base.clone(), format!("{base}/v1"), true)
    }
    pub async fn discovery(&self) -> Result<Discovery> {
        let r = self
            .client
            .get(format!("{}/.well-known/openid-configuration", self.auth))
            .send()
            .await
            .map_err(|_| auth_error("discovery unavailable"))?;
        if !r.status().is_success() {
            return Err(auth_error("discovery unavailable"));
        }
        let d: Discovery = serde_json::from_value(bounded_json(r).await?)
            .map_err(|_| auth_error("invalid discovery"))?;
        if d.issuer != ISSUER
            || d.authorization_endpoint != format!("{}/api/accounts/authorize", self.auth)
            || d.token_endpoint != format!("{}/api/accounts/oauth/token", self.auth)
            || d.jwks_uri != format!("{}/.well-known/jwks.json", self.auth)
            || d.revocation_endpoint != format!("{}/api/accounts/oauth/revoke", self.auth)
        {
            return Err(auth_error("untrusted discovery endpoints"));
        }
        Ok(d)
    }
    pub async fn form(&self, url: &str, fields: &[(&str, &str)]) -> Result<Reply> {
        let response = self
            .client
            .post(url)
            .form(fields)
            .send()
            .await
            .map_err(|_| {
                auth_error("OAuth transmission uncertain; fresh authorization required")
            })?;
        let status = response.status().as_u16();
        Ok(Reply {
            status,
            body: bounded_json(response).await?,
        })
    }
}
pub(crate) async fn bounded_json(mut r: reqwest::Response) -> Result<Value> {
    let mut bytes = Vec::new();
    while let Some(c) = r
        .chunk()
        .await
        .map_err(|_| auth_error("incomplete HTTP response"))?
    {
        if bytes.len() + c.len() > 131072 {
            return Err(auth_error("HTTP response exceeds limit"));
        }
        bytes.extend_from_slice(&c);
    }
    serde_json::from_slice(&bytes).map_err(|_| auth_error("invalid HTTP JSON response"))
}
