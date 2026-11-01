# M0 dependency baseline

Verified against upstream documentation and the resolved Cargo metadata on
2026-11-01. Cargo.lock is the reproducible dependency source; manifests declare
compatible update ranges. The Windows target is x86_64-pc-windows-msvc.

- Rust **1.99.1**, edition 2024, pinned in rust-toolchain.toml. This was the
  stable channel reported by the [official manifest](https://static.rust-lang.org/dist/channel-rust-stable.toml).
- Tokio **1.54.1**: runtime, channels, timers and Ctrl+C. Test-only time control
  is enabled only as a dev dependency. [Upstream docs](https://docs.rs/tokio/1.54.1/tokio/).
- rusqlite **0.41.1**, libsqlite3-sys **0.39.1**, bundled SQLite **3.54.1**:
  the exact SQLite version was also checked in the downloaded amalgamation
  header. No system SQLite DLL is required. [rusqlite release](https://github.com/rusqlite/rusqlite/releases/tag/v0.41.1),
  [SQLite release history](https://sqlite.org/changes.html).
- Serde **1.0.229**, serde_json **1.0.154** for durable records; thiserror
  **2.0.21** for typed errors; clap **4.6.7** for the CLI; tempfile **3.27.0**
  for owned test directories. [clap docs](https://docs.rs/clap/4.6.7/clap/).

All resolved dependencies' declared minimum Rust versions fit the pin. The
Windows release build also compiled bundled SQLite using official Visual
Studio 2022 Build Tools 17.14.44 with the C++ x64/x86 tools and Windows 11 SDK
26100 components. Rust was installed with the official minimal rustup profile;
no shell/global PATH changes or unrelated toolchain changes were needed.

Cargo metadata identifies permissive licenses for the selected Windows graph:
MIT, Apache-2.0 alternatives, Zlib (foldhash) and Unicode-3.0 (unicode-ident).
SQLite is public domain. Preserve required upstream notices when packaging.
The owner selected the [MIT license](../LICENSE) for this repository on
2026-11-01. Licensing is distinct from ChatGPT distribution/account eligibility,
which remains unproven under the [subscription contract](chatgpt-subscription/README.md).

## SQLite security review boundary

The checked SQLite release notes include the 3.54.1 fix for a 32-bit heap-buffer
overflow (the selected target is 64-bit). The checked RustSec package pages
listed the following older advisories; the selected versions exceed each
patched floor:

- [RUSTSEC-2020-0014](https://rustsec.org/advisories/RUSTSEC-2020-0014.html):
  rusqlite patched from 0.23.0.
- [RUSTSEC-2021-0128](https://rustsec.org/advisories/RUSTSEC-2021-0128.html):
  rusqlite patched from 0.26.2 (also 0.25.4).
- [RUSTSEC-2022-0090](https://rustsec.org/advisories/RUSTSEC-2022-0090.html):
  libsqlite3-sys patched from 0.25.1.

This is a targeted upstream SQLite/rusqlite advisory review, not a full
dependency security audit or a claim of no vulnerabilities. PR dependency
review and Cargo Dependabot updates remain enabled. Recheck the actual lockfile
and upstream advisories when updating or qualifying a release. No HTTP/auth,
tracing, native process or plugin dependencies were added before their slice.

## M1a dependency additions

Checked against upstream crate metadata/API documentation on 2026-11-01:

- reqwest **0.13.5**, native-tls **0.2.18**, schannel **0.1.30** on Windows:
  native Windows TLS, explicit direct/proxy selection, disabled redirects and
  connect/read-idle deadlines, with the engine bounding overall active work.
  Default reqwest features (including implicit
  system proxy discovery) are disabled. Only native-tls/json/stream are enabled.
  [reqwest](https://docs.rs/reqwest/0.13.5/reqwest/),
  [native-tls](https://docs.rs/native-tls/0.2.18/native_tls/).
- sha2 **0.11.1** for full raw-file/instruction/workspace identity hashes;
  [upstream](https://docs.rs/sha2/0.11.1/sha2/).
- httpdate **1.0.3** for standard Retry-After HTTP dates;
  [upstream](https://docs.rs/httpdate/1.0.3/httpdate/).
- windows-sys **0.61.2** for final-handle paths, file link count and drive type.
  Native unsafe code is confined to the reviewed workspace handle and native
  execution modules; the rest of the workspace denies unsafe code. The merged
  [launcher prerequisite](windows-launcher.md) uses additional Windows features
  of the same crate; agent command execution remains M3 work.
  [bindings](https://docs.rs/windows-sys/0.61.2/windows_sys/).
- rusqlite's existing version now enables its `backup` feature for consistent
  v1-to-v2 migration backups. SQLite remains the M0 bundled version.

These added direct dependencies declare MIT/Apache-2.0 alternatives and minimum
Rust versions below the pinned toolchain. Cargo.lock records the complete graph;
CI dependency review remains required. Native TLS uses the supported host trust
store. This targeted version/API/license check is not a complete security audit.
No dependency update changes the outstanding live provider release gates.
