# Repository maintenance

## Merge policy

Use PRs for all changes after bootstrap, including administrators. Squash-only merging, automatic branch deletion, auto-merge, and branch updates are enabled. Main requires up-to-date checks, resolved conversations, and stale-review dismissal. Force pushes and deletion are blocked. Zero approving reviews are required for this solo-maintainer repository.

Required contexts: `Workflow validation`, `Git hooks (ubuntu-latest)`, `Git hooks (windows-latest)`, `Dependency review`, `Analyze (actions)`, the separate `CodeQL` findings check, and `Rust (Windows)`. Re-read actual protection and current-head checks before each authorized merge.

## Checks and security

Actions use immutable SHA pins and checkout does not persist credentials. Workflow tokens default to read-only and cannot approve PRs. Only CodeQL has security-events upload permission. CI runs on every PR and main push without path filters. Hook tests use isolated repositories on Windows and Linux. Dependency review rejects introduced high/critical vulnerabilities on PRs. CodeQL analyzes Actions and scans weekly. Default CodeQL setup stays disabled to avoid duplicate analysis.

Enable Dependabot vulnerability alerts and security fixes, secret scanning and push protection, and private vulnerability reporting. Configuration and passing checks do not prove that the future harness is secure.

## Dependency updates and Rust activation

Dependabot checks pinned Actions and Cargo dependencies weekly on Monday at
09:00 America/Toronto, groups updates by ecosystem, and uses Conventional
Commit prefixes. Review and validate each update; updates are not automatically
merged. There are no npm dependencies. Cargo.lock records the executable's
resolved dependency graph.

M0 adds the pinned Rust toolchain and the `Rust (Windows)` CI context for
formatting, Clippy, the public-engine functional suite, release build and a
credential-free CLI smoke. This context was added to branch protection after
its first successful run on 2026-11-01, preserving all six previous checks.
Preserve all existing required contexts and the hook suite. Actions CodeQL
remains unchanged; Rust source security qualification is not claimed.
