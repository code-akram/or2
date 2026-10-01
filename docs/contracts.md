# M1 shared contracts

These are the interfaces the M1 lanes build against: Rust sessions and terminal, Android
host/key/trust, and Canvas/IME. The Rust source is authoritative (`core/or2-core/src/*.rs` for
behaviour, `core/or2-ffi/src/*.rs` for what Kotlin sees); this document records the decisions
and the rules the types cannot express. Changing a contract means changing the code, this
document and the tests together, and bumping `API_VERSION` in `or2-ffi` when an export changes.

## Status

All M1 contracts below are implemented and tested. M2 changes are specified in
[M2: hosts, multiplexers and mosh](#m2-hosts-multiplexers-and-mosh); lane 0 has landed the
M2 contract types and the FFI API 4 surface (`API_VERSION = 4`), with the lane A1, A2 and B
behaviour still to come.

| Contract | Implemented and tested | Open |
|---|---|---|
| Transport | `Transport` trait, `DirectTcp`, `Endpoint` validation | address racing (M2), UDP (M2 core, M3 export), jump host |
| Key material | Ed25519 generation, OpenSSH import with passphrase, storage form, typed errors; Keystore/biometric vault in Kotlin | secure-element and FIDO2 keys (later) |
| Host-key trust | verdicts, prompts bound to the presented fingerprint, Kotlin persistence and UI | per-host trust shared by several addresses (M2) |
| Session lifecycle | state machine, handle/driver split, `connect` over russh with PTY shell, timeouts, keepalive | `Connecting -> Connected` for channel sessions (lane 0); sessions as channels of a host connection (lane A1) |
| Frames | libghostty-vt adapter, full/delta merge, notify-once mailbox, Canvas drawing | |
| Input | libghostty key encoding, IME, keys row, scrolling, selection | bracketed paste |

## Ownership and threading

```text
┌──────────── Kotlin (lanes B, C) ─────────────┐        ┌─────────── Rust (lane A) ────────────┐
│ Room: hosts, trusted host keys, key records  │        │ or2-ffi Session ── SessionHandle     │
│ Keystore: encrypted private keys             │ UniFFI │            commands │   ▲ state,     │
│ App-scoped holder owns Session objects       │───────▶│                     ▼   │ mailbox    │
│ SessionListener ◀── Rust-owned thread ───────│◀───────│ SessionDriver (session task/thread)  │
│ Canvas renderer · IME · keys row             │        │   Transport · russh · libghostty-vt  │
└──────────────────────────────────────────────┘        └──────────────────────────────────────┘
```

- Kotlin owns each `Session` object. In M1 an application-scoped holder owns them, so Activity
  recreation does not end sessions; process death does. M3 moves the holder into the
  foreground service without changing this contract.
- `disconnect()` ends a session; `close()` on the Kotlin object (or dropping the last Rust
  reference) also disconnects. Both produce `Closed { Disconnected }`.
- `Session` methods never block and may be called from any thread, including inside listener
  callbacks. Each validates against the current state and enqueues a command for the driver.
- Listener callbacks run on a Rust-owned thread, never on the caller's thread, never
  concurrently for one session, and in order. They must return quickly (post to the main or
  render thread). A Kotlin `Exception` from a callback is ignored and does not affect the
  session; a Kotlin `Error` is not caught by the generated code, so do not throw one.
- Callbacks can begin before the factory (`connect`, or `contract_probe_session` today) has
  returned and before Kotlin has stored the `Session`. Listeners must not assume the handle is
  already assigned: record or post the event and defer work that needs the handle (for
  example `take_frame()` or `approve_host_key()`) until it is available.
- The initial state is `Connecting` and is not delivered as a change. `Closed` is delivered
  exactly once and last; Rust then releases the listener, which breaks the reference cycle
  through Kotlin. If the driver ends without closing, the handle reports
  `Closed { Failed { Internal } }`.
- Rust has no storage and keeps nothing after a session closes.

## Transport

`or2_core::transport::Transport` turns an `Endpoint` (validated host and nonzero port) into a
byte stream whose bound matches `russh::client::connect_stream`. Dropping the future cancels;
callers apply timeouts. Errors are `io::Error` for the session to classify. `DirectTcp` uses OS
sockets with `TCP_NODELAY`, resolving the host and trying each address in order. No other code
opens sockets.

## Key material

- `generate_ed25519_key(comment)` and `import_private_key(bytes, passphrase)` return
  `ClientKeyMaterial { private_key, public_key: PublicKeyInfo }`.
- `private_key` is the storage form: an unencrypted OpenSSH private key with LF line endings.
  Kotlin encrypts it with a hardware-backed Keystore key behind biometric unlock, decrypts it
  only to build a `ConnectRequest`, and zero-fills its `ByteArray` after the call returns or throws.
  A passphrase is needed only at import.
- Import accepts OpenSSH-format Ed25519, ECDSA (P-256/384/521) and RSA keys. Errors:
  `Malformed`, `UnsupportedFormat` (PEM/PKCS#8; convert with `ssh-keygen -p`),
  `PassphraseRequired`, `WrongPassphrase`, `UnsupportedAlgorithm` (DSA, `sk-*`).
- `PublicKeyInfo.openssh` is the `authorized_keys` line; `fingerprint` is `SHA256:…` exactly as
  `ssh-keygen -l -E sha256` prints it.
- Kotlin zero-fills every private-key `ByteArray` it holds, in a `finally`, once Rust no longer
  needs it: the imported file bytes after `import_private_key` returns or throws,
  `ClientKeyMaterial.private_key` after encrypting it, and the decrypted bytes after the
  synchronous `connect` (or probe) call returns or throws. The generated `ConnectRequest` keeps
  a reference to the array rather than a copy, so wiping it after constructing the request but
  before the call would erase the key before UniFFI serializes it.
- Never log, print or string-format `ClientKeyMaterial` or `ConnectRequest`. They are
  generated Kotlin data classes whose `toString()` is not redacted: it prints the private key
  bytes (`Arrays.toString`). Only the Rust `Debug` implementations are redacted.
- Rust zeroizes the key inputs it receives (`Zeroizing`) and parsed keys (`ssh-key`
  zeroize-on-drop). Known limits: the outgoing `ClientKeyMaterial.private_key` buffer and
  UniFFI's marshaling buffers are freed without being wiped, and a passphrase is a JVM `String`
  that cannot be wiped.

## Host-key trust

- `ConnectRequest.trusted_host_keys` carries the OpenSSH public key lines Kotlin trusts for the
  host. Trust is keyed by key data (algorithm and key), never by fingerprint text or comment.
- The session compares the presented key with them. Trusted: continue silently. Otherwise the
  state becomes `AwaitingHostKeyDecision { presented, previously_trusted }`; an empty
  `previously_trusted` means first use, a nonempty one means the key changed. Nothing continues
  until the user decides. Host certificates fail with `UnsupportedHostKey` in M1.
- To trust, Kotlin persists `presented.openssh` (on a change, replacing the old keys for that
  host), then calls `approve_host_key(presented.fingerprint)`. A different fingerprint fails
  with `HostKeyMismatch`, binding the decision to the key the user saw. `reject_host_key()`
  closes with `HostKeyRejected`.
- The server's login grace time bounds the wait; expiry arrives as `ConnectionLost`.
- Agent forwarding stays off.

## Session

M1 exports (replaced in M2 by `connect_host` plus `HostConnection.open_terminal`):

```rust
#[uniffi::export]
pub fn connect(request: ConnectRequest, listener: Box<dyn SessionListener>)
    -> Result<Arc<Session>, ConnectError>;
```

It validates synchronously (`ConnectError`: `InvalidHost`, `InvalidPort`, `InvalidUsername`,
`InvalidPrivateKey`, `InvalidTrustedHostKey { index }`, `EmptyDimension`) and returns at once;
everything else arrives through the listener.

```text
Connecting ──▶ AwaitingHostKeyDecision ──▶ Authenticating ──▶ Connected
    │   └──────────────────────────────────────▲                  │
    └──────────────┴───────────────────────────┴──────────────────┴──▶ Closed { reason }
```

`CloseReason`: `Disconnected`, `RemoteExited { exit_status }`, or `Failed { failure }` with
`SessionFailure`: `Unreachable`, `TimedOut`, `HostKeyRejected`, `UnsupportedHostKey`,
`AuthenticationRejected`, `ShellRejected` (PTY or shell refused), `ConnectionLost`, `Protocol`,
`Internal`. Messages are diagnostics without secrets, not for matching. Lane A maps transport
`io::Error`s to `Unreachable`, its connect timeout to `TimedOut`, russh auth failure to
`AuthenticationRejected`, EOF with an exit status to `RemoteExited` and other loss after
connecting to `ConnectionLost`.

Synchronous method errors (`SessionError`): `NotConnected` (input before `Connected`), `Closed`,
`NoHostKeyPrompt`, `HostKeyMismatch`, `EmptyDimension`, `InvalidKey`. `resize` is allowed from
`Connecting` on (latest wins; the PTY opens at that size). Input, `scroll` and
`request_full_frame` require `Connected`. The user-input queue is unbounded. Replies the terminal
generates for the host (for example to cursor-position or colour queries) are coalesced per
output batch and limited to 64 KiB queued or in flight; exceeding that closes the session with
`Failed { Protocol }` rather than dropping replies or growing without bound. Reading and
writing run concurrently, so a large paste never blocks output.

## Frames

The renderer pulls; Rust never queues frames.

1. The driver publishes frames into a mailbox as output arrives. Deltas merge by row index into
   the pending frame; a full frame replaces it. Memory is bounded by one viewport.
2. `on_frame_ready` fires when the mailbox goes from empty to non-empty, and not again until
   `take_frame()` empties it. The renderer takes on its next vsync.
3. `take_frame()` returns the merged `TerminalFrame` or `null`. `sequence` starts at 1 and
   increases by one per take. Frames published before `Closed` remain takeable, so the last
   screen can stay visible.

`TerminalFrame`: `columns`, `rows`, `full`, `styles` (deduplicated table), `changed_rows`
(ascending; all rows when `full`), `cursor` (`null` when hidden), `background`, `scrollback`
(`total_rows`, `offset` of the first viewport row; at the bottom when
`offset + rows == total_rows`). Each `TerminalRow` has exactly `columns` cells and a `wrapped`
flag (soft-wrapped into the next row). `TerminalCell`: `text` (one grapheme cluster; empty for
blanks and tails), `width` (`NARROW`, `WIDE` head spanning two columns, `SPACER_TAIL` with no
text) and `style` (index into `styles`).

Style indices are scoped to their own frame's `styles` table. When applying a delta, resolve
each changed cell's style through that delta's table; rows the delta does not contain keep the
styles they were resolved with earlier. Never reinterpret a cached row's indices through a
later frame's table.

Colours are `0x00RRGGBB` and fully resolved: default colours, palette, inverse and invisible
are applied in Rust, so Kotlin draws `foreground` on `background`. `CellStyle` keeps `bold`,
`italic`, `faint` (reduced opacity), `strikethrough`, `overline`, `underline` (`NONE`,
`SINGLE`, `DOUBLE`, `CURLY`, `DOTTED`, `DASHED`) and an optional `underline_color` (else the
foreground). Blink is not rendered in M1. The cursor gives its leftmost column, `wide` when it
covers a wide grapheme, `shape` (`BLOCK`, `BLOCK_HOLLOW`, `BAR`, `UNDERLINE`), `blinking` and
`color`.

Full frames are sent first after `Connected`, after every resize, when libghostty reports the
whole screen dirty (alternate screen switch, clear, palette change), and after
`request_full_frame()`. The renderer calls `request_full_frame()` whenever it creates a new
view for a connected session. A delta always matches the size of the grid the renderer holds.

Lane A's libghostty-vt adapter follows the render-state API: build rows from dirty rows (all
rows when `Dirty::Full`), then clear both dirty layers; map `SpacerHead` to a blank narrow cell;
use `cell.fg_color()`/`bg_color()` falling back to the terminal colours, swap for inverse, set
foreground to background for invisible; resolve palette underline colours; report a cursor at
a wide tail at its head column; take scrollback from `Terminal::scrollbar()`. libghostty
objects are `!Send`: keep each terminal on one thread and pass bytes to it over channels.
Default colour changes (OSC 10/11, reverse screen) republish every row. The grid size is owned
by Kotlin's `resize`: DECCOLM 80/132-column switching is unsupported, so any native geometry
change is reverted to the requested size and followed by a full frame.

## Input and resize

- `send_text(text)`: committed IME text. Rust writes it as UTF-8 with `\r\n` and `\n` mapped
  to `\r`. Composing text stays in Kotlin (drawn as an overlay at the cursor) until committed.
  Clipboard paste through the IME arrives here too; bracketed paste is not part of M1.
- `send_key(KeyInput { key, modifiers })`: keys row, hardware keys and modifier combinations.
  `TerminalKey` names Enter, Tab, Backspace, Escape, Insert, Delete, Home, End, PageUp,
  PageDown, the arrows, `Function { number }` (1–12) and `Character { text }`: the unmodified
  character, e.g. `c` for Ctrl+C. Rust maps ASCII characters to US-layout physical keys and
  encodes everything with libghostty's encoder using the terminal's current modes. In legacy mode
  (no Kitty keyboard flags, modifyOtherKeys off) Ctrl+`[`, Ctrl+`i` and Ctrl+`m` are sent as
  Escape, Tab and Enter, as xterm does, rather than Ghostty's CSI-u forms. Only presses
  are sent. IME `deleteSurroundingText(before, after)` becomes `before` Backspace keys and
  `after` Delete keys.
- `resize(columns, rows)`: Kotlin derives the grid from view size and cell metrics. Rust
  resizes the terminal and the PTY (`window-change`) and publishes a full frame.
- `scroll(ViewportScroll)`: `Top`, `Bottom`, `Delta { rows }` (negative is up). Kotlin sends it
  for every scroll gesture; in the alternate screen Rust translates it for the application.
- Selection in M1 is Kotlin-local over the displayed grid: join `wrapped` rows without a
  newline and trim trailing blanks.

## Contract probe

`contract_probe_session(request, listener)` is a test fixture, not a connection. It validates a
real `ConnectRequest` and returns a real `Session` whose driver is a deterministic script on a
Rust thread: it presents a host key generated once per process, checks it against the
request's trusted keys with the production trust code, waits for the decision, then renders
fixed cells (styles, a combining mark, CJK and emoji wide cells, a wide bar cursor) and echoes
input: row 2 shows the bytes `send_text` would write, row 3 the validated key. App code must
never call it. The JVM tests (`SessionContractTest`, `KeyContractTest`) and the device test
(`NativeDeviceTest`) use it and the key exports against the real native library.

# M2: hosts, multiplexers and mosh

M2 replaces "one `connect` = one SSH connection = one shell" with **one SSH connection per
host** that carries everything for that host: terminal channels, exec channels for tmux and
probing, and streamlocal channels to herdr. One connection means one biometric unlock and one
host-key decision per host, and sub-second terminal opens once the host is connected. FFI API
becomes **4**. The M1 sections above still govern frames, input, key material, trust and the
`Session` object; this section records what changes. Rust remains authoritative: when code and
this text disagree, fix one of them in the same change.

## Lanes and integration

| Lane | Owns | Depends on |
|---|---|---|
| 0: contract gate | `or2-core` `host`, `remote`, `herdr::view`, `tmux` types; `or2-ffi` API 4 surface; `contract_probe_host`; Kotlin JVM contract test | this document |
| A1: host connection | `ssh.rs` refactor into the host driver, address racing, exec, capability probe, tmux, terminal targets, `connect_host` | lane 0 |
| A2: herdr | `herdr` client: generated types, discovery, bootstrap/reconcile, projection, `watch` and `focus_pane` | lane 0 (`RemoteHost`) |
| A3: mosh core | vendored mosh-rs, `DatagramTransport`, `Screen` over libghostty, bootstrap, mosh session driver; **no FFI export** | lane 0 (`RemoteHost`) |
| B: Android | Room v2, host connection holder, multiple sessions, inbox, host screen with tmux picker, navigation | lane 0 (generated bindings, probe) |

Lane 0 has landed. A2 and A3 land independently. A1 and B land together: A1 removes the M1
`connect` export and B removes its Kotlin use. `connect_host` exists from lane 0 so Kotlin
compiles against it, but until A1 lands it closes every connection with
`Failed { Internal }` (never a pretend success). Likewise lane 0's `herdr::watch` and
`herdr::focus_pane` report "not integrated" failures until A2 replaces them.

## Remote commands (`or2_core::remote`)

Every exec and socket open goes through one trait so herdr, tmux and mosh code is testable
without SSH:

```rust
pub trait RemoteHost: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    /// Runs `command` without a PTY; collects stdout, stderr and the exit status.
    fn exec(&self, command: &RemoteCommand) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send;
    /// Opens a byte stream to a Unix socket on the host (OpenSSH direct-streamlocal).
    fn open_unix(&self, path: &str) -> impl Future<Output = Result<Self::Stream, RemoteError>> + Send;
}
```

- `RemoteCommand` is a program, arguments and environment assignments, never a shell string.
  sshd hands the rendered string to the user's login shell (bash, zsh or fish), so it renders
  as `env 'K=V' … 'program' 'arg' …`: every token single-quoted, an embedded `'` written as
  `'\''`. That form means the same in POSIX shells and fish provided no token contains a
  backslash or a control character, so rendering rejects those (`RemoteError::Unquotable`).
  Details lane 0 settled: without environment assignments there is no `env` prefix
  (`'program' 'arg' …`); `Unquotable` also covers an environment name outside
  `[A-Za-z_][A-Za-z0-9_]*` and a program that is empty, starts with `-` or contains `=`
  (`env` would misread it). A round-trip test runs the rendered string through `sh`, bash,
  zsh and fish (each skipped when not installed; fish is not installed on the runner that
  landed lane 0) and checks the argv arrives exactly.
  Fixed scripts (the capability probe) run as `sh -c '<script>'` (`remote::render_script`,
  which rejects a script containing `'` or `\`; newlines are allowed). Untrusted values
  (session names, pane ids) are only ever separate arguments.
- Lane 0 put the tmux and probe result types in `or2_core::host` (`TmuxSession`,
  `HostCapabilities`, `HerdrSessionInfo`) next to the other host types; there is no separate
  `tmux` module yet (lane A1 adds the command code). The output cap and exec timeout are the
  constants `remote::OUTPUT_CAP` and `remote::EXEC_TIMEOUT`.
- `ExecOutput { status: Option<u32>, stdout: Vec<u8>, stderr: Vec<u8> }`; output is capped at
  1 MiB per stream (excess fails with `RemoteError::OutputTooLarge`); each exec has a 10 s
  timeout (`RemoteError::TimedOut`).
- `RemoteError`: `Closed`, `TimedOut`, `OutputTooLarge`, `Unquotable`, `Rejected(String)` (channel refused,
  e.g. streamlocal forwarding disabled), `Io(String)`.
- `LocalHost` (feature `test-support`, enabled for or2-core's own tests through a dev-dependency
  on itself; used by integration tests) implements `RemoteHost` with local processes and
  `UnixStream`. `exec` runs the *rendered* string through `/bin/sh -c`, so quoting is exercised
  exactly as on a host; it enforces the output cap and timeout (`LocalHost::with_timeout`
  shortens the latter for tests, `exec_rendered` runs an already rendered line such as a
  fixed script). Production code never uses it.

### Capability probe

Non-interactive SSH does not load the user's `PATH`. One exec per connection, cached in memory
for the connection's lifetime (Rust has no storage), finds `tmux`, `herdr` and `mosh-server`:
`command -v`, then `$HOME/.local/bin`, `$HOME/.cargo/bin`, `/opt/homebrew/bin`,
`/usr/local/bin`, `/usr/bin`, `/bin`, `$HOME/.nix-profile/bin`, `/run/current-system/sw/bin`.
It also reports a UTF-8 locale (`C.UTF-8`, else the first `*.UTF-8`/`*.utf8` in `locale -a`,
else `en_US.UTF-8`). Every later tmux/herdr/mosh command uses the absolute path found.

## Host connection (`or2_core::host`)

A handle/driver split exactly like `session`: `HostHandle` (wrapped by FFI `HostConnection`)
never blocks and enqueues `HostCommand`s; `HostDriver` owns state changes and calls the
`HostObserver` on its own thread, in order, without holding locks.

```text
Connecting ──▶ AwaitingHostKey ──▶ Authenticating ──▶ Connected { address_index }
    │   └──────────────────────────────────▲                      │
    └─────────────┴────────────────────────┴──────────────────────┴──▶ Closed { reason }
```

- `HostConnectRequest { addresses: Vec<Endpoint> (1..=8, preference order), username, key,
  trusted_host_keys }`. Trust belongs to the host, not to an address.
- **Address racing.** Start address 0; start each next address 250 ms after the previous one
  started or immediately when it fails. The first TCP connection wins; the others are dropped.
  If all fail, close with `Unreachable` whose message lists each address's error (no secrets).
  The SSH handshake runs only on the winner. `Connected.address_index` reports which one won.
- Host-key relay, connect timeout (20 s, paused while awaiting the user), keepalive (15 s, 3
  misses) and failure mapping are M1's. `CloseReason` is reused; `RemoteExited` never occurs
  for a host.
- Closing a host (user `disconnect`, release of the last handle, or loss) closes every terminal
  session on it (`Disconnected` when the user disconnected the host, else the host's failure),
  stops every herdr watch, and fails pending queries with `Closed`.
- Queries are allowed only in `Connected` (`NotConnected` before, `Closed` after).
- **Commands carry drivers, not replies, for handle-returning operations.** `open_terminal` and
  `watch_herdr` return their handle synchronously (the FFI methods are not `async`), so
  `HostHandle` creates the `session::channel` / `herdr::channel` pair itself and sends the
  *driver* to the host driver:
  `HostCommand::OpenTerminal { target, size, driver: SessionDriver }` and
  `HostCommand::WatchHerdr { session, driver: HerdrWatchDriver }`. The host driver opens the
  channel (or starts `herdr::run`) and drives it; if it cannot, it closes that driver with a
  `Failed` reason. Only the queries (`Capabilities`, `ListTmux`) carry a tokio `oneshot`
  reply; a dropped reply means `Closed`. (The lane 0 brief listed `observer` and `reply`
  fields on the first two; that cannot return a handle without blocking.) Target and session
  names are validated in `HostHandle` before the pair is created, so a rejected request
  creates no session and fires no callback.
- `HostDriver::transition(Closed)` stops accepting commands and then fails every command still
  queued: terminal drivers close with the host's reason (`Disconnected` for a user disconnect,
  else the host failure), herdr drivers close, query replies are dropped (`Closed`). Sessions
  and watches the driver already started are its own to close, before it closes the host.
- Dropping a `HostDriver` closes with `Failed { Internal }`; dropping the last `HostHandle`
  enqueues `Disconnect`; both as for `session`.
- Rust API: `HostConnectRequest::new(addresses: &[(&str, u16)], username, private_key,
  trusted_host_keys)` validates in order: address count (`NoAddresses`, `TooManyAddresses`),
  each address (`InvalidAddress { index, error }`; FFI keeps only `index`), username, trusted
  keys (`InvalidTrustedHostKey { index }`), key. `connect_host` (lane 0) closes from a Rust
  thread with `Failed { Internal("host connections land with lane A1") }`.

### Terminal sessions on a host

`open_terminal(target, size, observer) -> SessionHandle` opens a PTY channel and returns a
`session::SessionHandle`. Its lifecycle is `Connecting → Connected → Closed` (lane 0 adds the
`Connecting → Connected` transition; host-key and authentication states never occur on a
channel session). Everything in the M1 Session, Frames and Input sections applies per session:
one terminal engine per session on its own thread, resize latest-wins, the 64 KiB reply
budget, concurrent read/write. Closing a session closes only its channel.

`TerminalTarget`:

| Target | Remote command (PTY, `TERM=xterm-256color`) |
|---|---|
| `Shell` | the login shell (`request_shell`) |
| `Tmux { session_name }` | `<tmux> -u new-session -A -s <name>` (attach or create) |
| `Herdr { session, pane_id }` | if `pane_id`: `herdr::focus_pane` first; then `<herdr>` (default session) or `<herdr> --session <name>` |

Names are validated before anything runs (`InvalidName`): tmux names nonempty, at most 128
bytes, no control characters, `\`, `:` or `.`; herdr session names `[A-Za-z0-9_-]{1,64}`; pane
ids `[A-Za-z0-9:_-]{1,128}`. A missing program fails with `NotInstalled { program }`.

### tmux

`list_tmux_sessions()` runs `<tmux> list-sessions -F` with fields joined by U+001F:
`session_name`, `session_windows`, `session_attached`, `session_created`,
`session_activity`. "no server running" / "no sessions" is an empty list, not an error.
`TmuxSession { name, windows: u32, attached_clients: u32, created_unix: i64,
activity_unix: i64 }`, sorted by most recent activity.

## herdr (`or2_core::herdr`)

- **Types** are generated from `herdr api schema --json` by `scripts/gen-herdr-types.sh`
  (normalize: extract `schemas.*`, rewrite `$ref`s; then cargo-typify). The normalized schema
  and the generated `herdr/generated.rs` are checked in with the herdr version they came from.
  Deserialization ignores unknown fields; unknown enum values map to an `Unknown` variant
  rather than failing the whole message. Never hand-edit the generated file.
- **Discovery:** `<herdr> session list --json` gives each session's name, `running` and
  `socket_path`. Never hard-code socket paths. `HostCapabilities.herdr_sessions` reports them.
- **Watch:** `herdr::watch(host, session, observer) -> HerdrWatchHandle`. Opens one long-lived
  streamlocal event channel and short-lived request channels. Bootstrap: `events.subscribe`
  and wait for its ack, then `session.snapshot`; events arriving during a read are
  invalidations, not patches: install the snapshot, then do serialized authoritative refreshes,
  repeating while another event arrived mid-read. Subscribe per pane to
  `pane.agent_status_changed` as panes appear. On `events_lost` or a dropped event channel,
  resubscribe and re-snapshot.
- **States** (`HerdrState`): `Starting` (initial, not delivered), `Live { view }`,
  `Unavailable { reason, message }`, `Closed`. `reason`: `NotInstalled` and
  `IncompatibleProtocol { protocol }` are final; `NotRunning` and `Failed` retry every 10 s
  while the host is connected (the state is redelivered only when it changes). `Closed` is
  delivered once, last, after `stop()` or host close, then the observer is released.
- **Rust API** (lane 0): `herdr::channel(observer) -> (HerdrWatchHandle, HerdrWatchDriver)`
  (the same split as `session`: the handle's `state()` and `stop()` never block, the driver
  delivers states in order and `Closed` exactly once, then releases the observer; dropping the
  driver closes it; dropping every handle stops it). `herdr::watch(host, session, observer)`
  is `channel` plus a task running `herdr::run(host, session, driver)` on the process
  runtime; the host driver uses `channel` and `run` itself so the handle can be returned
  synchronously. A state equal to the current one is not redelivered; after a final
  `Unavailable` only `Closed` is accepted. `focus_pane` returns `HerdrError`
  (`NotIntegrated`, `Remote`, `Failed`). Until lane A2, `run` reports
  `Unavailable { Failed, "herdr client not integrated" }` and waits for the stop, and
  `focus_pane` always fails with `NotIntegrated`.
- **View** (`HerdrView`) is or2's projection, delivered whole, coalesced to at most one
  delivery per 100 ms: `version`, `protocol`, `focused_pane_id`, `workspaces`
  (`workspace_id, number, label, focused, agent_status`), `tabs` (`tab_id, workspace_id,
  number, label, focused, agent_status`), `panes` (`pane_id, tab_id, workspace_id, label,
  agent, agent_status, cwd, title, focused`) and `agents` (`pane_id, tab_id, workspace_id,
  name, agent, display_agent, status, cwd, title, focused, state_change_seq`). `AgentStatus`:
  `Idle`, `Working`, `Blocked`, `Done`, `Unknown`. Field types follow herdr's schema:
  `version: u64` is or2's own counter (it increases with every delivery of one watch),
  `protocol: u32` is herdr's protocol number, `focused_pane_id` and pane/agent `label`, `name`,
  `agent`, `display_agent`, `cwd`, `title` are optional, `state_change_seq` is `u64`; the FFI
  records are `HerdrView`, `HerdrWorkspace`, `HerdrTab`, `HerdrPane`, `HerdrAgent`.
- **Focus:** `herdr::focus_pane(host, session, pane_id)` sends one `pane.focus` request. It
  changes what the user's herdr clients show; tests use isolated named sessions only.
- Tests never touch the default herdr session or any session they did not create. Live tests
  start `herdr --session or2-test-<unique> server` and stop/delete it afterwards.

## mosh core (`or2_core::mosh`, no FFI in M2)

- Vendor the mosh-rs library (pinned `90b37125f5e4a598be91dec37d23921b6865276e`, GPL-3.0-or-later)
  without its CLI front ends or the `vt100-screen` feature; record it in
  `THIRD_PARTY_NOTICES.md` and keep upstream file headers.
- UDP goes through a new `DatagramTransport` trait in `transport.rs` (`DirectUdp`). mosh code
  never binds sockets itself. Roaming rebinds through the transport.
- mosh-rs's `Screen` is implemented over or2's libghostty `TerminalEngine`, so terminal state
  stays in libghostty and frames reach Kotlin exactly as for SSH. Local-echo prediction is off
  in M2.
- `mosh::bootstrap(host: &impl RemoteHost, caps, size, target)` runs
  `LANG=<utf8> <mosh-server> new -s -c 256 -l LANG=<utf8> -- <target command>` and parses
  `MOSH CONNECT <port> <key>`. The key is a secret: `Zeroizing`, redacted `Debug`, never
  logged.
- `mosh::start(params, host_name, observer) -> SessionHandle` drives the standard session
  lifecycle (`Connecting → Connected → Closed`). M3 adds the FFI choice and network-health
  reporting.

## FFI API 4 (`or2-ffi`)

```rust
#[derive(uniffi::Record)] pub struct HostAddress { pub host: String, pub port: u16 }
#[derive(uniffi::Record)] pub struct HostConnectRequest {
    pub addresses: Vec<HostAddress>, pub username: String,
    pub private_key: Vec<u8>, pub trusted_host_keys: Vec<String>,
}
#[uniffi::export] pub fn connect_host(request: HostConnectRequest, listener: Box<dyn HostListener>)
    -> Result<Arc<HostConnection>, HostConnectError>;
// HostConnectError: NoAddresses, TooManyAddresses, InvalidAddress { index }, InvalidUsername,
//                   InvalidPrivateKey, InvalidTrustedHostKey { index }

pub enum HostState { Connecting, AwaitingHostKeyDecision { presented, previously_trusted },
                     Authenticating, Connected { address_index: u32 }, Closed { reason: CloseReason } }
#[uniffi::export(callback_interface)] pub trait HostListener: Send + Sync {
    fn on_host_state_changed(&self, state: HostState) -> Result<(), ListenerError>;
}

#[derive(uniffi::Object)] pub struct HostConnection;
impl HostConnection {                     // all non-blocking unless async
    fn state(&self) -> HostState;
    fn approve_host_key(&self, fingerprint: String) -> Result<(), HostError>;
    fn reject_host_key(&self) -> Result<(), HostError>;
    fn disconnect(&self);
    fn open_terminal(&self, target: TerminalTarget, columns: u16, rows: u16,
                     listener: Box<dyn SessionListener>) -> Result<Arc<Session>, HostError>;
    async fn capabilities(&self) -> Result<HostCapabilities, HostError>;
    async fn list_tmux_sessions(&self) -> Result<Vec<TmuxSession>, HostError>;
    fn watch_herdr(&self, session: Option<String>, listener: Box<dyn HerdrListener>)
        -> Result<Arc<HerdrWatch>, HostError>;
}
// HostError: NotConnected, Closed, NoHostKeyPrompt, HostKeyMismatch, EmptyDimension,
//            InvalidName, NotInstalled { program }, CommandFailed { reason }
// HostCapabilities { tmux: Option<String>, herdr: Option<String>, mosh_server: Option<String>,
//                    utf8_locale: String, herdr_sessions: Vec<HerdrSessionInfo { name, running, is_default }> }

#[uniffi::export(callback_interface)] pub trait HerdrListener: Send + Sync {
    fn on_herdr_state_changed(&self, state: HerdrState) -> Result<(), ListenerError>;
}
#[derive(uniffi::Object)] pub struct HerdrWatch;   // fn state(&self) -> HerdrState; fn stop(&self);
```

- `CommandFailed` carries `reason`, not `message`: a UniFFI error variant field named
  `message` generates a Kotlin property that clashes with `Throwable.message`. `EmptyDimension`
  is raised by the FFI layer (core takes a validated `TerminalSize`), like `SessionError`'s.
  `HerdrState` in the FFI includes `Starting` (what `HerdrWatch.state()` can return before the
  first change; it is never delivered). The herdr records and `HerdrWatch` live in
  `or2-ffi/src/herdr.rs`, the host surface in `or2-ffi/src/host.rs`.
- Async methods use UniFFI's tokio async runtime support and become Kotlin `suspend` functions;
  cancelling the Kotlin coroutine cancels the Rust future. Callbacks keep M1's threading rules:
  a Rust-owned thread, never concurrent per object, in order, may start before the factory
  returns, released after the final `Closed`.
- `HostConnectRequest` follows M1's private-key rules: Kotlin wipes the array after the
  synchronous `connect_host` returns or throws, never logs the record, and Rust zeroizes its
  copy.
- `contract_probe_host(request, listener)` is the test fixture for this surface (no network):
  it uses the production trust check with a per-process host key, reports
  `Connected { address_index: 0 }`, answers `capabilities` and `list_tmux_sessions` with fixed
  data, opens terminals through the M1 probe script, and drives a `HerdrWatch` through
  `Live` (a fixed view with one blocked, one working and one idle agent), one update (the
  blocked agent becomes working) and `Closed` on `stop()`. App code must never call it.
  Details: the probe host runs on one Rust thread with a single-threaded runtime, which also
  serves its terminals and watches, so callbacks are ordered and never concurrent per object.
  `open_terminal` targets show in row 0 (`or2 contract probe shell`, `... tmux <name>`,
  `... herdr <session|default> <pane|->`); a watch's first workspace label is its session name
  (`default` for `None`); the capabilities have no `mosh_server`, a running `default` herdr
  session and a stopped `or2-probe` one; tmux lists `main` (3 windows, 1 client) before
  `build`. Closing the host (user disconnect, or releasing the object) closes its terminals
  (`Disconnected`) and watches first, then reports the host's `Closed`.
- API 4 enables UniFFI's `tokio` feature (new locked dependency `async-compat`) and gives
  `or2-core` tokio's `process` feature (new locked dependency `signal-hook-registry`), both
  MIT/Apache-2.0; `or2-ffi` also depends on tokio directly for the probe runtime.
- `contract_probe_session` stays for the M1 session tests; it now runs on the same kind of
  probe thread (behaviour unchanged).

## Android (lane B)

- **Room v2** with a real `Migration(1, 2)` (never destructive: Keystore-bound keys cannot be
  recreated). New `host_addresses(hostId → hosts.id ON DELETE CASCADE, position, hostname,
  port, PRIMARY KEY(hostId, position))`; the migration moves each host's `hostname`/`port` to
  position 0 and drops those columns. `hosts` gains `showInInbox` (default true). Export the
  schema (`exportSchema = true`, `app/schemas/`) and add a `MigrationTestHelper` device test.
- Any change to a host's address list or ports clears its trust (M1's rule, generalised).
- **Holder:** an application-scoped `HostConnections` keeps at most one `HostConnection` per
  host and any number of terminal sessions per connection. Unlocking: one biometric prompt per
  distinct key record; hosts sharing a key are connected from one decryption, wiping the array
  after the last `connect_host` call. Host-key prompts move from sessions to hosts.
- **Screens:** Inbox (start destination): agents across all `showInInbox` hosts, blocked first,
  each with host, workspace/tab, agent and status; tap opens a `Herdr { session, pane_id }`
  terminal. Host screen: connection state, Shell, tmux sessions (attach, new by name), herdr
  sessions. Terminal: the M1 terminal screen per session, plus a switcher between open
  sessions. Host form: ordered address list.
