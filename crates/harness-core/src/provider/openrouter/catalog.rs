use super::*;
use std::collections::BTreeMap;
pub(super) struct Metadata {
    pub retrieved: SystemTime,
    pub context: usize,
    pub canonical: String,
    pub model: String,
    pub routes: BTreeMap<String, String>,
}
pub(super) async fn bounded_body(
    response: &mut reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| config_error("HTTP response interrupted"))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(config_error("HTTP metadata/error body exceeds cap"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn get(client: &Client, url: String) -> Result<Value> {
    let mut r = client
        .get(url)
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .map_err(|_| config_error("model catalog unavailable; no capability guesses permitted"))?;
    if !r.status().is_success() {
        return Err(config_error(
            "model catalog unavailable; no capability guesses permitted",
        ));
    }
    serde_json::from_slice(&bounded_body(&mut r, 8 * 1024 * 1024).await?)
        .map_err(|_| config_error("invalid model catalog"))
}
fn supports(v: &Value, parameter: &str) -> bool {
    v["supported_parameters"]
        .as_array()
        .is_some_and(|a| a.iter().any(|p| p == parameter))
}
pub(super) async fn fetch(client: &Client, base: &str, c: &OpenRouterConfig) -> Result<Metadata> {
    let catalog = get(client, format!("{base}/models")).await?;
    let models = catalog["data"]
        .as_array()
        .ok_or_else(|| config_error("invalid model catalog"))?;
    let selected = models
        .iter()
        .find(|m| m["id"] == c.model)
        .ok_or_else(|| config_error("configured model absent from live catalog"))?;
    // Public aliases are resolved once, then the concrete target is sent and
    // bound to the session so alias movement cannot silently switch its model.
    let m = if let Some(target) = selected.get("alias_target").filter(|v| !v.is_null()) {
        let slug = target["slug"]
            .as_str()
            .ok_or_else(|| config_error("alias target unavailable"))?;
        models
            .iter()
            .find(|m| m["id"] == slug && m.get("alias_target").is_none_or(Value::is_null))
            .ok_or_else(|| config_error("alias target absent from model catalog"))?
    } else {
        selected
    };
    let model = m["id"]
        .as_str()
        .ok_or_else(|| config_error("model identity unavailable"))?;
    for required in ["tools", "max_tokens"] {
        if !supports(m, required) {
            return Err(config_error(
                "model lacks required tools/output-cap capability",
            ));
        }
    }
    if let Some(effort) = &c.reasoning_effort
        && (!supports(m, "reasoning")
            || !m["reasoning"]["supported_efforts"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == effort)))
    {
        return Err(config_error(
            "reasoning effort is not explicitly supported by model metadata",
        ));
    }
    if !m["architecture"]["output_modalities"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == "text"))
    {
        return Err(config_error("model must support text output"));
    }
    let mut context = m["context_length"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| config_error("model context limit unavailable"))?
        as usize;
    let endpoints = get(client, format!("{base}/models/{model}/endpoints")).await?;
    let list = endpoints["data"]["endpoints"]
        .as_array()
        .ok_or_else(|| config_error("model endpoint metadata unavailable"))?;
    let mut routes = BTreeMap::new();
    for slug in &c.upstreams {
        let endpoint = list.iter().find(|e| e["tag"] == *slug).ok_or_else(|| {
            config_error("upstream slug absent; use an exact catalog endpoint tag")
        })?;
        for required in ["tools", "max_tokens"] {
            if !supports(endpoint, required) {
                return Err(config_error(
                    "upstream does not support required parameters",
                ));
            }
        }
        if c.reasoning_effort.is_some() && !supports(endpoint, "reasoning") {
            return Err(config_error("upstream lacks reasoning support"));
        }
        let ctx = endpoint["context_length"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| config_error("upstream context limit unavailable"))?
            as usize;
        let max = endpoint["max_completion_tokens"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| config_error("upstream output limit unavailable"))?
            as usize;
        if c.max_output_tokens > max || c.max_output_tokens >= ctx {
            return Err(config_error("output cap exceeds upstream/model allowance"));
        }
        context = context.min(ctx);
        let name = endpoint["provider_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| config_error("upstream identity unavailable"))?;
        if routes.values().any(|existing: &String| existing == name) || routes.contains_key(slug) {
            return Err(config_error(
                "configured endpoint identities are ambiguous; select one exact endpoint tag",
            ));
        }
        routes.insert(slug.clone(), name.into());
    }
    Ok(Metadata {
        retrieved: SystemTime::now(),
        context,
        canonical: m["canonical_slug"].as_str().unwrap_or(model).into(),
        model: model.into(),
        routes,
    })
}
