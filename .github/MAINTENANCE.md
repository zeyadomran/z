# Repository maintenance

## Merge policy

Use PRs for all changes after bootstrap, including administrators. Squash-only merging, automatic branch deletion, auto-merge, and branch updates are enabled. Main requires up-to-date checks, resolved conversations, and stale-review dismissal. Force pushes and deletion are blocked. Zero approving reviews are required for this solo-maintainer repository.

Required contexts: `Workflow validation`, `Git hooks (ubuntu-latest)`, `Git hooks (windows-latest)`, `Dependency review`, `Analyze (actions)`, and the separate `CodeQL` findings check. Re-read actual protection and current-head checks before each authorized merge.

## Checks and security

Actions use immutable SHA pins and checkout does not persist credentials. Workflow tokens default to read-only and cannot approve PRs. Only CodeQL has security-events upload permission. CI runs on every PR and main push without path filters. Hook tests use isolated repositories on Windows and Linux. Dependency review rejects introduced high/critical vulnerabilities on PRs. CodeQL analyzes Actions and scans weekly. Default CodeQL setup stays disabled to avoid duplicate analysis.

Enable Dependabot vulnerability alerts and security fixes, secret scanning and push protection, and private vulnerability reporting. Configuration and passing checks do not prove that the future harness is secure.

## Dependency updates and Rust activation

Dependabot checks pinned Actions weekly on Monday at 09:00 America/Toronto, groups updates, and uses Conventional Commit prefixes. Review and validate each update; updates are not automatically merged. No npm dependencies or Cargo manifests exist yet.

With the first Cargo workspace, commit Cargo.lock, pin the supported Rust toolchain, add Cargo Dependabot updates, and add mandatory Windows formatting, Clippy, build, and canonical functional checks. Extend CodeQL only with supported languages/configuration verified at that time. Register required contexts only after they actually run. Preserve the existing hook suite rather than adding duplicate tests.
