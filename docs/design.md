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
separates what is implemented from lane work. FFI API version 5 exports `build_info`,
`terminal_size`, key generation and import, `connect_host` with the `HostConnection`, `Session`
and `HerdrWatch` objects and their listeners, and two test fixtures (`contract_probe_session`,
`contract_probe_host`) that drive the real objects without a network. There is no export that
connects a single shell: terminals are channels of a host connection. See [build instructions](build.md) for the shared toolchain and verification commands.

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
  `~/.config/herdr/sessions/<name>/herdr.sock`. Discover paths with
  `herdr session list --json` (`socket_path`, `running`, `default`) over an exec channel; never
  hard-code them.
- Non-interactive SSH does not load the user's PATH (herdr is often in `~/.local/bin`). Probe
  common locations and cache the herdr path per host.
- or2 reaches the socket through OpenSSH streamlocal forwarding
  (`russh::client::Handle::channel_open_direct_streamlocal`) on the existing SSH connection:
  one long-lived event channel plus short-lived request channels. No daemon on the host.
- Bootstrap: subscribe and wait for the ack, then `session.snapshot`. Snapshots and events share
  **no sequence number**, so events received during the snapshot are invalidations, not patches:
  install the snapshot, then do serialized authoritative refreshes, repeating if another event
  arrives mid-read. On `events_lost`, resubscribe and reconcile. Re-snapshot after every reconnect.
- `pane.agent_status_changed` subscriptions need a `pane_id`, so or2 subscribes per pane. A
  subscription cannot grow, so when panes appear a new stream covering every current pane
  replaces the old one and the view is read again.
- Types are generated from `herdr api schema --json` by `scripts/gen-herdr-types.sh`, never
  hand-written. The bundle needs a normalization step first (extract `schemas.success_response`,
  `schemas.event`, …, rewrite their `$ref`s, drop validation keywords, open string enums), then
  cargo-typify. Test against sanitized fixtures and an isolated live session.
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

- UI tokens and component rules live in [the UI system](ui.md); Compose code uses them through
  one theme, never ad-hoc colours or sizes.
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
- mosh-rs is vendored (M2, lane A3) with the UDP socket injected through `DatagramTransport`.
  Source-interface selection for real Wi-Fi-to-mobile-data migration is M3: it is a
  `DatagramTransport` that binds each new socket to the current Android network, which the
  rebind-on-roam path already drives.
- Resolved in M2: or2 keeps terminal state in libghostty-vt without a raw host-byte tap. The
  vendored `Screen` is implemented over libghostty and copies are libghostty terminal snapshots,
  so `at-least/mosh` was not needed.
- mosh keeps no scrollback of its own: the server sends diffs of the visible screen, which scroll
  only for lines that scrolled between two frames. A fast burst (`seq 1 200`) arrives as one
  repaint and leaves no history in libghostty; a slow one leaves a little (libghostty caps it, so
  an endless paced stream costs about 70 KB and 0.2 ms per snapshot at worst; see contracts). Scrollback inside a
  mosh session is therefore not like SSH; tmux or herdr provides history there.
- `mosh-server -s` binds the UDP port to the address in the exec channel's `SSH_CONNECTION`, so
  the client must send to the address the SSH connection reached (with address racing, the
  winning one), not to a different name or address of the same host. The core pins every UDP
  socket, roaming included, to that IP and port (`HostHandle::peer_addr`, `Link::adopt`).
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
- [x] Add Compose host/key entry and persist host settings in Room; no storage in Rust.
- [x] Offer Ed25519 generation and OpenSSH import, including passphrase-protected keys, using
  the `or2-ffi` key exports. (Import and passphrase errors are verified by JVM tests against
  `ssh-keygen` and by device tests; phone acceptance used an app-generated key.)
- [x] Encrypt private keys with a hardware-backed Android Keystore key behind biometric unlock;
  decrypt only at connect time and hand them to Rust. Keep credentials out of logs and the repo.

**2. SSH connection and session lifecycle** (lane A: `core/`; trust UI in lane B)
- [x] Add russh key authentication over `DirectTcp` in `or2-core`, with connect timeout and
  keepalive mapped to the contract's failures.
- [x] Show the host fingerprint for explicit first-use confirmation, persist trust in Kotlin,
  and reject changed host keys until explicitly approved. Keep agent forwarding off. (A real
  changed host key is covered by Rust fixture and device UI tests, not by the phone session.)
- [x] Open a PTY shell and export `connect` driving the session contract through `or2-ffi`; test
  failures as well as successful connections, with no success-returning stubs.
- [x] Tie session ownership to the app lifecycle for M1 (lane B's app-scoped holder), with
  explicit disconnect and cleanup.

**3. Terminal rendering and input** (engine in lane A; Canvas, IME and keys row in lane C)
- [x] Integrate the pinned libghostty-vt engine in `or2-core`; keep terminal state in Rust and
  publish changed-row frames through the session driver. Preserve the M0 spike until its
  findings land.
- [x] Draw frames with hardware-accelerated Android Canvas and a glyph cache; instrument
  frame times. Handle cursor, styles, alternate screen, wide characters and terminal resizing.
- [x] Wire Android IME and the keys row through the terminal key encoder to SSH input; add
  scrolling and selection. Test text composition and control/navigation keys on the phone.

**4. End-to-end phone acceptance**
- [x] Run Rust and native contract tests, Android builds and lint using [the build workflow](build.md).
- [x] On the phone, add a host/key, confirm its fingerprint and use an interactive SSH shell;
  verify typing, keys row, resize, scrolling, Unicode and a full-screen terminal application.
- [x] Verify rejected/changed host keys, authentication failure, connection loss and disconnect
  produce clear states without silently trusting a host or leaking credentials.
- [x] Record results and remaining limitations; mark M1 complete only after the usable-shell
  acceptance passes. Install or change host authorization only with explicit user approval.

**M1 result: complete (2026-09-30).** Final checks on the integrated tree: `cargo fmt`, 61 Rust
tests (including OpenSSH interop) and Clippy with `-D warnings`; Gradle build, 57 JVM tests and
lint with no errors; 20 instrumented tests on the OnePlus 10 Pro (Android 16). Real-SSH
acceptance ran against a dedicated unprivileged test account on host B, created and authorized
with the owner's explicit approval, using an app-generated Ed25519 key:
- Biometric unlock and cancel; first-use host-key confirmation and rejection; authentication
  failure; remote exit; connection loss (airplane mode); Disconnect keeping the last screen.
- Typing through the default soft keyboard (single keys reach `vim` immediately), keys row
  (Ctrl+C, arrows, history), live resize (`stty size` 22↔42 rows with the IME), scrollback,
  `top`, `vim` alternate screen, Unicode and emoji, word selection, Copy and Paste (multi-line
  paste asks first), full-screen session layout, and Back to Hosts without disconnecting.
- Frame times on the phone at 55×42: frame apply p95 ≈ 12 ms; window total p50 ≈ 17 ms,
  p95 ≈ 24 ms for full-screen redraws.

Known M1 limitations:
- A terminal view recreated after a session has closed cannot restore the last screen (it can
  while connected).
- Physical-keyboard keys bypass the IME, so hardware-keyboard CJK composition is unavailable;
  soft-keyboard composition works.
- No bracketed paste; multi-line paste is confirmed instead. DECCOLM column switching is
  unsupported.
- Worst-case full-screen redraws exceed a 60 Hz frame budget at p95; optimise later if it matters.
- Sessions end with the app process, and a backgrounded app can lose its connection (observed
  after several minutes in another app), until the M3 foreground service.
- Before any release: ship licence notices for statically linked code, and make the build-time
  Ghostty fetch reproducible offline (F-Droid).

Mosh, tmux/herdr integration and the foreground service remain M2/M3 work. Release signing is
not configured by the scaffold; M1 development uses the debug APK.

### M2 implementation checklist

Scope: M2 plus the Rust half of M3's mosh. Lanes, ownership and interfaces are in
[contracts: M2](contracts.md#m2-hosts-multiplexers-and-mosh). One SSH connection per host
carries terminals, tmux/probe exec channels and herdr streamlocal channels (FFI API 7).

- [x] Lane 0: `remote`, `host`, `herdr::view` and tmux contract types; FFI API 4 surface;
  `contract_probe_host`; Kotlin JVM contract test.
- [x] Lane A1: host driver over russh with multiplexed channels, address racing, capability
  probe, tmux listing and attach, terminal targets, `connect_host`; OpenSSH interop tests.
- [x] Lane A2: generated herdr types, discovery, subscribe/snapshot/reconcile watch, focus;
  fixture and live isolated-session tests.
- [x] Lane A3: vendored mosh-rs behind `DatagramTransport`, `Screen` over libghostty,
  bootstrap and session driver; live tests against local `mosh-server`.
- [x] Lane B: Room v2 migration, multi-address hosts, host connection holder, inbox, host
  screen with tmux picker, session switcher.
- [x] UI system ([ui](ui.md)): Catppuccin theme, Moshi-grade screens and terminal chrome
  (toolbar, arrow pad, composer), small default cell size with pinch zoom, debug UI gallery.
- [x] Integration, internal review per lane, external adversarial review (Codex, 9 findings,
  all fixed) and phone acceptance.

**M2 result: complete (2026-10-01).** Final checks on `main`: `cargo fmt`, Clippy with
`-D warnings`, 349 Rust tests with live sshd, tmux, herdr and mosh-server required; Gradle
build, 141 JVM tests and lint; 67 instrumented tests on the OnePlus 10 Pro (Android 16).
Phone acceptance against the owner's real accounts (key authorized with the owner's explicit
approval, `no-agent-forwarding,no-X11-forwarding`):
- The M1 database (host, Keystore-bound key, trust) migrated to v2 on the phone without loss;
  a pre-upgrade copy was taken first.
- "Connect all": one fingerprint unlocked three hosts sharing a key (Arch as `akram` and as the
  test account, the Mac by name with a second address). First-use prompts for the two new hosts
  were approved on screen and the stored fingerprints matched the hosts' real ED25519 keys.
- v0 step 1: the inbox showed live herdr agents from both hosts (working, idle, unknown).
- v0 step 2: tapping an agent focused its herdr pane (verified through herdr) and opened it;
  the composer's single Send submitted a prompt that the agent (Codex) answered in 3 s. The
  first attempt exposed that agent TUIs treat text+Enter in one burst as a paste; `submit_text`
  (bracketed paste or text, a 100 ms pause, then a separate Enter) fixed it and was re-verified.
- tmux: create and attach on the test account rendered correctly.

Not yet verified on the phone: the 2-second inbox target was not timed; a dead first address
falling back to the second (covered by Rust racing tests, not observed on the phone); mosh (no
FFI until M3); v0 step 3 (background for 10 minutes and return), which needs M3's foreground
service and mosh.

### M3 implementation checklist

Interfaces are in [contracts: M3](contracts.md#m3-stays-connected) (FFI API 8).

- [ ] M3-A: mosh terminals on host connections (transport choice, peer pinning, bootstrap,
  terminate on early failure), link health, `roam`/`network_changed`, mosh survives host loss.
- [ ] M3-B: foreground service owning connections, notification, network callback, Room v3
  transport preference with AUTO fallback, grouped reconnect unlock, reattach to the last pane,
  battery-optimisation prompt.
- [x] UI-C: compact default scale everywhere (type, controls, popups, arrow pad).
- [ ] M3 follow-up: `resume_mosh` with Keystore-protected tickets, broader roaming triggers,
  5 s Auto fallback remembered per host, non-blocking return, manifest permissions
  ([contracts](contracts.md#m3-follow-up-advisor-review-owner-decisions-2026-10-01)).
- [ ] Integration, external review and phone acceptance, including v0 step 3 (background
  10 minutes over mobile data, return to the same pane without re-typing).

## Decisions

| Decision | Choice | Why |
|---|---|---|
| License | GPL-3.0-or-later | Nobody can close a fork and charge for it; compatible with mosh |
| UI | Kotlin + Jetpack Compose | IME, services, notifications and share sheet are native problems |
| Core | Rust, bound with uniffi | One implementation of protocols; testable on the desktop |
| Terminal engine | libghostty-vt | MIT; the engine herdr itself vendors |
| SSH | russh 0.63 (aws-lc-rs backend) | Pure Rust, async, streamlocal verified in M0 |
| mosh | mosh-rs, vendored (M2) | Only candidate verified against stock mosh-server 1.4.0 |
| Persistence | Room (Kotlin) | Idiomatic Android; Rust stays storage-free |
| Rendering | Hardware-accelerated Android Canvas | Selected renderer; measure frame times, no custom GPU backend in M1 |
| Package ID | `io.github.code_akram.or2` | Change if a domain is preferred |
