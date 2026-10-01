# AGENTS.md

Rules for humans and agents working in this repo. Read `docs/design.md` first.

## Architecture invariants

- **Protocols live in Rust.** SSH, mosh, terminal emulation, tmux and herdr clients are in
  `core/`. Kotlin never speaks a wire protocol.
- **One FFI surface.** Kotlin sees only `or2-ffi`. Do not expose `or2-core` types directly.
- **Rust has no storage.** Hosts, settings and keys are persisted by the Android app; Rust
  receives what it needs per call.
- **Everything network goes through the `Transport` trait.** No direct `TcpStream::connect` or
  `UdpSocket::bind` outside transport implementations.
- **herdr types are generated from `herdr api schema --json`.** Never hand-write them. Ignore
  unknown fields.
- **No Google Play Services or FCM.** Keep the app F-Droid-clean.

## Code

- Rust edition 2024. `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` must pass.
- Kotlin follows the official Kotlin style; Compose for all UI.
- Tests next to the code for Rust (`#[cfg(test)]`) and in `core/*/tests/` for integration tests.

## Tooling

- Repository tooling is Rust (the `xtask` crate, run as `cargo xtask <task>`) or POSIX `sh` for
  trivial glue. No Python, Node or other runtimes in the repo or the build; generated files are
  checked in and every generator has a `--check` mode.

## Dependencies

- Commit `Cargo.lock` and the Gradle lockfile. Git dependencies are pinned to a commit.
- Every dependency must be GPL-3.0-compatible. Record ported or vendored code in
  `THIRD_PARTY_NOTICES.md` with source, pinned commit, path, licence and copyright line.

## Spikes

- `spikes/` holds M0 throwaway prototypes. They are not part of the workspace, may use shortcuts,
  and are deleted once their findings land in `core/`. Each has a `RESULT.md`.

## Secrets

- No hosts, keys, tokens or ZeroTier network IDs in the repo.
