use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::Instant,
};
/// Single-use pending transaction. Dropping it closes the callback listener and
/// releases the profile lock. It deliberately has no Debug/Serialize support.
pub struct Login {
    auth: ChatGptAuth,
    guard: store::Guard,
    _pending: store::Guard,
    record: Record,
    profile: String,
    listener: TcpListener,
    redirect: String,
    url: String,
    state: String,
    nonce: String,
    verifier: String,
    deadline: Instant,
    logout_version: Option<String>,
}
impl ChatGptAuth {
    pub async fn begin_login(&self, profile: &str, consent: bool) -> Result<Login> {
        if matches!(profile, "host" | "pending-login") {
            return Err(auth_error("reserved profile name"));
        }
        let guard = self.vault.lock(profile)?;
        let pending = self.vault.lock("pending-login")?;
        let record = guard.read()?;
        let logout_version = self.vault.logout_version(profile)?;
        let discovery = self.remote.discovery().await?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| auth_error("loopback callback listener unavailable"))?;
        let redirect = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener
                .local_addr()
                .map_err(|_| auth_error("callback unavailable"))?
                .port()
        );
        let state = random();
        let nonce = random();
        let verifier = random();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse(&discovery.authorization_endpoint)
            .map_err(|_| auth_error("invalid authorization endpoint"))?;
        {
            let host = self.host_id()?;
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                (
                    "client_id",
                    record
                        .client_id
                        .as_deref()
                        .unwrap_or("dynamic_agent_client"),
                ),
                ("ext_agent_host_id", &host),
                ("response_type", "code"),
                ("redirect_uri", &redirect),
                ("scope", SCOPES),
                ("resource", RESOURCE),
                ("state", &state),
                ("nonce", &nonce),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge),
            ]);
            if record.client_id.is_none() {
                query.append_pair("agent_name_hint", PRODUCT_NAME);
            }
            if consent {
                query.append_pair("prompt", "consent");
            }
            // Hints are optional. Omit them to keep browser URLs free of retained
            // ID tokens and email addresses; validation remains mandatory.
        }
        Ok(Login {
            auth: self.clone(),
            guard,
            _pending: pending,
            record,
            profile: profile.into(),
            listener,
            redirect,
            url: url.to_string(),
            state,
            nonce,
            verifier,
            deadline: Instant::now() + Duration::from_secs(300),
            logout_version,
        })
    }
}
impl Login {
    /// Sensitive transaction URL: send only to the system browser, never logs.
    pub fn authorization_url(&self) -> &str {
        &self.url
    }
    pub fn open_browser(&self) -> Result<()> {
        if self.auth.remote.fixture {
            return Err(auth_error("fixture cannot launch browser"));
        }
        #[cfg(windows)]
        {
            windows::browser(&self.url)
        }
        #[cfg(not(windows))]
        {
            Err(auth_error("Windows browser required"))
        }
    }
    pub async fn finish(mut self) -> Result<AccountStatus> {
        let deadline = self.deadline;
        tokio::time::timeout_at(deadline,async {
            let (mut stream,peer)=self.listener.accept().await.map_err(|_|auth_error("callback listener failed"))?;
            if !peer.ip().is_loopback() { return Err(auth_error("invalid callback origin")); }
            let mut bytes=Vec::new();
            loop {
                let mut buffer=[0u8;1024];
                let n=stream.read(&mut buffer).await.map_err(|_|auth_error("callback read failed"))?;
                if n==0 { return Err(auth_error("incomplete callback")); }
                bytes.extend_from_slice(&buffer[..n]);
                if bytes.len()>16384 { return Err(auth_error("callback too large")); }
                if bytes.windows(4).any(|w|w==b"\r\n\r\n") { break; }
            }
            let line=std::str::from_utf8(&bytes).map_err(|_|auth_error("invalid callback"))?.lines().next().ok_or_else(||auth_error("invalid callback"))?;
            let parts:Vec<_>=line.split_whitespace().collect();
            let result=if parts.len()==3 && parts[0]=="GET" && parts[2]=="HTTP/1.1" { self.exchange(parts[1]).await } else { Err(auth_error("invalid callback method")) };
            let body=if result.is_ok(){"Sign-in complete. Return to harness."}else{"Sign-in failed. Return to harness for recovery."};
            let response=format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",body.len());
            // Browser acknowledgement is bounded by the same transaction deadline.
            let _=stream.write_all(response.as_bytes()).await;
            result
        }).await.map_err(|_|auth_error("sign-in expired; start a new transaction"))?
    }
    async fn exchange(&mut self, target: &str) -> Result<AccountStatus> {
        let invalid =
            || auth_error("callback state, path or registration invalid; start a new sign-in");
        if !target.starts_with("/auth/callback?") || target.contains('#') {
            return Err(invalid());
        }
        let url =
            reqwest::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| invalid())?;
        if url.path() != "/auth/callback" {
            return Err(invalid());
        }
        let mut params = BTreeMap::new();
        for (k, v) in url.query_pairs() {
            if params.insert(k.into_owned(), v.into_owned()).is_some() {
                return Err(invalid());
            }
        }
        if params.get("state") != Some(&self.state) {
            return Err(invalid());
        }
        if params.contains_key("error") {
            return Err(auth_error("authorization denied; no code exchanged"));
        }
        let client = match (&self.record.client_id, params.get("client_id")) {
            (Some(old), Some(new)) if old != new => return Err(invalid()),
            (Some(old), _) => old.clone(),
            (None, Some(new))
                if !new.is_empty()
                    && new.len() <= 256
                    && new != "dynamic_agent_client"
                    && new
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
            {
                new.clone()
            }
            _ => return Err(invalid()),
        };
        let code = params
            .get("code")
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        self.record.client_id = Some(client.clone());
        // Keep a newly issued registration even if the one-use code is rejected.
        // Existing active credentials remain isolated until identity validation.
        self.guard.save(&self.record)?;
        let discovery = self.auth.remote.discovery().await?;
        let r = self
            .auth
            .remote
            .form(
                &discovery.token_endpoint,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", &client),
                    ("code", code),
                    ("code_verifier", &self.verifier),
                    ("redirect_uri", &self.redirect),
                    ("resource", RESOURCE),
                ],
            )
            .await?;
        if r.status != 200 {
            return Err(auth_error(
                "code exchange rejected; retry sign-in using this saved profile",
            ));
        }
        let (tokens, identity) = identity::validated_tokens(
            &mut self.auth.remote,
            r.body,
            &client,
            Some(&self.nonce),
            None,
        )
        .await?;
        let identity = identity.ok_or_else(invalid)?;
        if self
            .record
            .identity
            .as_ref()
            .is_some_and(|old| old != &identity)
        {
            return Err(auth_error(
                "signed-in identity differs from this registration; create a separate profile",
            ));
        }
        self.record.identity = Some(identity);
        if self.auth.vault.logout_version(&self.profile)? != self.logout_version {
            return Err(auth_error(
                "logout superseded this sign-in; start a new transaction",
            ));
        }
        self.record.accepted_logout = self.logout_version.clone();
        self.record.tokens = Some(tokens);
        self.record.generation += 1;
        self.record.state = "authorized".into();
        self.guard.save(&self.record)?;
        Ok(self.record.status(&self.profile))
    }
}
