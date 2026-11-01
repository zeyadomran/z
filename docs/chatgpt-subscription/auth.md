# ChatGPT authentication implementation contract

Verified **2026-11-01**. Read [scope and qualification gates](README.md) first.
Protocol facts below come from linked OpenAI sources. Items explicitly called
“harness policy” are local implementation decisions, not claims of provider behavior.

## Public endpoints and credentials

Production [OIDC discovery](https://auth.openai.com/.well-known/openid-configuration)
was fetched without authentication. Its observed values are:

| Operation | Exact URL | Request authentication / encoding |
| --- | --- | --- |
| Discovery | `https://auth.openai.com/.well-known/openid-configuration` | Public GET |
| Issuer | `https://auth.openai.com` | Exact expected identity issuer |
| Authorization | `https://auth.openai.com/api/accounts/authorize` | System browser; URL-encoded query |
| Code exchange / refresh | `https://auth.openai.com/api/accounts/oauth/token` | POST; `Content-Type: application/x-www-form-urlencoded`; public client, no secret or Basic auth |
| JWKS | `https://auth.openai.com/.well-known/jwks.json` | Public GET; observed ID-token algorithm `RS256` |
| Revocation | `https://auth.openai.com/api/accounts/oauth/revoke` | POST form; refresh token and issued client ID |

Resolve `jwks_uri` and `revocation_endpoint` from trusted discovery at runtime.
Harness policy: require the production issuer and approved HTTPS destinations;
never follow token-bearing redirects or accept a callback-supplied issuer/key URL.
The [website identity-validation guide](https://developers.openai.com/siwc/website)
documents discovery/JWT verification, but its commercial-client provisioning and
website-session rules do not replace the OSS registration flow.

## Host and registration storage

OpenAI distinguishes a host from a registration. An issued client is tied to a
user and selected workspace; multiple hosts may share that registration for the
same user/workspace, each with a different host ID. Host IDs are opaque identifiers,
not authentication or proof of key possession. Recommended format is an RFC 9278
JWK-thumbprint URI; `urn:uuid:<UUIDv4>` and `did:key:<key>` are also supported.
[Host identity](https://developers.openai.com/siwc/token-sharing-open-source#a-client-vs-an-agent-host).

Harness policy: generate one random `urn:uuid:<UUIDv4>` per installation, persist
before sign-in, and retain across restart/logout. Do not derive it from personal
or hardware identifiers; do not copy it into distributed binaries. Keep:

- Host record: stable `ext_agent_host_id`.
- Registration record: local profile ID, issued `client_id`, verified issuer and
  subject, stable display label, optional minimal validated display information.
- Protected credential record: access/refresh/ID tokens, token type, granted
  scopes, receipt time, expiry, and returned refresh scheduling metadata.
- Session binding: provider plus the exact registration/verified identity
  reference; never tokens. A different account requires a separate session.

Email and `sub` are not workspace identifiers. Use `(issuer, client_id, sub)` as
the verified binding, preserving separate registrations even if emails match.
Workspace binding comes from the issued registration; do not invent a workspace
ID from opaque access-token metadata. A new account/workspace uses a separate
registration. [Accounts](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions).

## Authorization transaction

Start a listener bound only to `127.0.0.1` before opening the system browser.
Use `http://127.0.0.1:<available-port>/auth/callback` from first registration onward.
Only the port can change between attempts; retain scheme, host, and path.
Do not substitute `localhost`. Within an attempt, exchange with the identical URI.
Generate fresh random state, nonce, and PKCE verifier; S256 challenge is the
unpadded base64url SHA-256 of the verifier.
[Sign-in parameters](https://developers.openai.com/siwc/token-sharing-open-source/sign-in#2-start-authorization).

Authorization query fields:

```text
client_id = dynamic_agent_client (new) OR selected issued ID (returning)
agent_name_hint = z (new registration only; display metadata)
ext_agent_host_id = persisted host ID
response_type = code
redirect_uri = exact callback URI
scope = openid profile email offline_access resource.invoke chatgpt.tokens.use.direct
resource = https://api.openai.com/v1
state = fresh transaction state
nonce = fresh OIDC nonce
code_challenge_method = S256
code_challenge = derived PKCE challenge
```

Returning sign-in may send the selected registration's retained ID token as
`id_token_hint` (even expired), and validated email as `login_hint`. Omit the ID
hint after logout. Hints never replace validation or change the registered
workspace. Redact authorization URLs containing hints.
[Returning accounts](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions#switching-chatgpt-accounts).

Harness policy: one expiring, single-use pending transaction, isolated from the
active profile; reject missing/duplicate critical callback parameters, expired
or replayed transactions, wrong state/path, and unsolicited callbacks. Consume
the transaction once; close its listener on completion, cancellation, or timeout.
Validate state even on `error=access_denied`; do not exchange a denied code.

For new registration require the callback's issued `client_id`, retaining it as
pending registration metadata before exchange. Never save/use `dynamic_agent_client`
as an issued ID. Returning callbacks may omit the ID; if present it must equal
the pending registration. Exchange these form fields:

```text
grant_type=authorization_code
client_id=<issued ID>
code=<returned code>
code_verifier=<original verifier>
redirect_uri=<original exact URI>
resource=https://api.openai.com/v1
```

On code-exchange `invalid_grant`, discard the code and start fresh authorization
with the retained issued ID. Pending registration is not an authenticated profile.
[Callback/exchange](https://developers.openai.com/siwc/token-sharing-open-source/sign-in#3-handle-the-callback-and-exchange-the-code).

## Identity and grant validation

Use a maintained Rust OIDC/JWT implementation. Verify the signature with trusted
JWKS and allowed algorithm, exact issuer, audience containing the issued client
ID, expiry, required nonempty subject, and original nonce. Follow OIDC audience
and authorized-party validation rather than decoding JWT claims as proof. Allow
only small clock skew; responsibly cache JWKS and refresh on an unfamiliar `kid`.
Check issued-at/not-before constraints where applicable. For returning accounts,
require the newly verified binding to match before replacing any credentials.
[Verification](https://developers.openai.com/siwc/website#6-verify-the-id-token).

Use token-response scopes, not callback scopes or requested scopes, to decide
whether `chatgpt.tokens.use.direct` was granted. Without it, identity can remain
signed in but inference is disabled. Explicit user enablement repeats OAuth with
the complete scope set; use `prompt=consent` until OpenAI confirms deployment of
`force_reconsent=true` for this integration. Do not force consent at every login.
[Consent recovery](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery#chatgpt-plan-use-isnt-enabled).

## Rotation, persistence, and logout

The token response documents `access_token`, `refresh_token`, `id_token`,
`token_type`, `expires_in`, `scope`, and `earliest_refresh_at`. Published lifetimes
are one hour for access and 30 days for refresh; replacement refresh tokens restart
the 30-day window. Store actual response metadata rather than hardcoding expiry.
`earliest_refresh_at` is listed without a detailed scheduling/type contract in
the token reference: preserve it and resolve its semantics before relying on it.
Access-token auth metadata is opaque; do not use decoded metadata as identity.
[Token reference](https://developers.openai.com/siwc/token-sharing-open-source/token-reference).

Refresh near expiry with form fields `grant_type=refresh_token`, issued
`client_id`, current `refresh_token`, and `resource=https://api.openai.com/v1`;
omit `scope`. Serialize across processes for the same renewable session. Replace
access token, refresh token, expiry and granted scopes atomically. Use returned
ID-token updates only after appropriate identity validation; a refresh is not a
fresh browser authorization transaction.
[Refresh](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions#refreshing-tokens).

Harness policy: Windows OS-protected credential storage outside workspaces and
session databases, with restrictive user ACLs; Unix `0600` examples do not prove
Windows protection. Lock across refresh/logout, reload the current token generation
under lock, and prevent an in-flight refresh from restoring logged-out credentials.
If a rotated response is lost or cannot be persisted, stop this route and recover
through reauthorization; never replay an uncertain old refresh token in a loop.
Temporary network failures preserve credentials. Terminal refresh codes
`invalid_grant`, `invalid_refresh_token`, `token_expired`, `refresh_token_expired`,
`refresh_token_invalidated`, and `refresh_token_reused` require clearing unusable
tokens and fresh OAuth with the saved ID; `invalid_client` needs configuration
repair. [Refresh errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery#refresh-errors).

Logout stops requests, attempts revocation using `token=<REFRESH_TOKEN>`,
`token_type_hint=refresh_token`, and issued `client_id`, then clears access,
refresh, and ID tokens. Empty HTTP 200 confirms success even for an invalid token.
Boundedly retry network/5xx failures while the token remains available. If local
logout completes without confirmation, explicitly report unconfirmed remote
revocation and link ChatGPT Settings. Retain registration mapping, host ID, and
local history. No disconnect push notification is promised; detect invalidation
through request/refresh outcomes.
[Logout](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions#end-the-renewable-session).

Never expose credentials, codes, PKCE values, or sensitive URLs in logs, tool
arguments, child environments, analytics, diagnostics, or public evidence.
