# z

A Rust coding-agent harness inspired by fx, with an engine that owns turns and local sessions. Tools and approvals arrive in later milestones.

**Status: M0 scaffold. The CLI persists deterministic scripted turns in SQLite. No tools or live providers execute yet.**

With the pinned Rust toolchain and Windows MSVC prerequisites installed:

```powershell
cargo run -p harness-cli -- ask --workspace . 'hello'
```

Storage defaults to `%LOCALAPPDATA%/z-harness`, outside the repository. For an
owned disposable fixture, pass `--data-dir <absolute-local-directory>`. Keep
storage private and outside cloud-synchronized/network folders. The command
prints session/turn IDs, a completed response, and unavailable usage. Use
`--session <id>` to append to a completed session in the same workspace.

The first release targets Windows with two required providers: OpenRouter and direct ChatGPT plan access through the supported Sign in with ChatGPT flow. Account/app eligibility must be validated before a complete release. Linux and macOS are later qualification targets.

- [Implementation plan](RUST_HARNESS_PLAN.md)
- [Independent reviews and reconciliation](reviews/REVIEW_RECONCILIATION.md)
- [Contributing and setup](CONTRIBUTING.md)
- [M0 lifecycle and schema contracts](docs/m0-contracts.md)
- [Testing and qualification limits](docs/testing.md)
- [Dependency and toolchain baseline](docs/dependencies.md)
- [Repository maintenance](.github/MAINTENANCE.md)
- [Security reporting](SECURITY.md)

The two historical independent reviews evaluated the frozen v1 plan. Their
reports do not establish approval of this implementation. CI checks repository
maintenance and deterministic Windows M0 behavior; it does not qualify live
provider access, tools, crash recovery, or a complete coding-agent release.
