# Contributing

Use Git, Node.js 22.23.2 (repository hooks only), and Rust 1.99.1 from
`rust-toolchain.toml`. No npm packages are needed. Install Rust through official
rustup with the minimal profile; include rustfmt and Clippy. On Windows use
`x86_64-pc-windows-msvc`, Visual Studio C++ build tools and a Windows SDK.
See [the official prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html).

```sh
node scripts/git-hooks/install.mjs
node --test scripts/git-hooks/verify.mjs
```

The installer preserves existing hooks and recognizes the shared machine-wide size guard. Pre-commit checks complete staged files against the 20,000-byte limit. Keep files cohesive and small; never bypass hooks or exempt maintained source. Read [the hook guide](scripts/git-hooks/README.md).

All changes after repository bootstrap enter `main` through PRs, including administrator changes. Use a Conventional Commit title such as `feat: add durable sessions`; it becomes the squash commit message. Resolve conversations and pass every required check at the reviewed head. Zero external approvals are required for solo-maintainer work. Use an exact-head guarded squash merge after authorization; do not bypass protection.

Read [AGENTS.md](AGENTS.md) and the testing skill before coverage decisions.
`crates/harness-core/tests/engine_flow.rs` owns the canonical application flow;
extend it when a milestone adds behavior. See [testing](docs/testing.md).

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
node --test scripts/git-hooks/verify.mjs
git diff --check
```

Stage intended files, then run `node scripts/git-hooks/check-staged-file-size.mjs`.
Keep Cargo.lock committed. CI adds Windows Rust checks and a CLI smoke while
preserving workflow validation, Windows/Linux hook tests, dependency review
and Actions CodeQL. Passing CI is not live-provider or native tool qualification.
