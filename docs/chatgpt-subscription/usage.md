# Direct ChatGPT implementation and CLI

M1b implements the offline registration/authentication and read-only Responses
slice. **This does not qualify the app, account, model, or billing route.**
The required real sign-in, subscription inference, harness-owned tool round trip,
refresh/logout and approved coding workflow remain release gates in
[M6 #9](https://github.com/zeyadomran/z/issues/9). No real credentials, sign-in or
paid inference were used for this implementation. MIT is selected; eligibility
remains unproven. Read the [reviewed contract](README.md).

## Explicit account selection

`harness` is the product-name constant used for CLI and first registration.
These commands are available on Windows. Run login only when intentionally
authorizing this application; it opens the system browser and starts a five-minute
loopback transaction. It does not make an inference request.

```powershell
harness auth login chatgpt --profile personal
harness auth status --profile personal --json
harness models --provider chatgpt --profile personal --json
harness auth refresh chatgpt --profile personal
harness auth logout chatgpt --profile personal --json
```

Profiles are local registration labels (ASCII letters/numbers, `_` and `-`, up to
64 characters; reserved Windows device/internal names are rejected). Use a new
profile for another account or workspace. Return to the same profile after
logout or a terminal/uncertain refresh; the issued client ID remains saved.
New registration uses `dynamic_agent_client` only in browser authorization, never
as the token-exchange client. Callback-issued registration is saved before code
exchange, so a rejected code can be replaced through a fresh transaction using
that issued ID. Email is neither an identity nor a workspace identifier.

If direct-use permission was not granted, identity can remain signed in but
inference is disabled. Explicitly request consent with:

```powershell
harness auth login chatgpt --profile personal --enable-plan-usage
```

## Model and local limits

The account catalog supplies model slugs/display names, not context capacities.
`models` preserves visible ordering and returns the verified registration and
retrieval time. The adapter refetches on construction; it does not reuse another
account's catalog or infer capabilities from an API-key catalog.

Supply non-secret JSON via `--config` with exactly these fields:

| Field | Meaning |
| --- | --- |
| `profile` | Saved local registration label |
| `model` | Exact visible account catalog slug |
| `context_tokens` | Qualified host admission capacity, 4096 through 2000000 |
| `response_headroom` | Positive local reserve below that capacity |
| `limits_source` | Nonempty source/qualification provenance for this exact slug |

There are intentionally no guessed model limits or sample values presented as
qualified. Unknown fields are rejected. `--model` must match the configured
limits' model; `--profile` can select another registration for a new session.
The CLI's `--context-tokens` limit also applies; admission uses the smaller host
and adapter capacity. Model limits and provenance are saved in attempt metadata.

```powershell
harness ask --workspace . --provider chatgpt --config chatgpt.json "Explain README.md"
harness ask --workspace . --provider chatgpt --config chatgpt.json --session 1 --resume-turn 1
```

Session reuse requires the same `(issuer, issued client_id, verified subject)` and
exact model/protocol. Renewed tokens do not change that binding. Another
registration is rejected even if it belongs to the same person. Local admission
limits can change on explicit resume without discarding history or consumed
attempts. No account, model, provider or billing fallback exists.

Requests use only `model`, array `input`, `instructions`, namespaced function
`tools`, `store:false`, `stream:true`. No upstream output-token cap is promised.
The engine owns request/tool/time/output/context limits. All completed output is
retained in original order; encrypted reasoning comes from completed done items
and must reconcile with the terminal response. Only `response.completed` permits
tool execution. Results use provider call IDs, and the same shared engine closes
the complete batch before continuation. Unexpected executable/custom/hosted-tool
content fails closed. Actual route/model function support still needs #9.

## Protection, refresh and logout

Production state is outside session SQLite under the local application data
directory's `z-harness/credentials.chatgpt` folder. Current-user Windows DPAPI
encrypts the complete profile record. A protected DACL grants that user full
control with inheritance to children; no inherited broad principal is retained.
Reparse paths and nonlocal volumes are rejected. The existing scoped-tool
`credentials.*` denial covers traversal into this folder. This is OS-backed
at-rest protection, not a sandbox against another process running as the user.

Profile OS file locks serialize renewal/logout and credential-bearing requests.
Atomic same-directory replacement uses a flushed stage and Windows write-through
rename. Before sending a refresh, the saved generation becomes unusable. A
successful, validated response atomically replaces all token metadata; malformed,
lost, cancelled or unpersisted responses require reauthorization. Known transient
OAuth error responses restore the prior generation and permit an explicit retry.
`earliest_refresh_at` is retained verbatim; its undocumented scheduling semantics
are not assumed. Expiry is taken from the actual response, with a 60-second
refresh margin. No token, callback code, verifier or authorization URL is logged.

Logout publishes a durable generation that invalidates older sign-ins and
requests, then obtains the profile lock and clears the token record before remote
revocation. In-flight inference observes the generation independently of UI
delivery. A process exit after the marker cannot reenable the old generation.
Interrupted cleanup can be finished by rerunning logout. Revocation retries are
bounded; failure or cancellation explicitly leaves remote revocation unconfirmed
and links [ChatGPT Settings](https://chatgpt.com/settings). Local history,
registration identity and installation host ID remain.

HTTPS clients disable redirects and inherited proxies, and mark authorization
headers sensitive. Origins are fixed. Discovery endpoints must match the trusted
production issuer and allowlist. Time limits are 10 seconds to connect, 30 seconds
idle read, 120 seconds overall HTTP, and five minutes for browser authorization.
The engine adds its existing turn deadline/cancellation. Live proxy customization
is not exposed in M1b. The browser is invoked through the native shell API with
no command interpreter. Loopback fixtures cannot open a browser, read the live
vault, or use a remote endpoint; fixture/live vault markers cannot be mixed.

## Recovery and usage

A 401 admission failure gets at most one serialized refresh/newer-generation reuse
and one engine-counted retry. A second 401 suspends for diagnosis rather than
looping OAuth. Quota, consent, access restriction and uncertain/terminal credentials
have separate recovery reasons. Provider-only suspended recovery may be explicitly
resumed; it never authorizes patch/command uncertainty. Transient inference
responses use the engine's existing two-retry ceiling and usable Retry-After.
Unknown codes stop; no allowance/reset time or eligibility denial is inferred.
Sanitized status/code/parameter/request ID/phase and raw token-usage snapshots are
available in persisted attempt metadata and JSON output. Missing usage is unknown.

`ask` displays the bound registration/model and `chatgpt-plan` route. Token counts
do not establish remaining plan allowance or money. Manage app limits and
ChatGPT-managed credit permissions in [Usage](https://chatgpt.com/settings/usage).
No purchase or credit enablement is performed. The parent retains the separate
$3 total limit for any future DeepSeek V4 Flash qualification; this worker spent
nothing and did not access that credential.

## Integration with the patch worker

Auth and Responses live in separate `auth/` and `provider/chatgpt/` modules.
Shared changes are additive provider exports, diagnostic/context-budget fields on
`Progress`, six provider stop reasons, retry admission for a refreshed account,
and a narrowly restricted SQL resume path for provider-recovery reasons. There is
no schema migration and no edit to tool execution, workspace scope, approvals,
native launcher or patch algorithms. Preserve the patch worker's handling of
other `suspended_recovery` reasons when reconciling `journal.rs`/`turn_loop.rs`.
CLI command/config branches are additive; manifests add only the auth dependencies
and Tokio/reqwest/Windows features needed by these modules.
