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
  on a Canvas with a glyph cache. If Canvas is too slow, Rust renders into a SurfaceView instead;
  the FFI boundary does not change.
- **Input.** Kotlin owns the IME and the keys row and sends key events to libghostty's key encoder.
- **Persistence lives in Kotlin.** Hosts and settings in Room. Private keys encrypted at rest with
  a hardware-backed Android Keystore key behind biometric unlock, decrypted only at connect time
  and handed to Rust. Rust has no storage code.
- **Sessions live in Rust**, on a tokio runtime owned by the foreground service. The UI subscribes
  through uniffi callback interfaces.

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

- `targetSdk` 36, `minSdk` 31.
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

All three passed on the Arch host. Nothing has run on the phone yet. Details are in each `RESULT.md`.

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
| Rendering | Canvas first | Simplest; GPU path kept open behind the same FFI |
| Package ID | `io.github.code_akram.or2` | Change if a domain is preferred |
