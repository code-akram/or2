# or2 design

or2 is a FOSS (GPL-3.0-or-later) Android client for SSH and mosh, built for working with coding
agents that run on your own machines inside tmux or herdr. It is a tool for its author first:
two hosts, one phone, keys only.

## Goal

The phone is a remote control for agents, not a small desktop. Three layers, each useful on its own:

1. **Terminal.** Reliable SSH and mosh, fast rendering, a keyboard row, natural scrolling and selection.
2. **Multiplexer.** tmux sessions and herdr spaces, tabs and panes shown as native UI.
3. **Agents.** One inbox for every agent on every host (blocked, working, idle), with quick replies.

## v0 acceptance test

On the phone, over mobile data:

1. Open or2 and see herdr agents on both hosts with their state within about 2 seconds.
2. Tap a blocked agent, answer it, and send.
3. Background the app for 10 minutes, return, and land in the same pane without re-typing anything.

## Environment

| Thing | Value |
|---|---|
| Phone | OnePlus 10 Pro (NE2211), OxygenOS 16 / Android 16 |
| Host A | MacBook Air M4, macOS; sleeps with the lid closed |
| Host B | Hetzner dedicated, Arch Linux; always on, primary agent host |
| Network | LAN + ZeroTier (Tailscale console unavailable in Oman) |
| Auth | SSH keys only |

## Architecture

```
╭─────────────────── Android (Kotlin) ───────────────────╮
│ Compose UI: hosts · terminal · agent inbox · keys row  │
│ TerminalView (Canvas) · IME · Keystore · Room          │
│ Foreground service: owns the Rust runtime              │
╰───────────────────────────┬────────────────────────────╯
                            │ uniffi (async + callback interfaces)
╭───────────────────────────▼────────────────────────────╮
│ Rust (tokio)                                           │
│ Session manager ─┬─ ssh (russh) ── Transport trait     │
│                  ├─ mosh (SSP)  ──╯                    │
│                  ├─ term (libghostty-vt)               │
│                  ├─ tmux (exec: list / attach)         │
│                  ╰─ herdr (JSON over forwarded socket) │
╰────────────────────────────────────────────────────────╯
```

### Repository layout

```
or2/
├── core/                 Cargo workspace
│   ├── or2-core/         transport · ssh · mosh · term · tmux · herdr
│   └── or2-ffi/          uniffi surface, the only API Kotlin sees
├── android/app/          Jetpack Compose UI
├── spikes/               M0 throwaway prototypes, not part of the workspace
└── docs/
```

Start with two crates. Split a module into its own crate only once its interface has settled.

### Boundaries

- **Terminal state lives in Rust.** Kotlin receives packed snapshots of changed rows and draws them
  using hardware-accelerated Android Canvas with a glyph cache. Canvas is the selected renderer,
  not a fallback for a custom GPU renderer. Retain frame-time instrumentation before considering
  a different rendering backend.
- **Input.** Kotlin owns the IME and the keys row and sends key events to libghostty's key encoder.
- **Persistence lives in Kotlin.** Hosts and settings in Room. Private keys encrypted at rest with
  a hardware-backed Android Keystore key behind biometric unlock, decrypted only at connect time
  and handed to Rust. Rust has no storage code.
- **Sessions live in Rust**, on a tokio runtime owned by the foreground service. The UI subscribes
  through uniffi callback interfaces. The foreground service is M3 work; M1 ties session ownership
  to the app lifecycle instead.

### M1 scaffold contract

`core/` is a two-crate Rust edition-2024 workspace, separate from the M0 spikes.
`or2-core` owns the domain types and behaviour; `or2-ffi` converts them to UniFFI records,
errors, objects and callbacks. Kotlin imports only generated `io.github.code_akram.or2.ffi`
bindings. UniFFI 0.32.2 generates bindings from the host library; Gradle cross-builds and
packages the Android arm64 library at API 34. Generated sources and native libraries are build
outputs, not checked-in copies.

The shared M1 contracts (transport, key material, host-key trust, session lifecycle and
callbacks, changed-row frames, input) are defined in [contracts](contracts.md), which also
separates what is implemented from lane work. FFI API version 2 exports `build_info`,
`terminal_size`, key generation and import, the `Session` object with its `SessionListener`,
and `contract_probe_session`, a test fixture that drives a real `Session` without a network.
There is deliberately no `connect` export until the SSH session works; no success-returning
stubs. See [build instructions](build.md) for the shared toolchain and verification commands.

### Transport

The SSH and mosh layers sit on a `Transport` trait so the network path can change without
touching them.

| Transport | TCP | UDP | Milestone |
|---|---|---|---|
| Direct: OS sockets (LAN, ZeroTier app, WARP, Headscale) | ✅ | ✅ | v0 |
| Jump host: SSH ProxyJump | ✅ | ❌ | v0 |
| Embedded ZeroTier (libzt, userspace, no VPN slot) | ✅ | ✅ | later |
| Cloudflare Access (WebSocket) | ✅ | ❌ | later |

mosh is offered only on transports that carry UDP. Android allows one VPN app at a time, so
embedded ZeroTier frees the VPN slot. The libzt release under BSL 1.1 with change date
2026-01-01 is now Apache-2.0; the bundled ZeroTierOne core must be license-checked before use.

A host may have several addresses (LAN IP, ZeroTier IP). or2 races them and uses whichever
answers first.

### herdr

- herdr exposes newline-delimited JSON over a Unix socket. The default session lives at
  `~/.config/herdr/herdr.sock`; named sessions (`herdr --session <name>`) live at
  `~/.config/herdr/sessions/<name>/herdr.sock`. Discover paths with `herdr [--session <name>]
  status server` over an exec channel; never hard-code them.
- Non-interactive SSH does not load the user's PATH (herdr is often in `~/.local/bin`). Probe
  common locations and cache the herdr path per host.
- or2 reaches the socket through OpenSSH streamlocal forwarding
  (`russh::client::Handle::channel_open_direct_streamlocal`) on the existing SSH connection:
  one long-lived event channel plus short-lived request channels. No daemon on the host.
- Bootstrap: subscribe and wait for the ack, then `session.snapshot`. Snapshots and events share
  **no sequence number**, so events received during the snapshot are invalidations, not patches:
  install the snapshot, then do serialized authoritative refreshes, repeating if another event
  arrives mid-read. On `events_lost`, resubscribe and reconcile. Re-snapshot after every reconnect.
- `pane.agent_status_changed` subscriptions need a `pane_id`, so or2 subscribes per pane and adds
  subscriptions as panes appear.
- Types are generated from `herdr api schema --json`, never hand-written. The bundle needs a
  normalization step first (extract `schemas.success_response`, `schemas.event`, …, rewrite their
  `$ref`s), then cargo-typify; generated code needs `regress`. Test against sanitized fixtures.
- Clients must ignore unknown fields and treat unsupported methods as normal errors.
- M0 measured 365 ms from SSH connect to snapshot installed on the Arch host.

### tmux

v0 uses `tmux list-sessions -F …` and `tmux new -A -s <name>`. Control mode (`tmux -CC`), which
maps windows and panes to native tabs, comes later.

### Keys

- Generate Ed25519 in-app or import OpenSSH keys (with passphrase).
- "Install on host" appends the public key to `authorized_keys` over an existing connection.
- Trust on first use with the fingerprint shown. Agent forwarding off by default.
- Later: non-exportable P-256 in the secure element, FIDO2 `sk-` keys.

### Notifications

User choice, per host and per agent state (blocked, done). Default off; or2 asks the first time
it connects to herdr. While the app is alive, herdr events drive local notifications. Remote push,
if ever, goes through UnifiedPush or ntfy, never FCM.

## Android specifics

- `compileSdk` / `targetSdk` 36, `minSdk` 34 (Android 14).
- OxygenOS kills background apps aggressively. Defences: a foreground service with a persistent
  notification while sessions are open (type `specialUse`), a one-time battery-optimisation
  exemption prompt, and sub-second reattach to the last tmux session or herdr pane.
- No Google Play Services or FCM dependencies, so F-Droid stays possible. Distribution starts as a
  sideloaded APK.

## Host setup

**Hetzner Arch**
- `pacman -S mosh tmux`, plus herdr.
- nftables: allow UDP 60000–61000 only on the ZeroTier interface (`zt*`). Consider SSH on ZeroTier
  only as well.

**MacBook Air**
- System Settings → Sharing → Remote Login; `brew install mosh tmux`, plus herdr.
- `/opt/homebrew/bin` is not on `PATH` for non-interactive SSH, so `mosh-server` is not found.
  or2 probes common paths and remembers the result per host.
- Allow `mosh-server` in the macOS firewall prompt.
- The Mac sleeps with the lid closed. Run unattended agents on the Hetzner box, or keep the Mac on
  power with `caffeinate`.

## Milestones

**M0: prototypes.** Each has a pass/fail test. Results live in `spikes/*/RESULT.md`.
1. libghostty-vt builds for `aarch64-linux-android` via `libghostty-rs` and renders `htop` correctly.
2. A Rust mosh client holds a session with stock `mosh-server` 1.4.0, including an address change.
   Fallback: the official C++ `mosh-client` built with the NDK.
3. russh forwards herdr's Unix socket (streamlocal) and receives `session.snapshot` plus live events.

### M0 results (2026-09-30)

All three prototypes passed on the Arch host, not on the phone. Details are in each `RESULT.md`.
The later M1 scaffold's UniFFI bridge is tested on the phone; that does not validate these
protocol/terminal prototypes on Android yet.

| Spike | Result | Pinned |
|---|---|---|
| libghostty-vt | `top` renders correctly at 100×30; cursor, SGR, alt screen, wide CJK/emoji and key encoding pass; Android arm64 cdylib builds (2.4 MB, needs only `libc`/`libdl`), no workarounds | libghostty-rs `8953a74`, Ghostty `22d1317`, Zig 0.16.0 |
| mosh | `wilsonglasser/mosh-rs` interoperates with stock `mosh-server` 1.4.0, resizes, and survives a source-port change; pure-Rust AES-128-OCB3, no OpenSSL; Android arm64 builds | mosh-rs `90b3712` |
| herdr over SSH | russh Ed25519 auth, exec socket discovery, streamlocal, subscribe + snapshot, live events on an isolated session all pass; schema codegen partial (needs normalization); Android arm64 builds | russh 0.63.3 (aws-lc-rs) |

Consequences for `or2-core`:
- Vendor mosh-rs eventually: inject the UDP `Transport` instead of it owning a `UdpSocket`, and
  add source-interface selection for real Wi-Fi-to-mobile-data migration.
- Open for M3: mosh-rs renders into its own framebuffer (a caller-provided `Screen`) and exposes
  no raw host-byte stream, but or2 keeps terminal state in libghostty-vt. Either add a raw-bytes
  tap to mosh-rs or live-test `at-least/mosh` (GPL-3.0-or-later, has `take_host_bytes()`,
  repository moved to "conch").
- `mosh-server` refuses to start without a UTF-8 locale. or2 sets `LANG=C.UTF-8` (or the host's
  UTF-8 locale) in the bootstrap command.

**M1: terminal you can use.** Add host, add key, SSH in, shell with keys row.
**M2: multiplexers.** tmux picker, herdr agent inbox across hosts, multi-address hosts.
**M3: stays connected.** mosh transport, foreground service, automatic reattach. Passes the v0 test.
**M4: agent features.** Quick replies, opt-in notifications, image paste (SFTP upload + path), voice.
**Later.** tmux control mode, embedded ZeroTier, chat view, diff viewer, port-forward preview.

### M1 implementation checklist

Checked items are verified work, not a completed SSH terminal. Keep the two-crate architecture
and the Kotlin/Rust boundaries above. Stage 0 is the gate for parallel work: once it is complete,
stages 1–3 run as three lanes against [the shared contracts](contracts.md). Lane A owns `core/`;
lane B owns Room, Keystore, the host/key/trust screens and the app-scoped session holder; lane C
owns the terminal view (Canvas renderer, IME, keys row). Changes to `or2-ffi` exports, the
contracts or shared navigation (`MainActivity`) are coordinated. Stage 4 needs all three.

Completed foundation:
- [x] Configure shared user-local JDK, Android SDK/NDK and Gradle tooling.
- [x] Build the Rust/Android scaffold and generated UniFFI bootstrap contract with dependency locks.
- [x] Verify Rust checks, Android builds, JVM native tests and the phone native-library smoke test.

**0. Shared contract gate** (before the lanes split)
- [x] Define the `Transport` trait and `DirectTcp`; prove a transport stream drives russh's
  `connect_stream`.
- [x] Implement key material: Ed25519 generation, OpenSSH import with passphrase, the unencrypted
  storage form Kotlin encrypts, typed errors; verify against `ssh-keygen`.
- [x] Settle host-key trust: Kotlin-persisted keys in the request, first-use/changed prompts, and
  approval bound to the presented fingerprint.
- [x] Implement the session lifecycle contract: states, close reasons, errors, commands,
  listener threading/ordering/release, disconnect and drop semantics, via a handle/driver split.
- [x] Define changed-row frames (styles, cursor, wide cells, scrollback, full/delta resync) and
  the notify-once pull mailbox that provides backpressure.
- [x] Define resize, committed-text, key and scroll input semantics.
- [x] Round-trip the records, errors, callbacks and lifecycle between Rust and Kotlin on the JVM
  with the contract probe; bump the FFI API to version 2.
- [x] Run the extended `NativeDeviceTest` on the phone (callbacks from Rust threads on ART):
  `am instrument` reported OK (3 tests) on the OnePlus 10 Pro, Android 16 (API 36).

**1. Host and key entry** (lane B)
- [ ] Add Compose host/key entry and persist host settings in Room; no storage in Rust.
- [ ] Offer Ed25519 generation and OpenSSH import, including passphrase-protected keys, using
  the `or2-ffi` key exports.
- [ ] Encrypt private keys with a hardware-backed Android Keystore key behind biometric unlock;
  decrypt only at connect time and hand them to Rust. Keep credentials out of logs and the repo.

**2. SSH connection and session lifecycle** (lane A: `core/`; trust UI in lane B)
- [ ] Add russh key authentication over `DirectTcp` in `or2-core`, with connect timeout and
  keepalive mapped to the contract's failures.
- [ ] Show the host fingerprint for explicit first-use confirmation, persist trust in Kotlin,
  and reject changed host keys until explicitly approved. Keep agent forwarding off.
- [ ] Open a PTY shell and export `connect` driving the session contract through `or2-ffi`; test
  failures as well as successful connections, with no success-returning stubs.
- [ ] Tie session ownership to the app lifecycle for M1 (lane B's app-scoped holder), with
  explicit disconnect and cleanup.

**3. Terminal rendering and input** (engine in lane A; Canvas, IME and keys row in lane C)
- [ ] Integrate the pinned libghostty-vt engine in `or2-core`; keep terminal state in Rust and
  publish changed-row frames through the session driver. Preserve the M0 spike until its
  findings land.
- [ ] Draw frames with hardware-accelerated Android Canvas and a glyph cache; instrument
  frame times. Handle cursor, styles, alternate screen, wide characters and terminal resizing.
- [ ] Wire Android IME and the keys row through the terminal key encoder to SSH input; add
  scrolling and selection. Test text composition and control/navigation keys on the phone.

**4. End-to-end phone acceptance**
- [ ] Run Rust and native contract tests, Android builds and lint using [the build workflow](build.md).
- [ ] On the phone, add a host/key, confirm its fingerprint and use an interactive SSH shell;
  verify typing, keys row, resize, scrolling, Unicode and a full-screen terminal application.
- [ ] Verify rejected/changed host keys, authentication failure, connection loss and disconnect
  produce clear states without silently trusting a host or leaking credentials.
- [ ] Record results and remaining limitations; mark M1 complete only after the usable-shell
  acceptance passes. Install or change host authorization only with explicit user approval.

Mosh, tmux/herdr integration and the foreground service remain M2/M3 work. Release signing is
not configured by the scaffold; M1 development uses the debug APK.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| License | GPL-3.0-or-later | Nobody can close a fork and charge for it; compatible with mosh |
| UI | Kotlin + Jetpack Compose | IME, services, notifications and share sheet are native problems |
| Core | Rust, bound with uniffi | One implementation of protocols; testable on the desktop |
| Terminal engine | libghostty-vt | MIT; the engine herdr itself vendors |
| SSH | russh 0.63 (aws-lc-rs backend) | Pure Rust, async, streamlocal verified in M0 |
| mosh | mosh-rs, vendored later | Only candidate verified against stock mosh-server 1.4.0 |
| Persistence | Room (Kotlin) | Idiomatic Android; Rust stays storage-free |
| Rendering | Hardware-accelerated Android Canvas | Selected renderer; measure frame times, no custom GPU backend in M1 |
| Package ID | `io.github.code_akram.or2` | Change if a domain is preferred |
