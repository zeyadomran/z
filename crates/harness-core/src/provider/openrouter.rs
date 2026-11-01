use super::*;
use crate::Error;
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderValue},
    redirect::Policy,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};
mod catalog;
mod reasoning;
mod stream;
use catalog::Metadata;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterConfig {
    pub model: String,
    /// Explicit ordered endpoint slugs; no default billing/provider route.
    pub upstreams: Vec<String>,
    #[serde(default)]
    pub allow_fallbacks: bool,
    #[serde(default = "deny_collection")]
    pub data_collection: String,
    #[serde(default)]
    pub zdr: bool,
    #[serde(default = "output_default")]
    pub max_output_tokens: usize,
    pub reasoning_effort: Option<String>,
    /// Explicit proxy URL, never inherited from process proxy variables.
    pub proxy: Option<String>,
    #[serde(default = "key_ref")]
    pub credential_env: String,
}
fn deny_collection() -> String {
    "deny".into()
}
fn output_default() -> usize {
    4096
}
fn key_ref() -> String {
    "OPENROUTER_API_KEY".into()
}
impl OpenRouterConfig {
    pub fn new(model: String, upstream: String) -> Self {
        Self {
            model,
            upstreams: vec![upstream],
            allow_fallbacks: false,
            data_collection: deny_collection(),
            zdr: false,
            max_output_tokens: output_default(),
            reasoning_effort: None,
            proxy: None,
            credential_env: key_ref(),
        }
    }
    fn validate(&self) -> Result<()> {
        let slug = |s: &str| {
            !s.is_empty()
                && s.len() <= 200
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_/.:".contains(&b))
        };
        if !slug(self.model.strip_prefix('~').unwrap_or(&self.model))
            || self.upstreams.is_empty()
            || self.upstreams.len() > 8
            || !self.upstreams.iter().all(|s| slug(s))
            || !matches!(self.data_collection.as_str(), "allow" | "deny")
            || !(1..=131072).contains(&self.max_output_tokens)
            || self.credential_env.is_empty()
            || !self
                .credential_env
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(config_error(
                "set a model, explicit upstream slugs, valid data policy, output cap and credential reference",
            ));
        }
        if let Some(proxy) = &self.proxy {
            let url =
                reqwest::Url::parse(proxy).map_err(|_| config_error("invalid explicit proxy"))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(config_error(
                    "proxy must be HTTP(S) without embedded credentials",
                ));
            }
        }
        Ok(())
    }
}
fn config_error(message: &str) -> Error {
    Error::Configuration(message.into())
}
fn client(config: &OpenRouterConfig) -> Result<Client> {
    let mut builder = Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(45));
    if let Some(proxy) = &config.proxy {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| config_error("invalid proxy"))?);
    }
    builder
        .build()
        .map_err(|_| config_error("cannot initialize HTTPS client"))
}
/// Contains a sensitive header; deliberately has no Debug/Serialize implementation.
/// Live credentials can only be loaded for the fixed OpenRouter HTTPS origin.
pub struct OpenRouterProvider {
    config: OpenRouterConfig,
    client: Client,
    authorization: HeaderValue,
    credential_identity: String,
    base: String,
    metadata: Metadata,
}
impl OpenRouterProvider {
    pub async fn from_env(mut config: OpenRouterConfig) -> Result<Self> {
        config.validate()?;
        let client = client(&config)?;
        let metadata = catalog::fetch(&client, "https://openrouter.ai/api/v1", &config).await?;
        config.model = metadata.model.clone();
        let secret = std::env::var(&config.credential_env).map_err(|_| {
            config_error("configured OpenRouter credential environment variable is unavailable")
        })?;
        if secret.trim().is_empty() {
            return Err(config_error("credential is empty"));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|_| config_error("invalid credential encoding"))?;
        authorization.set_sensitive(true);
        Ok(Self {
            config,
            client,
            authorization,
            credential_identity: credential_identity(&secret),
            base: "https://openrouter.ai/api/v1".into(),
            metadata,
        })
    }
    /// Controlled external HTTP boundary for offline wire tests. This constructor
    /// cannot read or accept credentials and only permits a loopback HTTP origin.
    pub async fn loopback_fixture(base: String, config: OpenRouterConfig) -> Result<Self> {
        Self::loopback_fixture_key_revision(base, config, 0).await
    }
    /// Simulates key replacement using a numbered dummy credential. It never
    /// accepts a secret or reads host credentials; only direct loopback is allowed.
    pub async fn loopback_fixture_key_revision(
        base: String,
        mut config: OpenRouterConfig,
        revision: u64,
    ) -> Result<Self> {
        config.validate()?;
        let url = reqwest::Url::parse(&base).map_err(|_| config_error("invalid fixture URL"))?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || config.proxy.is_some()
        {
            return Err(config_error(
                "fixture endpoint must be direct loopback HTTP",
            ));
        }
        let client = client(&config)?;
        let metadata = catalog::fetch(&client, &base, &config).await?;
        config.model = metadata.model.clone();
        let secret = if revision == 0 {
            "fixture-not-a-secret".into()
        } else {
            format!("fixture-not-a-secret-{revision}")
        };
        let mut authorization = HeaderValue::from_str(&format!("Bearer {secret}"))
            .map_err(|_| config_error("invalid fixture credential"))?;
        authorization.set_sensitive(true);
        Ok(Self {
            config,
            client,
            authorization,
            credential_identity: credential_identity(&secret),
            base,
            metadata,
        })
    }
    pub fn metadata_retrieved_at(&self) -> SystemTime {
        self.metadata.retrieved
    }
    async fn request(
        &mut self,
        request: ProviderRequest,
        limit: usize,
        progress: watch::Sender<Progress>,
    ) -> Result<ProviderResponse> {
        // Clear only response payload: keep the engine's attempt/retry counters.
        // Presentation state from a prior attempt cannot establish this route.
        progress.send_modify(|p| {
            p.text.clear();
            p.usage = None;
            p.generation_id = None;
            p.model = None;
            p.upstream = None;
            p.bytes = 0;
            p.output_token_limit = Some(self.config.max_output_tokens);
        });
        super::scripted::validate(&request)?;
        if self.metadata.retrieved.elapsed().unwrap_or(Duration::MAX) > Duration::from_secs(3600) {
            return Err(config_error(
                "model metadata expired; reopen the provider to refresh",
            ));
        }
        let mut messages = vec![json!({"role":"system","content":request.epoch.prefix})];
        let mut opaque_route = None;
        for m in request.messages {
            messages.push(match m{
                ProtocolMessage::User(text)=>json!({"role":"user","content":text}),
                ProtocolMessage::ToolResult{call_id,result}=>json!({"role":"tool","tool_call_id":call_id,"content":result}),
                ProtocolMessage::Assistant{text,calls}=>{if !calls.is_empty(){return Err(Error::Protocol("legacy tool history has no arguments".into()));}json!({"role":"assistant","content":text})},
                ProtocolMessage::AssistantData(m)=>{
                    let mut value=json!({"role":"assistant","content":m.text});
                    if !m.calls.is_empty(){value["tool_calls"]=json!(m.calls.iter().map(|c|json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>());}
                    if let Some(details)=m.opaque.get("reasoning_details"){value["reasoning_details"]=details.clone();}
                    if let Some(reasoning)=m.opaque.get("reasoning"){value["reasoning"]=reasoning.clone();}
                    if let Some(route)=m.opaque.get("route").and_then(Value::as_str){
                        if opaque_route.as_deref().is_some_and(|r|r!=route){return Err(Error::Protocol("opaque continuation route changed".into()));}
                        opaque_route=Some(route.to_owned());
                    }
                    value
                }
            });
        }
        let mut routing = json!({"only":self.config.upstreams,"order":self.config.upstreams,"allow_fallbacks":self.config.allow_fallbacks,"require_parameters":true,"data_collection":self.config.data_collection,"zdr":self.config.zdr});
        let mut routes = self.metadata.routes.clone();
        if let Some(route) = opaque_route {
            if !routes.contains_key(&route) {
                return Err(Error::Protocol("opaque route no longer eligible".into()));
            }
            routes.retain(|tag, _| *tag == route);
            routing["only"] = json!([route]);
            routing["order"] = json!([route]);
            routing["allow_fallbacks"] = json!(false);
        }
        let mut body = json!({"model":self.config.model,"stream":true,"messages":messages,"tools":serde_json::from_str::<Value>(&request.epoch.tool_schema)?,"max_tokens":self.config.max_output_tokens,"provider":routing});
        if let Some(effort) = &self.config.reasoning_effort {
            body["reasoning"] = json!({"effort":effort});
        }
        let response = self
            .client
            .post(format!("{}/chat/completions", self.base))
            .header(AUTHORIZATION, self.authorization.clone())
            .json(&body)
            .send()
            .await;
        let mut response = match response {
            Ok(r) => r,
            Err(e) => return Ok(ProviderResponse::stopped(transport(&e))),
        };
        let status = response.status().as_u16();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                    httpdate::parse_http_date(v)
                        .ok()
                        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
                })
            });
        if status != 200 {
            let bytes = catalog::bounded_body(&mut response, 65536).await?;
            let error: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Ok(ProviderResponse {
                reason: stream::error_reason(status, &error),
                message: None,
                retry_after: retry,
            });
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(';').next() == Some("text/event-stream"))
        {
            return Ok(ProviderResponse::stopped(StopReason::Protocol));
        }
        let mut parser = stream::Stream::new(
            limit,
            self.config.model.clone(),
            self.metadata.canonical.clone(),
            routes,
            progress,
        );
        while let Some(chunk) = match response.chunk().await {
            Ok(c) => c,
            Err(e) => return Ok(ProviderResponse::stopped(transport(&e))),
        } {
            if let Some(mut outcome) = parser.feed(&chunk)? {
                outcome.retry_after = retry;
                return Ok(outcome);
            }
        }
        Ok(ProviderResponse::stopped(StopReason::Transport))
    }
}
// Deliberately conservative: replacing a key requires a new session, even if
// the replacement belongs to the same account. Never include this in diagnostics.
fn credential_identity(secret: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"z/openrouter/credential/v1\0");
    hash.update(secret.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn transport(e: &reqwest::Error) -> StopReason {
    if e.is_timeout() || e.is_connect() || e.is_body() {
        StopReason::RetryableTransport
    } else {
        StopReason::Transport
    }
}
impl Provider for OpenRouterProvider {
    fn binding(&self) -> ProviderBinding {
        let mut options = serde_json::to_value(&self.config).expect("serializable config");
        // This is a host budget, not account/model/routing identity. Explicitly
        // changing it on reopen must permit recovery from a truncated response.
        options
            .as_object_mut()
            .expect("config object")
            .remove("max_output_tokens");
        ProviderBinding {
            provider: "openrouter".into(),
            account_ref: format!(
                "{}:sha256:{}",
                self.config.credential_env, self.credential_identity
            ),
            model: self.config.model.clone(),
            options: options.to_string(),
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_tokens: self.metadata.context,
            output_tokens: self.config.max_output_tokens,
        }
    }
    fn respond<'a>(
        &'a mut self,
        r: ProviderRequest,
        n: usize,
        p: watch::Sender<Progress>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(self.request(r, n, p))
    }
}
