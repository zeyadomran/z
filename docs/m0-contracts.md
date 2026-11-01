# M0 contracts

M0 is a credential-free scripted turn, not a coding-agent release. No tool,
command, network provider, authentication, approval, or recovery action runs.
The two crates are `harness-core` (ownership and storage) and `harness-cli`
(input/presentation). Both OpenRouter and direct ChatGPT plan access remain
required release gates.

## Durable ownership

One engine owns one active coordinator. SQLite runs on its own blocking worker;
no transaction crosses a provider await. A prompt and running attempt are
committed before the turn handle is returned. Completed assistant content,
attempt outcome, turn outcome, and durable events commit together before the
terminal outcome is published. Unknown usage is NULL, never an invented zero.

Sessions bind canonical workspace, provider, account reference, model, and
serialized provider options. Credentials never belong in these records. M0's
only binding is `scripted` / `local` / `script-v1` / `{}`. New provider/account/
model/options require a new session; reopening must match the binding.

Per-session event sequences are allocated transactionally. Observer signals
only advertise the newest durable sequence; consumers query events after their
last sequence. Notifications may coalesce. Terminal results have a separate
retained channel. Paused observers cannot block work. Dropping a turn handle
requests cancellation. Explicit shutdown requests cancellation, joins the turn,
then closes and joins SQLite. Dropping the engine signals cancellation but is
not an asynchronous cleanup guarantee. A completion already committing may win
a cancellation race; a persisted completion is never rewritten as cancelled.

SQLite uses DELETE journaling, EXTRA synchronous mode, foreign keys, immediate
write transactions and a 5-second busy timeout, verified after configuration.
Unresolved-turn indexes exclude concurrent active work in the same database by
workspace and session. They do not replace the OS workspace lock required in
M1a, including across distinct data directories. A lock/busy failure never
reexecutes a provider request. M0 has no external effects to reconcile.

## Turn and attempt transitions

`running` may reach terminal `completed`, `failed`, `refused`, `cancelled`, or
`suspended_limit`. Only a complete valid response produces an assistant message.
Malformed/disconnected responses fail; refusal is not success. Local deadline
or output exhaustion suspends and retains the accepted prompt. M0 cannot resume
or abandon suspended work, so it blocks new work in that workspace; use an
owned disposable data directory for limit experiments.

Later slices also use `awaiting_approval`, `suspended_recovery`, `interrupted`.
All five unresolved states keep workspace/session exclusion. Persist limits,
attempt count and consumed active time; human waits must not reset budgets.
No suspended turn may retain a running command.

Crash takeover belongs to M1a/M4: acquire the OS lock, then atomically mark
orphaned running work interrupted. Opening a database alone cannot establish
orphanhood and M0 never clears a running record or replays it. Unknown command
effects block the workspace until explicit reconciliation/abandonment. A new
session must not bypass an unresolved turn.

## Batch contract for subsequent migrations

Batch states are `pending`, `suspended`, `closed`. A batch owns its immutable
completed assistant message, epoch, ordered provider call IDs and local call
IDs. Provider IDs and local attempt IDs are different identities. Only one
unclosed batch may belong to a turn. Calls progress through `proposed`,
`awaiting_approval`, `authorized`, `intent_recorded`, `running`; resolved calls
are `succeeded`, `failed`, `cancelled`, `denied`, `stale`, `rejected`, `skipped`,
or explicitly `abandoned`. `unknown` is unresolved and cannot be sent as success.

Persist intent before effects, then each durable result before proceeding.
After every call has a truthful result, atomically close the batch and append
the full provider-facing result sequence. No new prompt/request or compaction
can cross an open batch. Resume reuses results, never reruns completed effects.
Approvals bind session/turn/call/workspace, normalized action digest, policy
version and file expectations; restart requires fresh validation and approval
identity. Native Windows creation-time Job Object membership, independent pipe
draining and confirmed cleanup remain M3 requirements.

M0 does not create unused approval/artifact/execution tables. Those migrations
must add these ownership keys and constraints with the corresponding vertical
slice. The scripted protocol validator already rejects missing, duplicate,
foreign or interleaved tool results and changing an epoch's frozen input.

## Epochs and schema evolution

Epoch states are `candidate`, `active`, `retired`, `rejected`. One active epoch
per session freezes exact prefix and full tool-schema serialization. SQLite
rejects changes to either. M0 stores an empty schema because it has no tools.
Later policy modes retain all tool definitions and enforce permission in code.

Compaction can only prepare a candidate between closed batches, preserve the
original epoch/messages and validate its first response before atomic promotion.
On failure reject the candidate and retain the old active epoch. Never transplant
opaque or signed provider items beneath a changed prefix. Provider adapters must
store original wire content alongside normalized content before live inference.

Schema v1 uses an application ID, `user_version`, and a contract marker. An
unknown newer version, nonempty unversioned database or foreign application is
rejected before journal/schema writes. Initialization is transactional and
rechecks identity under an immediate transaction. There is no older owned
schema to migrate in M0. Every future migration must use SQLite's online backup
interface first, preserve a consistent recoverable backup, and update schema
and version together. Never downgrade, overwrite, or copy a live database file
as a backup. Storage failure does not acknowledge completion; a still-running
durable record remains blocked for later recovery.

## Required provider mappings

OpenRouter accepts only its full documented terminal contract: `stop` gives a
final response; `tool_calls` requires all valid complete calls; `length`
suspends without execution. Content refusal, errors, unknown finish reasons
and EOF are not success. ChatGPT accepts only `response.completed`; refusal
content remains refusal, while incomplete/failed/disconnected streams cannot
authorize tools. Account, credits, quota, transport and protocol failures need
distinct adapter outcomes; only recognized transient failures may retry. No
automatic account, model, provider or billing-route switch is allowed.

As checked on 2026-11-01, OpenAI documents plan usage for open-source/local apps
with their own registration and stable opaque host ID. Paid/remote products use
a separate interest route. This repository has no declared open-source license;
local development fits the documented local-app category, but distribution
eligibility and intended account access are still unproven. M1b must perform
the app's own sign-in, direct inference and custom-tool continuation; no existing
app registration or API credits may stand in. No credentials were accessed.

Sources: [overview](https://developers.openai.com/siwc/token-sharing-open-source),
[registration](https://developers.openai.com/siwc/token-sharing-open-source/sign-in),
[preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations).
