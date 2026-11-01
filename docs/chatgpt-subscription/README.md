# Direct ChatGPT subscription contract

Verified against public official documentation on **2026-11-01** for issue #2.
This is the M1b implementation prerequisite, not account or runtime qualification.
The inspected worktree contained plans and repository tooling, with no Rust auth
or provider implementation. No credentials were read, registration attempted,
user signed in, or inference requested. Only public documentation and OIDC
discovery metadata were fetched.

## Decision and eligibility

Implement an original Rust public OAuth client and direct HTTP/SSE Responses
adapter. The documented OSS/local route supports dynamic registration without a
client secret or partner API key. The bootstrap identifier is
`dynamic_agent_client`; the resulting issued client ID belongs to this app's
user/workspace registration. Never borrow Codex's or another app's registration.
The official quickstart describes eligible Plus/Pro accounts; it does not prove
eligibility for a particular user, workspace, region, or model.
[Quickstart](https://developers.openai.com/siwc/quickstart),
[registration](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

The quickstart makes plan usage available to open-source partners and selected
private clients; running locally alone does not establish eligibility. The owner
selected the [MIT license](../../LICENSE) for z on 2026-11-01, resolving the
previously missing license prerequisite. Before M1b/M6 distribution qualification,
confirm the shipped source and license establish the documented open-source
route, or obtain OpenAI selection for a private client. The public docs do not
define OpenAI's precise open-source criterion; a license is not proof of acceptance.
[Eligibility](https://developers.openai.com/siwc/quickstart).

The open-source integration documentation directs paid or remotely hosted products to the
[interest form](https://openai.com/form/sign-in-with-chatgpt-interest/).
A later paid or hosted distribution therefore needs a separate access decision.
An open repository alone is not proof that an arbitrary distribution qualifies.
Plan usage grants no access to the user's ChatGPT conversations.
[Distribution scope](https://developers.openai.com/siwc/token-sharing-open-source).

- [Authentication and credential lifecycle](auth.md): endpoints, registration,
  host identity, verification, refresh, and logout.
- [Responses adapter](responses.md): account models, request restrictions,
  function continuation, errors, and usage.

## External prerequisites and release blockers

| Gate | Required evidence in M1b/M6 | Status of this research |
| --- | --- | --- |
| Distribution | Ship MIT-licensed source and establish route eligibility, or obtain private-client selection; obtain separate access if paid/hosted | MIT selected and included; distribution not qualified |
| Own registration | User completes this app's dynamic flow with an eligible account/workspace and grants direct plan usage | Not attempted; no issued registration obtained |
| Local runtime | System browser, working `127.0.0.1` callback, trusted HTTPS, accurate clock, protected credential persistence | Rust/Windows behavior not exercised |
| Account access | Selected account catalog, completed subscription response, harness-owned read-tool call and continuation | Not attempted; listing/sign-in alone will not pass |
| Lifecycle | Rotation, reauthorization, account-bound restart, logout/revocation, quota recovery | Implementation and qualification outstanding |
| Billing | Explicitly verify intended subscription usage and user-controlled app/credit settings | No usage or billing evidence collected |

**No actual access denial was observed:** no authenticated access was attempted.
Possible external blockers have precise recovery: ineligible user/workspace or
region/policy denial needs an eligibility/access decision; missing direct scope
needs explicit consent; route-unavailable 503 needs bounded retry and eventual
integration diagnosis. Record sanitized status/code/request ID if encountered;
do not label a temporary outage an eligibility denial.
[Errors](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery).

Keep M1b/M6 blocked until the intended account completes the required direct
workflow. Independent M0 and later engineering may continue. API credits,
key-funded OpenAI requests, and Codex wrappers cannot satisfy this release gate.
CI and controlled protocol fixtures cannot establish live account qualification.

## Reconciliation with the plan

Section 4.2 remains correct about the required route, endpoint, stateless HTTP,
completion boundary, and unsupported output-token cap. It now links this contract.
The factual correction in section 4.3 is its blanket treatment of 401/403 as
account action: direct ChatGPT 403 can mean an unsupported route or a grant/policy
configuration problem, for which repeated sign-in is inappropriate. The exact
error mapping in [responses.md](responses.md) replaces that assumption.

Section 4.2 also clarifies that its harness-owned tool acceptance uses the wire
`function` type; it does not require the separate wire `custom_tool_call` type.
Review follow-up makes distribution prerequisites, configured model limits,
completed reasoning-item assembly, and bounded authentication recovery explicit.
Product branding remains provisional and must be consistent before registration.

The namespace requirement, dynamic bootstrap/issued-ID distinction, remote
revocation, and full denylist below elaborate previously underspecified details;
they do not change required providers or Windows execution, approval,
cancellation, persistence, and recovery contracts. Frozen reviews are unchanged;
this research is not an independent review of the revised plan.
