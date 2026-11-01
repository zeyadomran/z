use super::*;
use reqwest::header::HeaderValue;
use std::time::Duration;
pub(crate) struct Lease {
    _guard: store::Guard,
    pub identity: Identity,
    pub generation: u64,
    pub header: HeaderValue,
    pub logout_version: Option<String>,
}
impl ChatGptAuth {
    pub(crate) async fn lease(&mut self, profile: &str, force: Option<u64>) -> Result<Lease> {
        if profile == "host" {
            return Err(auth_error("reserved profile name"));
        }
        let guard = self.vault.lock(profile)?;
        let mut record = guard.read()?;
        if self.vault.logout_version(profile)? != record.accepted_logout {
            return Err(auth_error("logged out; sign in with the saved profile"));
        }
        if !record.direct() {
            return Err(auth_error(if record.tokens.is_some() {
                "direct plan usage is not granted; explicitly enable consent"
            } else {
                "sign-in required; registration retained"
            }));
        }
        let tokens = record
            .tokens
            .as_ref()
            .ok_or_else(|| auth_error("sign-in required"))?;
        if force == Some(record.generation) || tokens.expires_at <= now() + 60 {
            self.rotate(profile, &guard, &mut record).await?;
        }
        if self.vault.logout_version(profile)? != record.accepted_logout {
            return Err(auth_error("logout requested"));
        }
        if !record.direct() {
            return Err(auth_error("direct-use scope was not granted after refresh"));
        }
        let token = &record
            .tokens
            .as_ref()
            .ok_or_else(|| auth_error("sign-in required"))?
            .access_token;
        let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| auth_error("invalid access token encoding"))?;
        header.set_sensitive(true);
        Ok(Lease {
            _guard: guard,
            identity: record
                .identity
                .ok_or_else(|| auth_error("verified identity missing"))?,
            generation: record.generation,
            header,
            logout_version: record.accepted_logout,
        })
    }
    async fn rotate(
        &mut self,
        profile: &str,
        guard: &store::Guard,
        record: &mut Record,
    ) -> Result<()> {
        // Discovery happens before the durable invalidation, with no token sent.
        let discovery = self.remote.discovery().await?;
        let old = record.clone();
        let tokens = old
            .tokens
            .as_ref()
            .ok_or_else(|| auth_error("sign-in required"))?;
        let client = old
            .client_id
            .as_deref()
            .ok_or_else(|| auth_error("registration missing"))?;
        record.tokens = None;
        record.state = "rotation_uncertain".into();
        guard.save(record)?;
        let fields = [
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", &tokens.refresh_token),
            ("resource", RESOURCE),
        ];
        let request = self.remote.form(&discovery.token_endpoint, &fields);
        let reply = tokio::select! {r=request=>r?,_=self.logout_requested(profile,&old.accepted_logout)=>return Err(auth_error("logout interrupted refresh; authorization required"))};
        if reply.status != 200 {
            let code = reply.body["error"]
                .as_str()
                .or_else(|| reply.body["error"]["code"].as_str())
                .unwrap_or("");
            if matches!(code, "temporarily_unavailable" | "server_error") {
                *record = old;
                guard.save(record)?;
                return Err(auth_error(
                    "refresh temporarily unavailable; credentials retained; retry explicitly",
                ));
            }
            record.state = if code == "invalid_client" {
                "registration_repair_required"
            } else {
                "reauthorization_required"
            }
            .into();
            guard.save(record)?;
            return Err(auth_error(
                "refresh rejected; fresh authorization or registration repair required",
            ));
        }
        let (next, identity) =
            identity::validated_tokens(&mut self.remote, reply.body, client, None, Some(tokens))
                .await?;
        if identity
            .as_ref()
            .is_some_and(|i| Some(i) != old.identity.as_ref())
        {
            return Err(auth_error(
                "refresh identity changed; fresh authorization required",
            ));
        }
        record.tokens = Some(next);
        record.generation = old.generation + 1;
        record.state = "authorized".into();
        guard.save(record)
    }
    pub async fn refresh(&mut self, profile: &str) -> Result<AccountStatus> {
        if matches!(profile, "host" | "pending-login") {
            return Err(auth_error("reserved profile name"));
        }
        let guard = self.vault.lock(profile)?;
        let mut record = guard.read()?;
        if record.identity.is_none()
            || self.vault.logout_version(profile)? != record.accepted_logout
        {
            return Err(auth_error("sign-in required"));
        }
        self.rotate(profile, &guard, &mut record).await?;
        drop(guard);
        self.status(profile)
    }
    pub(crate) async fn logout_requested(&self, profile: &str, expected: &Option<String>) {
        loop {
            if !self
                .vault
                .logout_version(profile)
                .is_ok_and(|version| &version == expected)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    /// Returns true only when remote revocation was confirmed. Local credentials
    /// are cleared even on a dropped future, network error or process crash.
    pub async fn logout(&mut self, profile: &str) -> Result<bool> {
        self.vault.stop(profile)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let guard = loop {
            match self.vault.lock(profile) {
                Ok(g) => break g,
                Err(Error::Busy) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(e) => return Err(e),
            }
        };
        let mut record = guard.read()?;
        let tokens = record.tokens.take();
        record.state = "logged_out".into();
        record.accepted_logout = self.vault.logout_version(profile)?;
        record.generation += 1;
        guard.save(&record)?;
        let Some(tokens) = tokens else {
            return Ok(false);
        };
        let Ok(discovery) = self.remote.discovery().await else {
            return Ok(false);
        };
        let client = record
            .client_id
            .as_deref()
            .ok_or_else(|| auth_error("registration missing"))?;
        for attempt in 0..3 {
            let response = self
                .remote
                .client
                .post(&discovery.revocation_endpoint)
                .form(&[
                    ("token", tokens.refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client),
                ])
                .send()
                .await;
            match response {
                Ok(r) if r.status().as_u16() == 200 => return Ok(true),
                Ok(r) if !r.status().is_server_error() => return Ok(false),
                _ if attempt < 2 => tokio::time::sleep(Duration::from_secs(1 << attempt)).await,
                _ => return Ok(false),
            }
        }
        Ok(false)
    }
}
