# Supervised patches (M2)

`apply_patch` edits or creates one scoped UTF-8 file. The tool schema is identical
in inspect and supervised sessions. Inspect denies writes; supervised mode
requires individual approval. Neither provider nor project instructions can
change policy. Provider/account/model binding and opaque context stay unchanged.

```json
{
  "path": "src/example.txt",
  "expected_sha256": "whole-raw-file-sha256-from-read_file",
  "old_text": "one exact match",
  "new_text": "replacement"
}
```

For creation, use `expected_sha256: "must_not_exist"`, empty `old_text`, and the
complete LF file in `new_text`. The existing parent must be in scope. There is
no deletion, rename, fuzzy matching, multi-file atomicity, or permission tool.
Empty replacements may remove text but never delete the file. An empty old_text
is an exact edit only when the existing LF view is also empty.

Reads hash the entire raw file, including BOM/CRLF bytes, even for a line range.
They compare bounded snapshots from verified handles and reject observed changes.
Edit matching uses the LF view, strips exactly one BOM, requires exactly one
match (including overlapping matches), and restores uniform newline style/BOM.
Mixed or lone-CR newlines, binary data, files/results above 1 MiB, and assembled
arguments above 256 KiB are rejected. New files use the supplied UTF-8 LF bytes.

Credentials, `.git`, harness data/artifacts, traversal, ADS, reserved devices,
reparse points, junctions, hard links, and ambiguous Windows path forms are denied.
Tildes are conservatively rejected to exclude short-name aliases. Listing/search
share these exclusions. Host mode is not isolation against hostile filesystem
races; scope, bytes, and metadata are rechecked before replacement.

## Approval and CLI

Run `ask` with `--policy supervised`. When an edit is proposed, the engine
durably returns `approval_required`/`awaiting_approval` and releases its OS lock.
The unresolved workspace remains reserved. No provider request can continue
until every result in the authoritative batch is resolved.

Human CLI mode with terminal stdin/stderr prints the complete action and offers
approve, deny, or cancel. JSON and non-TTY modes are output-only: they exit 2
without reading stdin or executing the pending write. Resume with the existing
CLI form:

```text
harness ask --workspace <workspace> --provider <same-provider> --config <same-provider-config> --session <id> --resume-turn <turn-id> --policy supervised
```

Omit `--config` for the credential-free scripted provider. Real providers still
require their original account/model/route. Approval waits do not consume active
turn time. Ctrl+C or observer loss cancels active work; an in-flight bounded
native replacement is joined and its actual outcome recorded.

The public engine API is `preview_patch(session, turn)` followed by
`decide_patch(approval_id, Approve | Deny)`. A preview binds session, turn, call,
canonical workspace, normalized arguments, original bytes/hash, result hash,
supported file/parent metadata, instruction snapshots, and policy version.
Previews are retained only inside that Engine instance. Each refresh has a new
random identity; durable approval audit records cannot authorize a restart.
The CLI escapes controls and all non-ASCII preview characters, without truncating
security-relevant fields. Stale bytes/metadata invalidate the decision and require
a fresh model proposal. Denial/staleness skips subsequent unstarted calls.

Complete batches are schema-validated before any effect. One malformed call
rejects all unstarted calls in that batch, with complete ordered error results.
Previously completed results remain intact during resume, cancellation, or abandon.

Exit outcomes: completed 0, failure 1, approval required 2, configurable limit 3,
interruption/recovery required 4, cancellation 130. Existing Clap usage errors
also use 2; use the versioned JSON outcome to distinguish them.

## Native replacement and reconciliation

Windows is the supported patch platform. Each attempt commits its full binding,
prior/new bytes and metadata, plus unique same-directory staging/backup paths,
before staging. Staging uses create-new, applies the original DACL before
writing source bytes, and flushes bytes. The engine durably
records expected metadata before attempting replacement. Existing targets use
`ReplaceFileW` with flags zero and a recorded backup; creates use `MoveFileExW`
without replace-existing and with write-through. Sharing/lock violations retry
for at most 500 ms. Resulting files are explicitly flushed and verified.

Supported existing-file metadata is owner/group, DACL (including inheritance
protection), and normal/hidden/system/archive/not-content-indexed attributes.
Owner/group must match the newly inherited staging owner/group; unsupported
preservation fails before replacement. Read-only, compressed, encrypted, sparse,
offline, reparse, and other unsupported attribute combinations fail closed.
New files inherit their parent's security metadata. SACL/audit preservation is
not qualified. No flag ignores ACL merge errors, and the unsupported
`REPLACEFILE_WRITE_THROUGH` flag is never used. See Microsoft's
[replacement contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).

On restart, original bytes **and metadata** request new approval; expected final
bytes **and metadata** record success without replay; anything else suspends as
`recovery_required`. Failed result persistence never triggers another effect.
`harness abandon` reconciles known completed patches first, then closes unresolved
effects explicitly as unknown/abandoned. It does not undo edits.

Recorded backups/staging evidence are retained, including after success. Built-in
tools exclude their reserved names. No automated artifact cleanup is implemented:
the engine never glob-deletes, resets the working tree, or removes similarly named
user files. Stored intents include source bytes; keep the local store private.
Retained artifacts consume disk space; power-loss durability and disk-full are
not qualified by transaction-abort fixtures.

## Integration and validation boundary

The canonical `engine_flow` now covers reads/search, approved CRLF+BOM edit with
a protected non-default DACL and non-default attributes, must-not-exist LF create,
LF edit, final response, shutdown, and reopened history with frozen opaque context.
`patch_approval` owns denial/staleness/restart and exact-edit/size rejection.
`patch_recovery` observes real SQLite intent/staged/result failures, signals an
owned child boundary, kills that child, and reopens through the engine. It covers
unchanged/applied/changed-byte/changed-metadata recovery and abandonment, retaining
a user-owned staging-like sentinel. These are bounded cases, not stress loops.
`scope` adds write denials; `cli_flow` checks output-only suspension with stdin
left open. Native ACL assertions use owned fixtures and require no credentials.

Shared integration points for parallel #4 work:

- `store.rs` recognizes schema 3; `migration_v3.sql` adds only `patch_attempts`.
  A consistent v2 backup precedes migration. Sequence any concurrent migration
  after this one and update recognized version/contract together. Frozen epochs
  are never rewritten; create a new session to advertise the added patch schema.
- `engine.rs` adds ephemeral preview ownership and `preview_patch`/`decide_patch`;
  `turn_loop.rs` processes pending batches before provider calls; `journal.rs`
  leaves interrupted patches unresolved and accepts approval/recovery resume.
- `context.rs` adds the same patch schema for every mode/provider. `workspace.rs`
  shares path/read validation. Provider adapter modules and opaque payloads are
  untouched; both adapters should reuse normalized `ToolCall` processing.
- `main.rs` adds approval handling and exit mapping; `approval.rs` owns terminal
  interaction. Retain #4 provider selection/auth changes when integrating it.

Repository checks are separate from hosted CI and runtime release qualification.
No real sign-in, credentials, paid inference, `run_command`, clean-install, or
Linux/macOS qualification occurred. Live provider coding qualification remains
[#9](https://github.com/zeyadomran/z/issues/9), with the parent-owned total $3
DeepSeek V4 Flash Latest limit unchanged. Command tools remain #6.
