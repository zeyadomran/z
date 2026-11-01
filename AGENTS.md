# Repository instructions

Read [the plan](RUST_HARNESS_PLAN.md) and [review reconciliation](reviews/REVIEW_RECONCILIATION.md) before implementation. Both OpenRouter and direct ChatGPT plan access are required. Preserve the plan's Windows execution, approval, cancellation, and recovery contracts.

## Testing

Whenever a task needs tests, test changes, a coverage review, or suite cleanup, read and apply [minimal-functional-tests](skills/minimal-functional-tests/SKILL.md) before deciding coverage or editing tests. Use one canonical public-interface flow with real internal modules and persistence; control external boundaries. No placeholder feature tests.

## Delivery

Use Conventional Commits and focused PRs. Preserve existing hooks. Never bypass checks or branch protection. Keep maintained files at or below 20,000 bytes; split by responsibility, without exemptions for maintained source. Keep public documentation and examples free of personal filesystem paths, credentials, and private transcripts. Report actual validation and distinguish repository CI from runtime qualification.
