# OpenRouter scoped inspection (M1a)

M1a implements the offline-verified read-only loop for #3. Live qualification
remains **outstanding in [M6 #9](https://github.com/zeyadomran/z/issues/9)**.
Neither wire fixtures nor catalog access establish a working paid route.
Direct ChatGPT plan support is still required in M1b; this slice provides the
shared adapter interface but does not implement subscription authentication.

## Invoke

Build with the pinned toolchain on Windows/MSVC:

```powershell
cargo build --locked --release
./target/release/harness.exe ask --workspace . --provider scripted 'hello'
```

The scripted path is the credential-free CI smoke. It does not explain files
or call a model. Real inspection uses an explicitly configured OpenRouter route:

```powershell
./target/release/harness.exe ask --workspace . --provider openrouter `
  --config ./openrouter.json 'Explain this project using its source files.'
```

Example **non-secret** `openrouter.json`:

```json
{
  "model": "deepseek/deepseek-v4.1-flash-1017",
  "upstreams": ["deepseek"],
  "allow_fallbacks": false,
  "data_collection": "deny",
  "zdr": false,
  "max_output_tokens": 4096,
  "reasoning_effort": "high",
  "proxy": null,
  "credential_env": "OPENROUTER_API_KEY"
}
```

Before a live run, obtain the key through the secure secret interface and expose
it in the named process environment variable using the host's secure launcher.
Never put the key in JSON, CLI arguments, repository files, issues, or transcripts.
The adapter reads this explicit reference only after public capability discovery.
The environment reference is persisted; its value is never persisted or logged.
Rotating that variable changes the key behind the reference: OpenRouter offers
no authenticated account binding here. Use a new reference/session for a new
account; direct ChatGPT's validated account identity belongs to M1b.

The public catalog checked on 2026-11-01 listed the example model. Recheck the
latest eligible DeepSeek V4 Flash family snapshot and exact endpoint `tag` before
qualification; model names, endpoints and capabilities may change. The example
upstream is conditional on its presence in current endpoint metadata. The CLI
rejects absent tags, missing capabilities, unsupported reasoning efforts and
output caps exceeding endpoint limits. It never guesses when discovery fails.
Override the JSON's model/route with `--model <slug> --upstream <endpoint-tag>`;
that override disables fallback. Unknown JSON fields fail rather than being ignored.

For this project's manual qualification, only the user-selected latest DeepSeek
V4 Flash route is authorized. The parent owns secure key acquisition and the
**cumulative $3 ceiling across all testing**, preferably using a provider-enforced
capped key. Include retries, unknown and in-flight usage conservatively; stop
before the ceiling. No other model or billing route substitutes for that gate.
No inference, key access, or paid calls occurred during this implementation.

## Limits and continuation

Defaults: 30 model attempts (including retries), 60 inspection calls, 20 minutes
of active work, 256 KiB cumulative model/tool output, 512 KiB serialized request,
and a conservative 128,000-token local context ceiling. OpenRouter reserves
4,096 output tokens by default. Budgeting uses one token per UTF-8 byte plus
framing/schema headroom and response reserve; it does not claim tokenizer accuracy.
The lower configured/catalog context limit wins. No compaction is implemented.

CLI host controls: `--max-requests`, `--max-tools`, `--active-seconds`,
`--max-output-bytes`, `--context-tokens`. `Config` also exposes request/prompt byte
caps. Neither model output nor repository instructions can raise any limit.
`--policy inspect` and `--policy supervised` currently permit the same scoped
read tools. Patches, commands and approval execution remain later milestones.

A completed session continues with `--session <id>`. Resume a suspended or
interrupted turn using the same provider config and:

```powershell
./target/release/harness.exe ask --workspace . --provider openrouter `
  --config ./openrouter.json --session 1 --resume-turn 2 --max-requests 40
```

No new prompt accompanies `--resume-turn`. Consumed requests/tools/output/time
survive restart; raising a limit is an explicit host action. Calls skipped by a
limit or cancellation receive truthful results, so the next request always sees
a closed batch. Completed results are never rerun. A provider may propose a new
read after receiving a skipped result. Older M0 sessions remain readable and
continuable under their original scripted binding and frozen empty tool schema;
create a new M1a session to use inspection tools.

Normal stdout streams escaped text. JSON mode (`--json`) emits version 1 NDJSON
`accepted`, coalesced `progress` snapshots, and a durable `terminal` record with
per-attempt usage. It is output-only. Terminal/bidi controls are escaped in human
output; model reasoning is preserved for continuation, not displayed. A blocked
or broken stdout cannot block engine cancellation, database work or cleanup.
Progress delivery is lossy; completed records and engine event queries are
canonical. Final output acknowledgment has a two-second bound after shutdown.

Exit codes: 0 completed, 1 failure/refusal, 2 argument usage, 3 suspended limit,
130 cancelled. Missing usage remains unavailable, never zero. The CLI reports
raw per-attempt usage; it does not enforce a dollar budget or promise upstream
cancellation/refund. Local cancellation ends consumption and future tool work.

## Failure outcomes

| Outcome | User action |
| --- | --- |
| Authentication/permission | Check the explicit credential reference, key permissions and account/key budget. |
| Credits | Restore authorized OpenRouter credits/key budget; no billing fallback occurs. |
| No upstream | Recheck exact endpoint tags, required parameters and privacy settings. |
| Throttle/transient server/transport | At most two retries per response; Retry-After seconds/dates are honored within the turn deadline. Prior partial display is discarded. |
| Context/output/request/tool/deadline limit | Explicitly raise the applicable host limit and resume; context compaction is later work. |
| Refusal/content filter | Request is refused, with the prompt retained for audit. |
| Invalid request/protocol/unknown error | Correct configuration/protocol; deterministic errors are not retried. |
| Abrupt EOF | Incomplete transport result; partial calls never run. |
| Persistence failure | No further inspection/request is scheduled. Reopen after fixing storage and explicitly resume interrupted work. |
| Workspace busy | Finish/resume the unresolved turn in its original store; a new session or data directory cannot bypass it. |

## Data, scope and storage

Context goes to OpenRouter and the explicitly selected upstream. No telemetry.
`data_collection` and `zdr` are explicit routing restrictions, not local promises
about third-party retention. The client disables redirects and implicit proxies,
marks authorization sensitive, and fixes the live credential origin to
`https://openrouter.ai/api/v1`. Connect/read/overall HTTP deadlines are 10/45/180
seconds. Metadata is cached in memory with retrieval time for at most one hour;
reopen to refresh. No stale catalog capability guesses are used. The loopback
fixture constructor accepts no credentials and uses a fixed dummy header.

Tools: `list_files`, literal `search_text`, and UTF-8 `read_file`. Paths must be
normal workspace-relative components. Device/traversal/ADS/trailing-dot paths,
common credential paths, reparse points and hard-linked files are denied.
Windows reads check the opened handle's final path and link count. Listings and
searches share exclusions and do not follow links. Generated/VCS directories are
ignored by default. No hostile-filesystem sandbox guarantee is made.

Reads hash the whole raw file (SHA-256), cap files at 1 MiB, normalize CRLF/BOM for
text presentation and return bounded line ranges. List/search pages contain at
most 200 entries; scans cap 10,000 entries, depth 16, five seconds of traversal,
and 8 MiB searched text. Results cap 64 KiB; instruction content caps 32 KiB per
result. Oversized operations return an error directing the model to narrow scope.
Root/user instruction snapshots freeze each epoch. Applicable nested AGENTS.md
snapshots carry origin/hash/content in tool results; they cannot mutate the
prefix or grant permissions. `--instructions` explicitly loads user preferences.

Store data outside the workspace in a **private local directory**, normally
`%LOCALAPPDATA%/z-harness`. Do not choose shared, network or cloud-synchronized
storage. File permissions inherit the selected directory's ACL; the application
does not provision a hardened OS account or sandbox. Session storage and migration
backups contain prompts/source context and must be protected accordingly.

SQLite v2 adds protocol messages, call intents/results, counters, instruction
snapshots and closed-batch checkpoints. Upgrade takes a consistent SQLite backup
before a transactional migration, retaining M0 sessions/events/prefixes. Each
migration attempt uses a fresh backup; unknown schemas fail without schema writes.
No downgrade is supported. Backups contain no provider credential values but do
contain any source or private content the user supplied in conversation.

Workspace locks live in the stable per-user local lock directory, independently
of `--data-dir`. The lock records the last store; unresolved work in that store
blocks a different store after the OS lock is released. A missing referenced
store fails closed: restore the original store for recovery. Do not manually
remove lock records while work is unresolved. Holding the OS lock permits stale
read-only reconciliation, never automatic replay. Finished results are reused;
unfinished reads close with interrupted-result errors. Unrecorded stream time
is conservatively charged from the persisted wall-clock start (with bounded
clock-jump handling). A crashed in-flight stream reserves its remaining output
budget because its actual consumed output is unknown; explicitly raise the
output cap to resume that turn. M2/M3 must add their own effect reconciliation before use.

## Verified upstream contracts

Checked 2026-11-01 against primary sources:

- [OpenRouter streaming](https://openrouter.ai/docs/api/reference/streaming):
  fragmented SSE, comments, terminal finish plus `[DONE]`; usage can repeat finish.
- [Reasoning preservation](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens):
  retain reasoning detail objects in stream order without changing opaque data.
- [Tool calling](https://openrouter.ai/docs/guides/features/tool-calling) and
  [routing](https://openrouter.ai/docs/guides/routing/provider-selection): matching
  tool_call_id, required parameters, explicit eligible routes and privacy options.
- [Typed errors](https://openrouter.ai/docs/api/reference/errors-and-debugging):
  classify documented types/statuses, not provider error-text guesses.
- [Model catalog](https://openrouter.ai/docs/api/api-reference/models/get-models)
  and the public `/api/v1/models/{model}/endpoints` metadata establish configured
  capability ceilings only; they do not establish inference eligibility.
- [reqwest client](https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html),
  [Windows final paths](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew),
  [Rust file locks](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock)
  and [SQLite backup](https://www.sqlite.org/backup.html) guide native boundaries.
