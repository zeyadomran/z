# Contributing

Use Git and Node.js 22.23.2 (for repository hooks only). No npm packages are needed. This repository has no Cargo workspace yet; add pinned Rust tooling, formatting, Clippy, build checks, and the canonical functional suite with M0.

```sh
node scripts/git-hooks/install.mjs
node --test scripts/git-hooks/verify.mjs
```

The installer preserves existing hooks and recognizes the shared machine-wide size guard. Pre-commit checks complete staged files against the 20,000-byte limit. Keep files cohesive and small; never bypass hooks or exempt maintained source. Read [the hook guide](scripts/git-hooks/README.md).

All changes after repository bootstrap enter `main` through PRs, including administrator changes. Use a Conventional Commit title such as `feat: add durable sessions`; it becomes the squash commit message. Resolve conversations and pass every required check at the reviewed head. Zero external approvals are required for solo-maintainer work. Use an exact-head guarded squash merge after authorization; do not bypass protection.

Read [AGENTS.md](AGENTS.md) and the testing skill before coverage decisions. Existing hook tests are the canonical repository-hook flow; no application tests exist yet. Before committing, run `git diff --check`, stage the intended files, then run `node scripts/git-hooks/check-staged-file-size.mjs`. CI validates workflow syntax, hook behavior on Windows and Linux, PR dependency changes, and Actions CodeQL analysis.
