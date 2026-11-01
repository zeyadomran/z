# z

A planned Rust coding-agent harness inspired by fx, with an engine that owns the agent loop, tools, approvals, and local sessions.

**Status: planning and repository foundation. No runnable harness exists yet.**

The first release targets Windows with two required providers: OpenRouter and direct ChatGPT plan access through the supported Sign in with ChatGPT flow. Account/app eligibility must be validated before a complete release. Linux and macOS are later qualification targets.

- [Implementation plan](RUST_HARNESS_PLAN.md)
- [Independent reviews and reconciliation](reviews/REVIEW_RECONCILIATION.md)
- [Contributing and setup](CONTRIBUTING.md)
- [Repository maintenance](.github/MAINTENANCE.md)
- [Security reporting](SECURITY.md)

The two independent reviews evaluated the frozen v1 plan. The reconciled v2 and later provider requirements have not been independently re-reviewed. Repository CI validates repository maintenance only; it does not establish runtime correctness or provider access.
