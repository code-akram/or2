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
│   ├── or2-ffi/          uniffi surface, the only API Kotlin sees
│   └── or2-pair/         host CLI for Easy pair (QR + pairing over sshd); depends on nothing in the app
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
separates what is implemented from lane work. FFI API version 23 exports `build_info`, key
generation and import, `connect_host` with the `HostConnection`, `Session` and `HerdrWatch`
objects and their listeners, pairing, and two test fixtures (`contract_probe_session`,
`contract_probe_host`) that drive the real objects without a network. There is no export that
connects a single shell: terminals are channels of a host connection. API 21 adds a bounded recent-directory
history read and `TerminalTarget.ShellIn` for one-tap project shells (v0.1.5). API 22 adds pure directory
merging/validation so Dirs includes the live herdr agent/pane cwd already watched by the picker, ahead of history,
without another query. Unreleased API 23 adds coalescing-safe moved-row references to terminal
frames; Android reuses resolved rows, retains their drawing commands, and pulls/decodes frames
off main before applying at vsync. See [contracts: Frames](contracts.md#frames) and the
[build instructions](build.md) for the shared toolchain and verification commands.

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
- Types are generated from `herdr api schema --json` by `cargo xtask gen-herdr-types`, never
  hand-written. The bundle needs a normalization step first (extract `schemas.success_response`,
  `schemas.event`, …, rewrite their `$ref`s, drop validation keywords, open string enums), then
  cargo-typify. Test against sanitized fixtures and an isolated live session.
- Clients must ignore unknown fields and treat unsupported methods as normal errors.
- M0 measured 365 ms from SSH connect to snapshot installed on the Arch host.

### tmux

v0 uses `tmux list-sessions -F …` and `tmux new -A -s <name>`. Control mode (`tmux -CC`), which
maps windows and panes to native tabs, comes later.

### Keys

- Generate Ed25519 in-app (in the host form and the pairing review as **New key**, or on the Keys screen) or
  import OpenSSH keys (with passphrase).
- "Install on host" appends the public key to `authorized_keys` over an existing connection.
- Trust on first use with the fingerprint shown. Agent forwarding off by default.
- Later: non-exportable P-256 in the secure element, FIDO2 `sk-` keys.

### Notifications

User choice, per host and per agent state (blocked, done). Default off; or2 asks in context, where the
alerts are switched on, never while connecting. While the app is alive, herdr events drive local
notifications. Remote push, if ever, goes through UnifiedPush or ntfy, never FCM.

No permission dialog ever comes in the middle of a connect: every connect, the first one included, goes
straight to the biometric unlock. The battery-optimisation exemption is the last step of adding a host
(asked once); the connection notification is offered by a small, dismissible card on Home while a host is
connected, and the service runs without it ([contracts](contracts.md#permissions-none-on-connect)).

## Android specifics

- UI tokens and component rules live in [the UI system](ui.md); Compose code uses them through
  one theme, never ad-hoc colours or sizes.
- `compileSdk` / `targetSdk` 36, `minSdk` 34 (Android 14).
- OxygenOS kills background apps aggressively. Defences: a foreground service with a persistent
  notification while sessions are open (type `specialUse`), a one-time battery-optimisation
  exemption prompt as the last step of adding a host, and sub-second reattach to the last tmux
  session or herdr pane.
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
M4 backlog (owner requests, all to implement):
1. **Wake-on-LAN magic packet.** Optional MAC address field in the host form (validated
   `aa:bb:cc:dd:ee:ff`, stored in Room, used only in the packet). A "Wake" action on a host shown
   as asleep sends the standard magic packet (6 × `0xFF` + 16 × MAC) as UDP to port 9 on the
   limited broadcast and the current network's subnet-directed broadcast, **through or2's
   `DatagramTransport`** (no socket outside it), then retries the connection for up to ~30 s
   with in-place progress. Same LAN only; the Mac needs "Wake for network access" and power.
2. **TCP wake probe** (Moshi's "Mosh Wake Probe"), per-host toggle: before reconnecting a
   sleeping host or restoring its mosh session, make a TCP connection attempt to its SSH port on
   each address so macOS's Bonjour Sleep Proxy (an always-on Apple TV/HomePod/AirPort) wakes it;
   then connect normally. Needs no MAC address and can work beyond the broadcast domain.
3. **Device behaviour settings** (seen in Moshi): "Keep screen on while a terminal is open"
   (per-device, off by default, uses the window keep-screen-on flag only while a terminal is
   visible) and "Reopen the last terminal on launch" as a visible choice for or2's existing
   reattach (on by default).

Limits to surface, not solve: a MacBook with the lid closed on battery enters sleep and then
standby with Wi-Fi off, so neither wake path reaches it ("Wake for network access: Always" on
battery helps only in light sleep). When a wake attempt fails for a host marked "sleeps", show
"Can't wake: it may be asleep with the lid closed or on battery" and stop retrying; the host
form's help text explains the options (keep it awake on power, a closed-lid keep-awake tool or
`pmset disablesleep`, or run long agents on the always-on host).
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
not configured by the scaffold; M1 development uses the debug APK. (Since v0.1.0 the release build is signed
locally when the maintainer's signing file is present, and unsigned otherwise: see
[build: Release signing](build.md#release-signing).)

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

Interfaces are in [contracts: M3](contracts.md#m3-stays-connected) (FFI API 10, with the follow-up and
the orphan cleanup).

- [x] M3-A: mosh terminals on host connections (transport choice, peer pinning, bootstrap,
  terminate on early failure), link health, `roam`/`network_changed`, mosh survives host loss.
- [x] M3-B: foreground service owning connections, notification, network callback, Room v3
  transport preference with AUTO fallback, grouped reconnect unlock, reattach to the last pane,
  battery-optimisation prompt. Integrated on `m3/integrate` with M3-A and the compact UI scale; its
  JVM, real-FFI and fake-backed tests and the Rust suite with real `sshd`, `tmux`, herdr and
  `mosh-server` pass, and the device tests compile. Ticked on those checks only: the notification,
  the service starting from the foreground, the permission and battery dialogs, the network
  callback and reattach after 10 minutes in the background are device-only behaviour that nothing
  here has observed, so they stay in the acceptance item below. Decisions and deviations are in
  [contracts: M3-B](contracts.md#m3-b-android-status-and-decisions-integrated-on-m3integrate).
- [x] UI-C: compact default scale everywhere (type, controls, popups, arrow pad).
- [x] M3 follow-up (advisor review and owner decisions of 2026-10-01): broader roaming triggers
  (transport-set and interface changes, every foreground return), AUTO's 5 s mosh budget as an absolute
  deadline over the whole start with a 24 h per-host memory in Room (v4, with `sleeps`), a non-modal
  reconnect chip and `Asleep` hosts, auto-resume after process death, the host-form address hint and
  the manifest permissions, on FFI API 9. `resume_mosh` was **rejected after review** (nonce reuse
  under a persisted key, and the transport state a fresh client lacks); process death is handled by
  the fast Home Resume path instead, and the `mosh-server` it orphans is stopped by its recorded pid
  over the next connection (FFI API 10). Merged with the review fixes of M3-A (unconfirmed goodbyes stopped
  over SSH, the host-owned cancellation signal, stops kept as host debt). Ticked on the Rust suite (real `sshd`, `tmux`, herdr, `mosh-server`),
  the JVM, real-FFI and fake-backed tests and the compile-checked device tests; the phone-only
  behaviour stays in the acceptance item below
  ([contracts](contracts.md#m3-follow-up-advisor-review-owner-decisions-2026-10-01)).
- [ ] Integration, external review and phone acceptance, including v0 step 3 (background
  10 minutes over mobile data, return to the same pane without re-typing).

**M3 phone re-check (2026-10-02, Wi-Fi, plugged in, OnePlus 10 Pro, `or2.timing`):**
fingerprint → first live herdr view 1.1–1.6 s (three hosts in parallel, SSH connected in about
0.8 s); tap agent → mosh pane frame 0.8 s; reopen an open agent terminal 0.7 s; process killed →
launcher start auto-resumes the same pane 2.0 s after the fingerprint, stopping both orphaned
mosh-servers on reconnect; no battery dialog on return. Earlier the same day (before the polish
lane) these paths took 5–11 s. Still deferred by the owner: mobile data, Wi-Fi→mobile handover,
unplugged (Doze) background runs.

### Easy pair checklist

Version 1 (FFI API 12: a one-shot HMAC-authenticated TCP listener on a random port) shipped and was
replaced before phone acceptance: on the first real host (a rented server reached over its public
address) the extra port was unreachable although SSH worked. Version 2 pairs over sshd itself
([contracts](contracts.md#easy-pair-qr-onboarding)).

- [x] `or2-pair` v2: the phone's code typed on the host, a bootstrap key with a forced command,
  `enroll` with a state file, removal on every ending and the sweep, sshd version gating, IPv6 addresses;
  v1 listener, bind policy and HMAC exchange removed. Verified by the `or2-pair` unit tests (120), `flow`,
  `cli`, `fifo` and `manual_keys` suites and `tests/sshd.rs` (12, against a disposable sshd with
  `OR2_REQUIRE_SSHD=1`, including the product's own `or2_core::pair` client).
- [x] Phone v2: `or2_core::pair` over SSH (pinned `hk`, bootstrap key only, one handshake), FFI API 13
  `pair_new_code`/`pair_enroll`, the Easy pair screen with the code, messages. Verified by the
  `or2-core` pair unit tests and `tests/pair.rs` (10, disposable sshd), `PairFlowTest`, `PairMessagesTest`,
  the JVM `PairEndToEndTest` (testhost behind a disposable sshd, then a real `connect_host` with no prompt)
  and the device suite (`PairUiDeviceTest`, with screenshots of the three screens).
- [x] Docs: ["Pair a host"](pairing.md) rewritten for v2 (no ports, no firewall workarounds) and the
  [manual setup guide](manual-setup.md).
- [x] One add-host chooser (owner feedback against Moshi): the empty Home, the empty inbox and the **+** sheet show
  the same two cards (Easy pair with QR, Set up manually) and Home has no "Add an SSH key" step; the manual form
  offers **New key** like the pairing review ([contracts](contracts.md#first-run-one-add-host-chooser)). Verified
  by `AddHostOptionsTest`, `NavigationTest`, `KeyOperationsTest` and the device tests, which pass on the
  phone (the `.devicetest` app; 2026-10-02).
- [x] No permission dialogs during a connect (owner feedback on a fresh install against Moshi): the battery
  exemption is the last step of adding a host (after Easy pair, before the host connects; after the manual form's
  save), once, skipped when exempt; the notification permission is offered in context on Home, and the service
  runs without it ([contracts](contracts.md#permissions-none-on-connect)). Verified by `OneTimePromptsTest`,
  `AddHostEndTest`, `NavigationTest` and the device tests (`AddHostEndDeviceTest` and others), which pass on the phone (the `.devicetest` app,
  117 of 118 with the notification test skipped; 2026-10-02).
- [x] Phone acceptance of Easy pair: a real `or2-pair` code on a host reached over its public address
  and on a LAN host, with the battery step, then connect. The public-address host passed on 2026-10-02 (paired,
  saved with its host key trusted, connected), and so did the LAN host on 2026-10-02: a Mac, with the macOS
  `or2-pair` from the v0.1.0 one-liner and the v0.1.0 release APK (paired and connected with no host-key prompt).
- [x] Installer, Linux part: `cargo xtask dist` builds the static x86_64 and aarch64 Linux binaries with
  `SHA256SUMS` (built on the Arch runner; the x86_64 one runs, the aarch64 one is checked by `file` only), and
  `scripts/install-or2-pair.sh` (POSIX `sh`, ShellCheck-clean) installs one with its checksum verified, tested
  against fixture releases (`install_or2_pair.rs`: checksum OK and mismatch, unsupported CPU and OS). The checks
  print the exact fix for the host (sshd unit, package manager, firewall), and systemd's userdb
  `AuthorizedKeysCommand` snippet no longer warns ([contracts](contracts.md#host-cli)). External review (Codex:
  a P1, four P2s, a P3; Fable) fixed: a manual workflow run can never publish; the installer calls no GitHub API
  (GitHub's `latest/download`), is one function called on its last line, stages with `mktemp` in the destination
  and runs the binary before replacing anything; unknown init systems get no `systemctl`; a later `Match` block
  no longer reopens a decided setting. Tested in `install_or2_pair.rs` (truncation at every line, planted links,
  a binary that does not run, `--version`, unsafe destinations) and the `or2-pair` unit tests.
- [x] Installer, release: one release stream, tagged `vX.Y.Z` (workspace version = app `versionName`, checked by
  `dist --expect-version` and an xtask test). The first release, **v0.1.0** (the APK and the `or2-pair` binaries,
  [notes](releases/v0.1.0.md)), is published by pushing the tag `v0.1.0`: `.github/workflows/release.yml` builds
  the four binaries (the macOS ones only there, on a macOS runner) and creates the release; the APK is signed
  locally and uploaded with `gh release upload`. Ticked once the workflow has run, the release exists, the
  one-liner installs from it and the APK installs on the phone. Done on 2026-10-02: the release exists with its four
  binaries, the one-liner installed `or2-pair` on a Mac, and the v0.1.0 APK (its SHA-256 checked against the notes)
  replaced the debug build on the phone and paired a host.

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
