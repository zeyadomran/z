# Testing M0

Run from the repository root using the toolchain pinned in rust-toolchain.toml:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
node --test scripts/git-hooks/verify.mjs
git diff --check
```

After staging intended files, run
`node scripts/git-hooks/check-staged-file-size.mjs`. Never bypass hooks.

## Canonical flow and failure owners

`crates/harness-core/tests/engine_flow.rs` owns prompt -> complete response ->
durable outcome/events -> shutdown -> reopen history -> next turn. It uses the
public engine, real SQLite and an owned temporary workspace/data directory.
The first development run failed at the completed-outcome assertion; the
implemented persistence flow then passed. Extend this test with actual tools,
approvals and execution when their milestones land; there are no placeholder
tool-feature tests.

The other tests cover distinct material failures:

- `cancellation.rs`: an explicitly gated provider with an unread observer;
  direct cancellation, observer loss, shutdown and engine drop. Observer loss
  joins through `wait_idle` without using shutdown to cause the cancellation.
- `provider_contract.rs`: closed call/result pairing and frozen epoch input;
  truncated/invalid/disconnected/refused/executable responses cannot become
  completed assistant messages. The tool frame is rejected, never executed.
- `storage_guards.rs`: foreign/newer schema bytes remain unchanged, and two
  independent SQLite connections cannot accept unresolved work in the same
  workspace. This is database exclusion, not OS lock or crash qualification.
- `limits.rs`: invalid settings/prompts have no accepted turn; output/deadline
  limits suspend without partial assistant content and survive reopening.
  Deadline time is advanced explicitly after a provider-entry handshake.

Fixtures inject only the provider boundary or prepare external database
conditions. There are no internal store mocks, sleeps, screenshot tests or
coverage-percentage gates. SQLite's settings and initial schema are exercised
through engine opening. Schema protection does not prove migration/backup,
disk-full, corruption recovery or durable effects under power loss.

## Actual M0 qualification

Local verification on Windows x64/MSVC, Rust 1.99.1:

- Formatting, Clippy with warnings denied, eight functional tests, and release
  build passed.
- The three existing repository-hook tests passed unchanged.
- Actionlint 1.7.12 validated the workflows; its official download was checked
  against the upstream release checksum.
- Release CLI smoke uses a fresh owned data directory and repeats with its
  saved session ID to exercise fresh-process continuation.

CI separately runs these Rust checks and a single CLI smoke on Windows,
preserving all repository maintenance checks. CI status belongs to the PR's
actual head; local results do not assert remote checks passed.

CLI success is exit 0, ordinary failure/refusal is 1, clap usage errors are 2,
suspended limits are 3 and cancellation is 130. Explicit shutdown completes
before final stdout; a broken output pipe returns an error with no active work.
Normal CLI output has no streaming presentation in M0.

No live OpenRouter/ChatGPT, credentials, command processes, file edits,
approvals, abrupt-crash takeover, storage failure injection, migration upgrade,
package installation, or Linux/macOS runtime qualification occurred. M0
supports reading completed history and continuing completed sessions. An
orphaned running or suspended turn remains blocked; recovery UX is later work.
