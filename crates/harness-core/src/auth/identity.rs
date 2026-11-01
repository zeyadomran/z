use super::*;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
impl Remote {
    pub async fn identity(
        &mut self,
        jwt: &str,
        client: &str,
        nonce: Option<&str>,
    ) -> Result<Identity> {
        let invalid = || auth_error("ID token signature, identity or claims are invalid");
        let header = decode_header(jwt).map_err(|_| invalid())?;
        if header.alg != Algorithm::RS256 {
            return Err(invalid());
        }
        let kid = header.kid.as_deref().ok_or_else(invalid)?;
        if self
            .keys
            .as_ref()
            .is_none_or(|(k, t)| now().saturating_sub(*t) > 3600 || k.find(kid).is_none())
        {
            let d = self.discovery().await?;
            let r = self
                .client
                .get(&d.jwks_uri)
                .send()
                .await
                .map_err(|_| invalid())?;
            if !r.status().is_success() {
                return Err(invalid());
            }
            let keys: JwkSet =
                serde_json::from_value(transport::bounded_json(r).await?).map_err(|_| invalid())?;
            self.keys = Some((keys, now()));
        }
        let jwk = self
            .keys
            .as_ref()
            .and_then(|(k, _)| k.find(kid))
            .ok_or_else(invalid)?;
        if jwk
            .common
            .key_algorithm
            .is_some_and(|a| a != jsonwebtoken::jwk::KeyAlgorithm::RS256)
        {
            return Err(invalid());
        }
        let key = DecodingKey::from_jwk(jwk).map_err(|_| invalid())?;
        if jwk
            .common
            .public_key_use
            .as_ref()
            .is_some_and(|u| u != &jsonwebtoken::jwk::PublicKeyUse::Signature)
            || jwk
                .common
                .key_operations
                .as_ref()
                .is_some_and(|ops| !ops.contains(&jsonwebtoken::jwk::KeyOperations::Verify))
        {
            return Err(invalid());
        }
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[client]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
        validation.leeway = 30;
        validation.validate_nbf = true;
        let claims = decode::<Value>(jwt, &key, &validation)
            .map_err(|_| invalid())?
            .claims;
        let sub = claims["sub"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(invalid)?;
        let iat = claims["iat"].as_u64().ok_or_else(invalid)?;
        if iat > now() + 30 || claims["exp"].as_u64().is_none_or(|exp| exp <= iat) {
            return Err(invalid());
        }
        if let Some(expected) = nonce
            && claims["nonce"].as_str() != Some(expected)
        {
            return Err(invalid());
        }
        let audiences = claims["aud"].as_array().map_or(1, Vec::len);
        if (audiences > 1 || claims.get("azp").is_some()) && claims["azp"].as_str() != Some(client)
        {
            return Err(invalid());
        }
        Ok(Identity {
            issuer: ISSUER.into(),
            client_id: client.into(),
            subject: sub.into(),
        })
    }
}
pub(super) async fn validated_tokens(
    remote: &mut Remote,
    body: Value,
    client: &str,
    nonce: Option<&str>,
    previous: Option<&Tokens>,
) -> Result<(Tokens, Option<Identity>)> {
    let invalid = || auth_error("invalid OAuth token response; authorize again");
    let string = |k: &str| {
        body[k]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 32768)
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let access_token = string("access_token")?;
    let refresh_token = string("refresh_token")?;
    let token_type = string("token_type")?;
    if !token_type.eq_ignore_ascii_case("Bearer") {
        return Err(invalid());
    }
    let scope = string("scope")?;
    let expiry = body["expires_in"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= 86400)
        .ok_or_else(invalid)?;
    let id_token = body
        .get("id_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| previous.map(|p| p.id_token.clone()))
        .ok_or_else(invalid)?;
    let identity = if body.get("id_token").is_some() {
        let identity = remote.identity(&id_token, client, nonce).await?;
        // A refresh is not a new authorization. An optional nonce must still
        // equal the original transaction; omission is permitted on refresh.
        if nonce.is_none() {
            let claims = jwt_claims_after_verification(&id_token)?;
            if let Some(n) = claims.get("nonce")
                && Some(n.as_str().ok_or_else(invalid)?) != previous.map(|p| p.nonce.as_str())
            {
                return Err(invalid());
            }
        }
        Some(identity)
    } else {
        None
    };
    Ok((
        Tokens {
            access_token,
            refresh_token,
            id_token,
            token_type,
            scope,
            received_at: now(),
            expires_at: now() + expiry,
            earliest_refresh_at: body
                .get("earliest_refresh_at")
                .cloned()
                .unwrap_or(Value::Null),
            nonce: nonce
                .map(str::to_owned)
                .or_else(|| previous.map(|p| p.nonce.clone()))
                .ok_or_else(invalid)?,
        },
        identity,
    ))
}
fn jwt_claims_after_verification(jwt: &str) -> Result<Value> {
    use base64::Engine;
    let part = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| auth_error("invalid ID token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| auth_error("invalid ID token"))?;
    serde_json::from_slice(&bytes).map_err(|_| auth_error("invalid ID token"))
}
