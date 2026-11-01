# Rust coding-agent harness implementation plan

Status: revised v2; both independent v1 reviews have been reconciled in `reviews/REVIEW_RECONCILIATION.md`. OpenRouter and direct ChatGPT plan access are both required in v0.1. These requirements postdate the frozen review input at `reviews/RUST_HARNESS_PLAN.v1.md`, and v2 has not been independently re-reviewed. M1a now implements the bounded OpenRouter read-only loop, scoped tools, durable continuation and read-only recovery. See [M1a configuration/contracts](docs/openrouter.md), [historical M0 contracts](docs/m0-contracts.md) and [actual validation limits](docs/testing.md). Live OpenRouter qualification remains outstanding in M6 (#9); ChatGPT plan access and the complete release remain unqualified.

M2 now adds [supervised patches and native reconciliation](docs/supervised-patches.md), with offline/native fixture evidence. This does not close the live provider or complete release gates.

- [1. Outcome and scope](docs/plan/01.md)
- [2. Architecture and ownership](docs/plan/02.md)
- [3. Rust implementation choices](docs/plan/03.md)
- [4. Required providers and agent loop](docs/plan/04.md)
- [5. Tools and workspace behavior](docs/plan/05.md)
- [6. Permissions, credentials, and process ownership](docs/plan/06.md)
- [7. Persistence and crash recovery](docs/plan/07.md)
- [8. Context and instructions](docs/plan/08.md)
- [9. CLI and observable behavior](docs/plan/09.md)
- [10. Delivery sequence and acceptance gates](docs/plan/10.md)
- [11. Minimal functional validation](docs/plan/11.md)
- [12. Later capabilities and decision gates](docs/plan/12.md)
- [13. Independent review procedure](docs/plan/13.md)

The numbered sections preserve revised v2 in order. Paths mentioned in prose are relative to the repository root.
