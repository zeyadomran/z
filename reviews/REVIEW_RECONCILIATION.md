# Rust harness plan review reconciliation

Status: both independent reviews complete; all findings have an author disposition below and the working plan is revised v2. No implementation has been started. Neither reviewer has re-reviewed v2 or its later provider requirements.

## Review input and independence

Both reviewers received `reviews/RUST_HARNESS_PLAN.v1.md`, SHA256 `6AA92D87DC0E5224D118ABBB3DCBD5A57C9106A620D14B0E195176B8C2819330`, with the same review brief. Neither was asked to read the other's output or edit the plan.

- GPT-6.1 Sol: `reasoningEffort=high`, `serviceTier=priority` (requested fast setting); complete review in `SOL_REVIEW.md`.
- Claude Opus 5.5: `effort=high`, `fastMode=false`; complete review in `OPUS_REVIEW.md`.

## Sol findings

- **SOL-01, P1 — accepted.** Section 6 now requires creation-time Job Object membership, avoiding the create-then-assign crash gap. M3 qualification explicitly covers parent termination during launch and parent-job compatibility.
- **SOL-02, P1 — accepted.** Section 4 now defines a durable pending assistant tool batch, provider call IDs, complete next-message results, suspension/resume, and truthful outcomes for unexecuted or uncertain calls. The plan retains multi-call responses with sequential execution; it does not depend on a single-tool-call model setting.
- **SOL-03, P1 — accepted.** Section 2 separates cancellation and supervision from UI queues, defines observer disconnection and explicit shutdown, and makes durable state queryable despite delivery loss. M3 qualification includes a blocked or dropped observer.
- **SOL-04, P2 — accepted.** M1 now owns workspace locking, stale-state reconciliation, credential sourcing, diagnostic redaction, and terminal escaping before their first use. M3 owns child-environment scrubbing. M4/M5 no longer introduce these prerequisites late.
- **SOL-05, P2 — accepted.** Section 5 resolves executable/interpreter paths before approval, binds the effective launch form, and treats batch shims as explicit interpreter execution or rejects them. M3 includes a safe fixture for spaces and batch handling.
- **SOL-06, P2 — accepted.** Section 6 defines completion across the managed tree, descendant cleanup reporting, and withholding workspace reuse if shutdown is unconfirmed. Following OPUS-05, normal root exit now gets a short fixed drain/settle window rather than waiting the original command deadline.
- **SOL-07, P2 — accepted.** Section 5 requires supported metadata preservation, documented native replacement behavior, no ignored ACL merge errors, and metadata-aware reconciliation. M2/qualification includes non-default ACL behavior.

Additional nonblocking notes incorporated: pin provider protocol options; merge cumulative usage updates rather than summing snapshots; reject unsupported executable content; budget complete tool-result batches; prioritize current user requirements. These refine existing modules without introducing new providers, sandbox infrastructure, or a UI framework.

Sol's verdict on v1 was conditionally ready for M0, with corrections required before the affected milestones. The author has incorporated the corrections; that does not mean Sol reviewed or approved the revisions.

## Subsequent user requirement: OpenRouter

The user required OpenRouter support after the frozen draft was sent for review. The working plan now makes OpenRouter Chat Completions the initial required adapter, replacing the provisional direct Anthropic Messages choice. Updated sections cover routing/capability validation, streamed tool arguments, reasoning-state preservation, credit/rate-limit failures, model metadata, and the correct `role: tool`/`tool_call_id` continuation format. SOL-02's durable batch invariant still applies, but its Anthropic-specific wire encoding no longer describes the first adapter. M1 acceptance now explicitly covers OpenRouter. The immutable v1 and reviewer reports remain untouched; neither review is represented as approval of this later provider change.

The user subsequently made direct ChatGPT plan access required too. The working plan now requires both providers in v0.1, moves subscription integration into section 4.2, adds a dedicated auth module and M1b milestone, specifies provider/account-bound resume and quota behavior, and makes successful subscription-backed coding qualification a release gate. API credits or a Codex wrapper do not satisfy that requirement. Eligibility is unverified; an external access failure remains an explicit release blocker while independent work can proceed. No credential was requested or accessed, and no inference call was made. This added requirement also postdates the reviewers' frozen input and has not been independently reviewed.

## Opus findings

Opus's verdict on v1 was not ready as written, but ready after targeted revision without re-architecture. It raised 25 findings, including 12 classified as blockers to particular milestones. The findings below are recorded in ID order; their original severity and full evidence remain in the unchanged report.

- **OPUS-01 — accepted, overlapping SOL-02.** Sections 4/7 now require closure of every tool batch, atomic provider-facing closure, truthful skipped/abandoned outcomes, and no repeated completed effects. The first adapter now uses OpenRouter tool messages; direct Anthropic user-message encoding is superseded.
- **OPUS-02 — accepted as a protocol-preservation design, with route-specific claims qualified.** Sections 4/8 freeze epoch prefixes/tool schemas and retain original provider messages; nested instructions append as data. The review's Anthropic account-date enforcement and beta settings are not assumed to apply through OpenRouter or ChatGPT. Actual opaque-state requirements remain adapter qualification gates.
- **OPUS-03 — accepted with a simpler compaction design.** Section 8 creates a fresh summary epoch, preserves originals separately, and validates the first candidate response before promotion or tool execution. Old signed/opaque blocks are not transplanted beneath a changed prefix. This avoids relying on provider-specific keep-tail stripping heuristics.
- **OPUS-04 — concern accepted; suggested implementation rejected.** Stable-Rust native launch is now an explicit first M3 spike. Keep creation-time job membership using CreateProcessW/STARTUPINFOEX. Suspend/assign/thread enumeration would retain SOL-01's parent-death gap. The plan does not depend on unverified `process-wrap` behavior or nightly std methods. Argument quoting, handle inheritance, and cancellable pipes require actual fixture proof.
- **OPUS-05 — accepted with an explicit result distinction.** After root exit use a short drain/settle window then terminate remaining owned descendants. Record root status and `completed_with_cleanup`, rather than claiming all child work succeeded from root exit zero. This addresses both long hangs and silent descendant abandonment.
- **OPUS-06 — accepted.** Windows no-window execution uses forced Job Object termination and reports possible partial effects. Graceful signaling is not promised on that path; later Unix qualification can use its own signaling implementation.
- **OPUS-07 — accepted.** Section 5 specifies PATH/PATHEXT resolution without implicit workspace lookup and an explicit versioned PowerShell form with no profile, noninteractive mode, and encoded exact script. Batch shims require that visible script form; unsafe argument forms fail clearly.
- **OPUS-08 — accepted, extending SOL-04.** M1a now explicitly includes credential-file denial, path checks, pre-send budgeting, and usage capture. M3 includes output spooling/retrieval and head/tail previews. Locking, display protection, and child-secret scrubbing remain before first use.
- **OPUS-09 — accepted.** Section 7 defines turn states, orphan takeover while holding the OS lock, unresolved-turn exclusion across a workspace, suspension/resume, and budget persistence. Section 9 specifies interactive approval and output-only JSON. Human wait time is excluded from the active-work budget.
- **OPUS-10 — accepted using the review's rollback alternative.** Section 7 removes the single-writer-process assumption, specifies DELETE/EXTRA, short immediate transactions, bounded contention handling, and consistent backups. A transient busy error retries result persistence without reexecuting the command. WAL is not required for this MVP.
- **OPUS-11 — accepted in terms of the required providers.** Section 4 explicitly maps OpenRouter finish/error states and ChatGPT Responses terminal states, account/quota failures, context overflow, validation errors, and retryable conditions. The direct Anthropic stop-reason table is superseded. Refused prompts remain in audit history rather than being silently erased.
- **OPUS-12 — accepted.** Section 5 defines whole-file raw-byte hashes, normalized text views, exact LF-view matching, restoration of CRLF/BOM, must-not-exist creation, mixed-newline rejection, large-file restrictions, and concurrent-read conflicts. CRLF enters the M2 gate.
- **OPUS-13 — accepted.** Section 5 separates raw per-attempt spools from ephemeral display events; final artifact metadata/outcome commit follows flush. Crash output remains explicitly partial/unverified. M3 owns this behavior.
- **OPUS-14 — accepted.** Section 5 adds Windows path-component rules, final-handle scope checks, resolved-name credential denial, non-following traversal, and consistent read/search/list policy. Implementation still must prove OS-level behavior; no hostile-filesystem sandbox guarantee is claimed.
- **OPUS-15 — accepted.** Approval contracts distinguish patch hashes from command launch specifications, include timeout/environment, show escaped controls without misleading truncation, and deny built-in writes to `.git` control paths.
- **OPUS-16 — accepted, already covered by SOL-03.** Cancellation and supervision do not depend on UI queues. Section 9 now explicitly treats broken stdout as observer loss and cleanup.
- **OPUS-17 — accepted.** The scripted provider validates protocol invariants, crash coverage uses synchronized owned subprocess termination, and persistence failure uses a verified narrow injection with real SQLite. Scope/approval and migration/locking are separate flows. `query_only` is not claimed to simulate disk-full.
- **OPUS-18 — accepted with route-specific accounting.** Hard budgeting starts in M1a/M1b; prompt/cache accounting follows each adapter rather than copying Anthropic counters. Before compaction exists, overflow is recoverable context-limit suspension.
- **OPUS-19 — accepted as capability-gated cost behavior.** Stable prefixes, cache fields, and verified supported options are in scope. Direct Anthropic cache markers and the review's illustrative prices are not copied into both adapters; unsupported subscription cache fields remain excluded. No savings are promised without measurement.
- **OPUS-20 — accepted.** Section 3 disables credential-bearing redirects, distinguishes connect/read/overall deadlines, marks sensitive headers, and makes proxy use explicit. Browser OAuth navigation remains a separate flow.
- **OPUS-21 — partially superseded; general safeguards accepted.** Direct Anthropic version/effort/thinking-display defaults no longer define v0.1. Persist model and option identity, prohibit silent model changes, pin only documented endpoint versions, emit retry-discard events, and show progress independently of exposed reasoning.
- **OPUS-22 — accepted, complementing SOL-07.** Record same-directory staging ownership, bounded sharing-violation retries, supported metadata preservation, and intent-based orphan cleanup. Never glob-delete presumed leftovers or ignore ACL errors.
- **OPUS-23 — accepted.** Section 8 names AGENTS.md discovery and gives the closest applicable project file precedence below explicit user instructions and engine policy.
- **OPUS-24 — accepted with a truthful decoding limit.** Preserve raw bytes; decode BOM-marked UTF-16 or valid UTF-8, otherwise use explicit code-page configuration or marked lossy output. Do not claim arbitrary encoding can be reliably inferred.
- **OPUS-25 — accepted.** Session command nouns are consistent and include deletion. Linux/macOS move to explicit post-v0.1 work. The payload/file-size distinction is documented, and configured-limit outcomes are resumable through user action.

## Deferrable recommendations

- **OPUS-D1:** reusable command approvals remain deferred; per-call approval is the v0.1 contract.
- **OPUS-D2:** eager input execution and provider-specific strict-tool options remain deferred; complete tool validation is mandatory regardless.
- **OPUS-D3:** server-side compaction, server task budgets, and refusal fallback models remain deferred. No automatic alternate billing route is introduced.
- **OPUS-D4:** a complete child-environment allowlist remains deferred. Provider-secret scrubbing is required; host commands retain the documented host privileges.
- **OPUS-D5:** convenience environment defaults can be added during command implementation when useful; they do not replace timeouts or approval.
- **OPUS-D6:** undo, stress infrastructure, TUI, and Linux remain deferred. The suggestion to defer a second provider is superseded by the user's explicit requirement for both OpenRouter and ChatGPT plan access in v0.1.

## Reconciliation outcome and verification limits

All 7 Sol findings and 25 Opus findings have a disposition. This is author reconciliation, not unanimous approval or independent validation of v2. Both reviewers evaluated the same immutable v1; the latest provider requirements were added afterward.

The remaining gates are explicit implementation work: stable-Rust Windows process/argument/pipe behavior, file and SQLite failure semantics, protocol qualification for each selected model/route, actual ChatGPT app/account eligibility, and the canonical functional/native release checks. No source claim establishes that those behaviors already work. No runtime tests were run for this documentation-only task.
