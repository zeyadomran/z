# Testing M2

Run from the repository root with the pinned toolchain:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
node --test scripts/git-hooks/verify.mjs
git diff --check
```

After staging, run `node scripts/git-hooks/check-staged-file-size.mjs`.
Never bypass hooks or branch protection. All seven required CI contexts remain
unchanged. The Rust Windows job retains the credential-free scripted CLI smoke.

## Canonical flow and failure owners

- `engine_flow.rs` owns prompt -> real read/search -> durable closed multi-call
  batch -> approved CRLF/BOM edit -> LF create/edit -> final answer -> shutdown
  -> reopen -> next prompt. Native fixtures verify protected non-default DACL
  and hidden/archive/not-content-indexed attributes. It checks
  metadata strings against independent native reads and edits a file under an
  explicit legacy no-`AI` parent, reproducing the hosted ACL-merge failure. It checks
  provider IDs, opaque continuation, sequential results, events and stable epoch input.
  Changing the file after the checkpoint does not replay completed reads. The
  initial added read failed with `Failed` instead of `Completed` against M0;
  implementation then made this flow pass.
- `patch_approval.rs` owns inspect/user denial, stale creates, restart approval
  invalidation, abandonment, malformed batches, mixed/ambiguous edits and size
  limits. `patch_recovery.rs` injects verified SQLite intent/staged/result aborts,
  kills an owned process after an explicit handshake, and reopens through the
  engine. Completed bytes plus metadata reconcile; changed bytes or ACLs require
  recovery. Abandon preserves known completed effects and unrelated artifacts.
  Reopen also accepts old trailing-NUL serialization padding without changing ACL
  semantics. An external native fixture materializes the replacement/restoration
  gap and verifies unknown recovery with original/intended evidence retained.
- `openrouter_wire.rs` owns the external adapter contract: real loopback HTTP,
  arbitrary chunk boundaries, comments, fragmented arguments, opaque reasoning,
  repeated terminal usage frames, full `[DONE]`, continuation IDs and routing.
  It covers midstream/account/limit/refusal failures, recognized-only retries,
  retry discard events, cumulative usage snapshots and incompatible catalog
  configuration. Catalog requests carry no credential; inference fixtures use a
  fixed dummy header and cannot access host secrets.
- `openrouter_resume.rs` owns credential replacement rejection and explicit
  provider token-cap increases across durable suspension/reopen. Host budget
  changes do not change account/model/route identity or reset consumed attempts.
- `provider_contract.rs` retains closed-pairing/frozen-epoch validation and
  incomplete/malformed/disconnected/refused/executable scaffold-frame failures.
  Refusal coverage retains audit messages while excluding the refused turn from
  subsequent provider input.
- `scope.rs` owns path escapes, ADS/device/trailing-dot paths, credential aliases,
  hard links, an actual owned Windows junction, common read/list/search exclusions
  and nested instruction provenance. It checks real files through the engine.
  Repeated nested discoveries deduplicate by epoch/origin/hash; scan omissions
  remain visible without exposing deliberately excluded credential paths.
- `storage_guards.rs` owns foreign/newer schema preservation, independent SQLite
  connection exclusion and immutable workspace/provider binding.
- `migration.rs` starts with the actual M0 schema and saved messages, upgrades
  through public engine opening, reads/continues the original session, and
  verifies the SQLite backup's version, integrity and original history.
- `recovery.rs` kills one owned subprocess after an explicit provider-entry
  handshake. A different data directory is blocked both before and after process
  death. Taking the OS lock marks stale work interrupted, and explicit resume
  retains attempt accounting. `crash_fixture_child` is that subprocess entry,
  not a placeholder feature test. This is separate from #12's process launcher.
  Recovery errors expose saved IDs and the original data directory; unavailable
  recovery stores fail closed without silently creating an empty replacement.
- `persistence_failure.rs` injects SQLite transaction aborts at intent/result and
  terminal writes. The exact failure is observed; no new model request follows
  failed persistence. Reopen closes unfinished inspections truthfully. An already
  stored final answer is recovered without replacing it. This is not disk-full
  or power-loss qualification.
- `limits.rs` owns input validation, output/active-time suspension, pre-send
  context limits and request/tool caps across explicit resume. Time is advanced
  only after a provider-entry handshake. Closed batches and consumed requests
  survive reopening.
- `cancellation.rs` owns cancellation, unread/dropped observers, shutdown and
  engine drop. Durable cleanup is independent of UI consumption.
- CLI `cli_flow.rs` owns credential-free abandonment of an unavailable provider's
  suspended turn, durable cancellation, a new scripted prompt, one final answer,
  and fresh-process session continuation in JSON mode. The output flow injects
  a blocked writer, verifies coalescing and terminal delivery, and distinguishes
  tool continuation from a discarded request attempt. Output-only patch resume
  exits approval-required with stdin left open, without a new model attempt.
- `windows_launcher.rs` retains the seven native ownership/cancellation/capture
  flows from the merged launcher prerequisite. See [its scope](windows-launcher.md).

All engine tests use real internal modules, temporary workspaces and SQLite.
Only the provider/network boundary, controlled time, owned process death or
explicit external database failures are controlled. No paid inference, mocks
of internal storage, arbitrary sleeps, stress loops or coverage-percentage gates.

## Qualification boundary

M2 local validation is on Windows x64/MSVC with Rust 1.99.1. The required
formatting, Clippy, functional suite, release build, existing hook tests,
whitespace and staged-size checks are run for the delivered head. Repository
CI is separate and must be read at the PR head; local checks do not assert CI
success. The seven required contexts and hooks are unchanged.

The release CLI smoke uses an owned fresh data directory, verifies a second
fresh-process invocation with the saved session ID, and checks JSON output.
Human output visibly escapes terminal/bidi controls. The stdout writer has a
coalesced pending snapshot and never owns engine cleanup; a slow writer alone
does not cancel work. A broken writer still signals observer loss.

No real OpenRouter inference, key access, paid model call, direct ChatGPT auth,
live-provider patch workflow, command execution, clean-install qualification, hostile-filesystem sandbox,
disk-full/power-loss behavior, or Linux/macOS runtime qualification is claimed.
The catalog-only fetch establishes metadata availability, not tool/inference
eligibility. The merged #12 native launcher remains separate from agent tool
execution; M3 still owns approvals, secret filtering, spools and persistence.

The **live OpenRouter gate remains open in [M6 #9](https://github.com/zeyadomran/z/issues/9)**:
latest user-selected DeepSeek V4 Flash only, secure key acquisition by the parent,
and a cumulative $3 ceiling including retries and unknown/in-flight spend. The
manual task must explain an owned fixture using actual read tools and continue
with the resulting provider IDs/reasoning. Both required providers must later
pass the full approved edit/verification/resume release workflow. See
[OpenRouter usage/configuration](openrouter.md) and the
[direct subscription contract](chatgpt-subscription/README.md).

See [supervised patch contracts and integration notes](supervised-patches.md) for
supported Windows metadata, retained artifacts, schema migration, and the exact
approval/recovery boundaries. Native fixtures are local evidence, not complete
Windows release or hostile-filesystem qualification.
