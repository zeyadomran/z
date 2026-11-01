use super::*;
impl ChatGptAuth {
    /// Always retrieve for the selected registration; never share account catalogs.
    pub async fn catalog(&mut self, profile: &str) -> Result<Catalog> {
        let mut refresh = None;
        for attempt in 0..2 {
            let lease = self.lease(profile, refresh).await?;
            let request = self
                .remote
                .client
                .get(format!("{}/models", self.remote.api))
                .header(reqwest::header::AUTHORIZATION, lease.header.clone())
                .send();
            let response = tokio::select! {r=request=>r.map_err(|_|auth_error("account catalog unavailable"))?,_=self.logout_requested(profile,&lease.logout_version)=>return Err(auth_error("logout cancelled catalog request"))};
            if response.status().as_u16() == 401 {
                refresh = Some(lease.generation);
                if attempt == 0 {
                    continue;
                }
                return Err(auth_error(
                    "account catalog still rejected after one refresh; diagnose account/service grant, do not repeat sign-in",
                ));
            }
            if !response.status().is_success() {
                return Err(auth_error(
                    "account catalog rejected; diagnose account eligibility, grant/policy or temporary route availability; no alternate billing route",
                ));
            }
            let value = tokio::select! {r=transport::bounded_json(response)=>r?,_=self.logout_requested(profile,&lease.logout_version)=>return Err(auth_error("logout cancelled catalog response"))};
            let entries = value["models"]
                .as_array()
                .ok_or_else(|| auth_error("invalid account catalog"))?;
            let mut result = Vec::new();
            let mut ids = std::collections::BTreeSet::new();
            for item in entries {
                if item["visibility"] != "list" {
                    continue;
                }
                let m: Model = serde_json::from_value(item.clone())
                    .map_err(|_| auth_error("invalid catalog model"))?;
                if m.slug.is_empty()
                    || m.slug.len() > 200
                    || m.display_name.len() > 512
                    || !ids.insert(m.slug.clone())
                {
                    return Err(auth_error("invalid catalog entry"));
                }
                result.push(m);
            }
            return Ok(Catalog {
                identity: lease.identity.clone(),
                retrieved_at: now(),
                models: result,
            });
        }
        Err(auth_error("account catalog unavailable"))
    }
    pub async fn models(&mut self, profile: &str) -> Result<Vec<Model>> {
        Ok(self.catalog(profile).await?.models)
    }
}
