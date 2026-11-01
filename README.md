# z

A Rust coding-agent harness with engine-owned turns, scoped inspection and local
SQLite sessions. The first release targets Windows and requires both OpenRouter
and direct ChatGPT plan access.

**Status: M1a read-only implementation, offline verified. Live OpenRouter
qualification remains open in [M6 #9](https://github.com/zeyadomran/z/issues/9).
Direct ChatGPT authentication, edits and command execution are later slices.**

Credential-free smoke:

```powershell
cargo run -p harness-cli -- ask --workspace . --provider scripted 'hello'
```

OpenRouter inspection with explicit non-secret configuration:

```powershell
cargo run -p harness-cli -- ask --workspace . --provider openrouter `
  --config ./openrouter.json 'Explain this project using its source files.'
```

Read [OpenRouter configuration, limits and failure guidance](docs/openrouter.md)
for the JSON format, secure credential reference, model/upstream validation,
privacy options and manual qualification constraints. No credential is accepted
as a CLI argument. Scripted CI never calls a paid model.

Storage defaults to `%LOCALAPPDATA%/z-harness`, outside the workspace. Keep it
private, local and outside synchronized folders. `--data-dir` selects another
absolute local directory; it cannot bypass the workspace lock or unresolved
turn. `--session <id>` continues a completed conversation; `--resume-turn <id>`
explicitly resumes suspended/interrupted read-only work without resetting budgets.
`harness abandon --workspace <path> --session <id> --turn <id>` resolves saved
work locally without credentials or inference, preserving its audit history.

- [Implementation plan](RUST_HARNESS_PLAN.md)
- [Independent historical reviews](reviews/REVIEW_RECONCILIATION.md)
- [Contributing/setup](CONTRIBUTING.md)
- [Historical M0 contracts](docs/m0-contracts.md)
- [Tests and actual qualification limits](docs/testing.md)
- [Native Windows launcher prerequisite](docs/windows-launcher.md)
- [Dependencies/toolchain](docs/dependencies.md)
- [Repository maintenance](.github/MAINTENANCE.md)
- [Security reporting](SECURITY.md)

Neither historical plan reviews nor deterministic CI establish live provider
eligibility or a complete release. Both providers must pass the coding workflow
in M6. Linux and macOS remain later runtime qualification targets.
