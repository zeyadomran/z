use super::*;
use crate::{
    Error,
    auth::{ChatGptAuth, Identity},
};
use serde_json::json;
mod errors;
mod request;
mod stream;
fn protocol() -> Error {
    Error::Protocol("invalid ChatGPT response; no partial calls executed".into())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatGptConfig {
    pub profile: String,
    pub model: String,
    pub context_tokens: usize,
    pub response_headroom: usize,
    pub limits_source: String,
}
pub struct ChatGptProvider {
    auth: ChatGptAuth,
    config: ChatGptConfig,
    identity: Identity,
    recovered_401: bool,
}
impl ChatGptProvider {
    pub async fn new(mut auth: ChatGptAuth, config: ChatGptConfig) -> Result<Self> {
        if config.model.is_empty()
            || config.model.len() > 200
            || !(4096..=2_000_000).contains(&config.context_tokens)
            || config.response_headroom == 0
            || config.response_headroom >= config.context_tokens
            || config.limits_source.trim().is_empty()
            || config.limits_source.len() > 1024
        {
            return Err(Error::Configuration(
                "ChatGPT requires exact model, valid context/headroom and limit provenance".into(),
            ));
        }
        let catalog = auth.catalog(&config.profile).await?;
        if !catalog.models.iter().any(|m| m.slug == config.model) {
            return Err(Error::Configuration(
                "selected model is absent from this account catalog".into(),
            ));
        }
        let identity = catalog.identity;
        Ok(Self {
            auth,
            config,
            identity,
            recovered_401: false,
        })
    }
    fn auth_failure(&self) -> StopReason {
        match self.auth.status(&self.config.profile) {
            Ok(s) if s.state == "authorized" && !s.direct_use => StopReason::ConsentRequired,
            Ok(s) if s.state == "authorized" => StopReason::AccountDiagnosis,
            Ok(_) => StopReason::ReauthorizationRequired,
            Err(_) => StopReason::AccountDiagnosis,
        }
    }
    async fn request(
        &mut self,
        r: ProviderRequest,
        n: usize,
        p: watch::Sender<Progress>,
    ) -> Result<ProviderResponse> {
        let body = request::body(r, &self.config.model, &self.identity.binding())?;
        p.send_modify(|p|p.context_budget=Some(json!({"context_tokens":self.config.context_tokens,"response_headroom":self.config.response_headroom,"source":self.config.limits_source})));
        let lease = match self.auth.lease(&self.config.profile, None).await {
            Ok(l) => l,
            Err(_) => return Ok(ProviderResponse::stopped(self.auth_failure())),
        };
        if lease.identity != self.identity {
            return Err(Error::Binding);
        }
        let generation = lease.generation;
        let client = self.auth.client().clone();
        let url = format!("{}/responses", self.auth.api());
        let mut response = tokio::select! {
            _=self.auth.logout_requested(&self.config.profile,&lease.logout_version)=>return Ok(ProviderResponse::stopped(StopReason::ReauthorizationRequired)),
            r=client.post(url).header(reqwest::header::AUTHORIZATION,lease.header.clone()).json(&body).send()=>match r { Ok(r)=>r,Err(_)=>return Ok(ProviderResponse::stopped(StopReason::Transport)) }
        };
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.parse::<u64>()
                    .ok()
                    .map(std::time::Duration::from_secs)
                    .or_else(|| {
                        httpdate::parse_http_date(s)
                            .ok()
                            .and_then(|t| t.duration_since(std::time::SystemTime::now()).ok())
                    })
            });
        if status != 200 {
            let value = tokio::select! {r=crate::auth::transport::bounded_json(response)=>r.unwrap_or(Value::Null),_=self.auth.logout_requested(&self.config.profile,&lease.logout_version)=>return Ok(ProviderResponse::stopped(StopReason::ReauthorizationRequired))};
            errors::diagnostic(&p, status, &value, request_id.as_deref(), "admission");
            let reason = errors::reason(status, &value);
            drop(lease);
            if reason == StopReason::Authentication {
                if self.recovered_401 {
                    return Ok(ProviderResponse::stopped(StopReason::AccountDiagnosis));
                }
                self.recovered_401 = true;
                return Ok(
                    match self
                        .auth
                        .lease(&self.config.profile, Some(generation))
                        .await
                    {
                        Ok(l) if l.identity == self.identity => ProviderResponse {
                            reason: StopReason::AuthenticationRefreshed,
                            message: None,
                            retry_after: Some(std::time::Duration::ZERO),
                        },
                        _ => ProviderResponse::stopped(self.auth_failure()),
                    },
                );
            }
            return Ok(ProviderResponse {
                reason,
                message: None,
                retry_after: retry,
            });
        }
        if !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.split(';').next() == Some("text/event-stream"))
        {
            return Err(protocol());
        }
        let mut parser = stream::Stream::new(
            self.config.model.clone(),
            self.identity.binding(),
            n,
            p.clone(),
        );
        loop {
            let chunk = tokio::select! {
                _=self.auth.logout_requested(&self.config.profile,&lease.logout_version)=>return Ok(ProviderResponse::stopped(StopReason::ReauthorizationRequired)),
                r=response.chunk()=>match r { Ok(c)=>c,Err(_)=>return Ok(ProviderResponse::stopped(StopReason::Transport)) }
            };
            let Some(chunk) = chunk else {
                return Ok(ProviderResponse::stopped(StopReason::Transport));
            };
            if let Some(mut out) = parser.feed(&chunk)? {
                if out.message.is_some() {
                    self.recovered_401 = false;
                }
                let diagnostic = p.borrow().diagnostic.clone();
                if let Some(diag) = diagnostic {
                    errors::diagnostic(
                        &p,
                        status,
                        &json!({"error":{"code":diag["code"],"param":diag["param"]}}),
                        request_id.as_deref(),
                        "stream",
                    );
                }
                // An SSE auth failure is never a safe admission rejection retry.
                if out.reason == StopReason::Authentication {
                    out.reason = StopReason::AccountDiagnosis;
                }
                out.retry_after = retry;
                return Ok(out);
            }
        }
    }
}
impl Provider for ChatGptProvider {
    fn binding(&self) -> ProviderBinding {
        ProviderBinding {
            provider: "chatgpt".into(),
            account_ref: self.identity.binding(),
            model: self.config.model.clone(),
            options: "{\"protocol\":\"direct-responses-v1\",\"route\":\"chatgpt-plan\"}".into(),
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_tokens: self.config.context_tokens,
            output_tokens: self.config.response_headroom,
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
