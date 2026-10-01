# M1 shared contracts

These are the interfaces the M1 lanes build against: Rust sessions and terminal, Android
host/key/trust, and Canvas/IME. The Rust source is authoritative (`core/or2-core/src/*.rs` for
behaviour, `core/or2-ffi/src/*.rs` for what Kotlin sees); this document records the decisions
and the rules the types cannot express. Changing a contract means changing the code, this
document and the tests together, and bumping `API_VERSION` in `or2-ffi` when an export changes.

## Status

All M1 contracts below are implemented and tested. M2 changes are specified in
[M2: hosts, multiplexers and mosh](#m2-hosts-multiplexers-and-mosh); lane 0 has landed the
M2 contract types and the FFI API 4 surface (`API_VERSION = 4`), and lane A1 has landed the
host driver behind `connect_host` (address racing, host connection, probe, tmux, terminal
targets); the lane A2 and B behaviour is still to come.

| Contract | Implemented and tested | Open |
|---|---|---|
| Transport | `Transport` trait, `DirectTcp`, `Endpoint` validation, address racing (`transport::race`) | UDP (M2 core, M3 export), jump host |
| Key material | Ed25519 generation, OpenSSH import with passphrase, storage form, typed errors; Keystore/biometric vault in Kotlin | secure-element and FIDO2 keys (later) |
| Host-key trust | verdicts, prompts bound to the presented fingerprint, Kotlin persistence and UI; one trust set per host shared by all its addresses (`HostConnectRequest`) | |
| Session lifecycle | state machine, handle/driver split, `connect` over russh with PTY shell, timeouts, keepalive | removal of the M1 `connect` export (with lane B; [checklist](#removing-the-m1-path)) |
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
`AuthenticationRejected`, `ShellRejected` (PTY, shell or, on a host, the session channel refused), `ConnectionLost`, `Protocol`,
`Internal` (M2 adds `NotInstalled { program }` and `CommandFailed`, for terminals on a host).
Messages are diagnostics without secrets, not for matching. Lane A maps transport
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

Lanes 0, A2 and A1 have landed. A3 lands independently. A1 and B land together: the lead
removes the M1 `connect` export (kept, marked "M1 path: removed when lane B lands", so the
app keeps working) once B removes its Kotlin use. `connect_host` is real since A1, and
`herdr::run`, `watch` and `focus_pane` are the real client since A2; the host driver treats a
failed `focus_pane` as `CommandFailed`.

### Removing the M1 path

The M1 `connect` path stays compiled until lane B has removed its Kotlin use. Deleting it
touches these places, so none is left dead:

- `or2-ffi/src/session.rs`: the `connect` export and its `ConnectRequest` record (bump
  `API_VERSION`, regenerate the bindings), `ConnectContractTest.kt` and the app's production
  connector.
- `or2-core/src/ssh.rs`: `connect`, `start`, `drive`, `network`, `shell`,
  `authenticated_shell` and the `From<HostKeyRequest>`/`From<TransportEnd>` impls for the M1
  `Event`; `session::ConnectRequest` if nothing else uses it.
- `or2-core/src/ssh/pump.rs`: the M1-only `Event` variants `HostKey`, `Authenticating` and
  `TransportEnded`, and their no-op arms in `ssh/terminal_session.rs` (`drive`).
- Tests: `ssh_tests.rs` (the M1 driver tests; the reply-budget test there imports
  `super::pump::GeneratedReplies`, so move it next to `pump.rs` instead of dropping it) and
  `tests/openssh.rs` (`host.rs` covers the host path). The prompt-time behaviour M1 tests
  there (timer pause, EOF during the prompt) has host-path tests in
  `ssh/connection_tests.rs`.
- `docs/build.md`: the sentences that describe the M1 connector and `openssh.rs`.

## Remote commands (`or2_core::remote`)

Every exec and socket open goes through one trait so herdr, tmux and mosh code is testable
without SSH:

```rust
pub trait RemoteHost: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    /// The primitive: runs an already rendered command line, as sshd hands it to the login
    /// shell, without a PTY; collects stdout, stderr and the exit status.
    fn exec_rendered(&self, line: &str) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send;
    /// Provided: `exec_rendered(command.render()?)`.
    fn exec(&self, command: &RemoteCommand) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send;
    /// Provided: `exec_rendered(render_script(script)?)`, for fixed multi-line scripts.
    fn exec_script(&self, script: &str) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send;
    /// Opens a byte stream to a Unix socket on the host (OpenSSH direct-streamlocal). A socket
    /// that is missing or refuses the connection is `Io`; `Rejected` is only for a host that
    /// refuses the channel itself (streamlocal forwarding disabled).
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
  zsh and fish and checks the argv arrives exactly. It starts zsh with `-f` and fish with
  `--no-config`, so the user's startup files cannot change the output. A shell that is not
  installed is skipped (fish is not installed on the runner that landed lane 0), unless
  `OR2_REQUIRE_SHELLS` is set, which fails the test instead: set it in CI so the cross-shell
  claim is never verified vacuously.
  Fixed scripts (the capability probe) run as `sh -c '<script>'` through
  `RemoteHost::exec_script` (`remote::render_script`, which rejects a script containing `'` or
  `\`; newlines are allowed). `exec_script` and `exec` both reach `exec_rendered`, the one
  method a `RemoteHost` implements, so a probe written over `RemoteHost` runs on `LocalHost`
  too. Untrusted values (session names, pane ids) are only ever separate arguments.
- Lane 0 put the tmux and probe result types in `or2_core::host` (`TmuxSession`,
  `HostCapabilities`, `HerdrSessionInfo`) next to the other host types; lane A1 added the
  command code as `or2_core::tmux` and `or2_core::probe`, both generic over `RemoteHost`. The
  output cap and exec timeout are the constants `remote::OUTPUT_CAP` and
  `remote::EXEC_TIMEOUT`.
- The SSH `RemoteHost` (lane A1) runs each exec on its own session channel without a PTY and
  sends EOF at once, so a command that reads stdin ends. Output over the cap, a refused
  command and the timeout close that channel before failing (`OutputTooLarge`, `Rejected`,
  `TimedOut`). A channel that ends with neither an exit status nor a signal is `Closed` when
  the connection is gone. `open_unix` is `direct-streamlocal@openssh.com`; a refusal (the
  target does not exist, streamlocal forwarding is disabled) is `Rejected`. The exec timeout
  also bounds `open_unix`.
- `ExecOutput { status: Option<u32>, stdout: Vec<u8>, stderr: Vec<u8> }`; output is capped at
  1 MiB per stream (excess fails with `RemoteError::OutputTooLarge`); each exec has a 10 s
  timeout (`RemoteError::TimedOut`).
- `RemoteError`: `Closed`, `TimedOut`, `OutputTooLarge`, `Unquotable`, `Rejected(String)` (channel refused,
  e.g. streamlocal forwarding disabled), `Io(String)`.
  **`open_unix` error mapping** (the herdr watch depends on it: `Io` becomes `NotRunning`,
  `Rejected` becomes `Failed`): a missing socket or one that refuses the connection is
  `Io`; `Rejected` is a refusal by the host's policy. Over SSH both arrive as a channel-open
  failure, told apart by its reason code: `SSH_OPEN_ADMINISTRATIVELY_PROHIBITED` is
  `Rejected`; `SSH_OPEN_CONNECT_FAILED` (nothing listens, no such file) is `Io`; any other
  reason is `Rejected`. Lane A1's OpenSSH interop test covers a stale socket path (must be
  `Io`) next to a host with streamlocal forwarding disabled (must be `Rejected`).
- `LocalHost` (feature `test-support`, enabled for or2-core's own tests through a dev-dependency
  on itself; used by integration tests) implements `RemoteHost` with local processes and
  `UnixStream`. `exec_rendered` runs the *rendered* line through `/bin/sh -c`, so quoting is
  exercised exactly as on a host; it enforces the output cap and timeout
  (`LocalHost::with_timeout` shortens the latter for tests). Production code never uses it,
  so the feature, not the production build, enables tokio's `process` support
  (`test-support = ["tokio/process"]`).

### Capability probe

Non-interactive SSH does not load the user's `PATH`. One exec per connection, cached in memory
for the connection's lifetime (Rust has no storage), finds `tmux`, `herdr` and `mosh-server`:
`command -v`, then `$HOME/.local/bin`, `$HOME/.cargo/bin`, `/opt/homebrew/bin`,
`/usr/local/bin`, `/usr/bin`, `/bin`, `$HOME/.nix-profile/bin`, `/run/current-system/sw/bin`.
It also reports a UTF-8 locale (`C.UTF-8`, else the first `*.UTF-8`/`*.utf8` in `locale -a`,
else `en_US.UTF-8`). Every later tmux/herdr/mosh command uses the absolute path found: the
host driver passes `HostCapabilities.herdr` to `herdr::run`, `herdr::watch` and
`herdr::focus_pane` (as `mosh::bootstrap` takes `caps`), so no client repeats the PATH search.
The probe is a fixed script run with `exec_script`.

Lane A1: `or2_core::probe::PROBE_SCRIPT` and `probe::parse` (`probe::probe(&host)` runs them).
The script prints `or2:tmux:<path>`, `or2:herdr:<path>`, `or2:mosh-server:<path>` and
`or2:locale:<name>`; `parse` is lenient (unknown lines are ignored, a relative path or one
with `\`, `=` or a control character counts as not installed, odd or missing locale falls
back to `en_US.UTF-8`). A path with a space is fine (one quoted token). `C.UTF-8` is reported
when `locale -a` lists `C.UTF-8` or `C.utf8`; otherwise the first `*.UTF-8`/`*.utf8` entry as
listed. The script contains no `'` or `\` (a test enforces it, because `render_script`
rejects them). A probe that fails to run is not cached, so the next query or watch retry runs
it again.

Lane A1 decisions, both from review: (1) herdr's session list is **not** part of the script.
When herdr was found, `probe::herdr_sessions` runs `<herdr> session list --json` as a second
exec bounded by `probe::HERDR_LIST_TIMEOUT` (5 s), so a wedged herdr costs only
`herdr_sessions` (empty), never tmux, mosh-server or the locale, and never makes the probe
fail and be retried at full cost on every terminal open. Only a connection that closes
mid-probe fails it. JSON is read for each session's `name`, `running` and `default`, unknown
fields ignored. (2) The cache holds programs and locale; **`capabilities()` reads herdr's
session list afresh on every call** (`probe::with_fresh_sessions`), so `running` and sessions
started or stopped after connecting show on the host screen. A listing that fails (herdr
gone, hung, garbage) keeps the last list rather than emptying it. Live per-pane state is
still `watch_herdr`'s job; terminal opens and watches use only the cached paths.

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
  creates no session and fires no callback. That includes a request that loses the race with
  a closing host: if the command queue refuses the command, the handle recovers it and
  *discards* its driver (`SessionDriver::discard`, `HerdrWatchDriver::discard`: closed
  silently, observer released without a call), then returns `HostError::Closed`. Kotlin sees
  the error and no callback, never a `Closed` on its own calling thread.
- **Failing a terminal or watch.** A terminal the host driver cannot open closes with a
  `SessionFailure`: `NotInstalled { program }` when the probe found no `tmux`/`herdr`,
  `CommandFailed` when a helper command (the herdr `focus_pane` before a pane open) failed,
  otherwise the matching M1 failure (`ShellRejected`, `ConnectionLost`, `Internal`). A watch
  with no herdr reports `Unavailable { NotInstalled }`. `HostError::NotInstalled` and
  `CommandFailed` therefore come out of the two queries only; Kotlin need not check
  `capabilities()` before opening a terminal.
- **Query bounds.** The driver must answer or drop every query reply. As a safety net the
  handle gives up after `host::QUERY_TIMEOUT` (30 s, longer than the 10 s exec timeout) with
  `HostError::CommandFailed`. Cancelling a Kotlin `suspend` call only drops the receiving end:
  the driver's `reply.send` must tolerate `Err`, and the exec it started runs to its own end.
- `HostDriver::transition(Closed)` stops accepting commands and then fails every command still
  queued: terminal drivers close with the host's reason (`Disconnected` for a user disconnect,
  else the host failure), herdr drivers close, query replies are dropped (`Closed`). Sessions
  and watches the driver already started are its own to close, before it closes the host.
- Dropping a `HostDriver` closes with `Failed { Internal }`; dropping the last `HostHandle`
  enqueues `Disconnect`; both as for `session`.
- Rust API: `HostConnectRequest::new(addresses: &[(&str, u16)], username, private_key,
  trusted_host_keys)` validates in order: address count (`NoAddresses`, `TooManyAddresses`),
  each address (`InvalidAddress { index, error }`; FFI keeps only `index`), username, trusted
  keys (`InvalidTrustedHostKey { index }`), key.
- **Lane A1 implementation.** `ssh::connect_host(request, observer)` is `connect_host_with`
  over `DirectTcp` and `HostOptions::default()`; `connect_host_with<T: Transport>` takes any
  transport and `HostOptions { connect_timeout (20 s), exec_timeout (10 s), stagger (250 ms) }`
  so tests need not wait for production timings. The driver is `ssh/connection.rs`: a thread
  `or2-host` (`HostDriver`, commands, callbacks) plus a network task (`transport::race`, the
  host-key relay of M1, handshake, authentication, then holding the connection) that reports
  to it, and an `Arc<SshHost>` (the russh `Handle`, the exec timeout and the cached probe)
  that is the `RemoteHost` shared by terminals, queries and herdr watches. Each terminal is
  its own thread `or2-terminal` (below); queries and watches are tasks on the process runtime.
  An address that wins the race but whose SSH handshake then fails is not retried on the
  next address: racing is only over TCP, as specified.
- **Closing order.** Whatever ends the host is one `CloseReason`: `Disconnected` for a user
  disconnect (or the release of the last handle), the failure for loss, `Unreachable` when
  every address fails (its message lists each address by position and error kind, never by
  name), `TimedOut` when the 20 s connect timeout expires (paused while the user decides on a
  host key), `HostKeyRejected`, `AuthenticationRejected`, `UnsupportedHostKey`. The driver
  hands that reason to every terminal and watch, waits (up to 3 s) until each has delivered
  its own `Closed`, tears the connection down (a user disconnect first sends the SSH
  disconnect and each terminal's channel close), and only then reports the host's `Closed`.
  So the callback order is always: terminals and watches `Closed`, then the host `Closed`,
  exactly as `contract_probe_host`. A terminal closes with the host's reason: `Disconnected`
  or, on loss, the host's `ConnectionLost` (a channel that dies with the connection waits
  up to 250 ms for the host's reason so both report the same cause). Queries still running
  are dropped, so their callers see `HostError::Closed`. The wait for terminals and watches
  is bounded (3 s) so one wedged session, for example a terminal thread stuck in a slow
  observer callback, cannot hold the host open forever: past the bound the host's `Closed`
  can arrive **before** that session's. The order is therefore guaranteed unless a session
  is wedged; Kotlin callbacks are documented to return quickly, so it should not happen.
- **A network task that ends without reporting** (it panicked; a buggy `Transport` is
  re-raised by `race`) closes the host with `Failed(Internal)` instead of leaving it in its
  last state, so terminals and watches are told and exactly one `Closed` is reported.
- **Session channels are limited by the server.** Every terminal and every exec in flight
  (probe, tmux listing, herdr client) is one session channel on the host's single
  connection, and OpenSSH's `MaxSessions` defaults to 10 per connection. A refused channel
  open is not a lost connection: a terminal closes with `ShellRejected` (the connection
  stays `Connected`; Kotlin must not reconnect), and an exec fails with `Rejected`
  (`CommandFailed` from a query). A dropped (cancelled) exec closes its channel, so it does
  not keep counting against the limit.
- **One slow terminal slows the connection.** russh delivers channel data to a bounded
  per-channel queue from the one loop that serves the whole SSH session, so a terminal that
  stops draining its output (a very slow engine or a blocking observer callback) eventually
  stalls every other channel on that host: other terminals, queries (which then hit their
  10 s timeout), the herdr socket and keepalive replies. Backpressure is kept on purpose: the
  alternative, buffering without bound or failing a terminal that falls behind, would break
  ordinary output floods (`cat` of a large file) to protect a rare case. Keep observers
  quick; there is no per-terminal buffer to tune.
- **The private key is dropped after authentication**; the connection keeps only what it
  needs, not the `ClientKey`.
- `ssh::connect_tapped` and `ssh::SshRemote` (feature `test-support`) hand an integration
  test the established connection as a `RemoteHost`, to test exec limits and streamlocal
  against a real sshd without a second code path.

### Terminal sessions on a host

`open_terminal(target, size, observer) -> SessionHandle` opens a PTY channel and returns a
`session::SessionHandle`. Its lifecycle is `Connecting → Connected → Closed` (lane 0 adds the
`Connecting → Connected` transition; host-key and authentication states never occur on a
channel session). Everything in the M1 Session, Frames and Input sections applies per session:
one terminal engine per session on its own thread, resize latest-wins, the 64 KiB reply
budget, concurrent read/write. Closing a session closes only its channel.

`TerminalTarget` (the default herdr session is `session: None`; the entry of
`HostCapabilities.herdr_sessions` with `is_default` is that session, so Kotlin passes `None`
for it, never `Some(name)`, which runs `herdr --session <name>` and may differ. Otherwise the
inbox would watch one session twice):

| Target | Remote command (PTY, `TERM=xterm-256color`) |
|---|---|
| `Shell` | the login shell (`request_shell`) |
| `Tmux { session_name }` | `<tmux> -u new-session -A -s <name>` (attach or create) |
| `Herdr { session, pane_id }` | if `pane_id`: `herdr::focus_pane` first; then `<herdr>` (default session) or `<herdr> --session <name>` |

Names are validated before anything runs (`InvalidName`): tmux names nonempty, at most 128
bytes, no control characters, `\`, `:` or `.`; herdr session names `[A-Za-z0-9_-]{1,64}`; pane
ids `[A-Za-z0-9:_-]{1,128}`. A missing program closes the session with
`SessionFailure::NotInstalled { program }` (FFI `SessionFailure.NotInstalled(program)`); a failed
herdr pane focus closes it with `SessionFailure::CommandFailed` (FFI
`SessionFailure.CommandFailed(message)`, a diagnostic).

Lane A1 (`ssh/terminal_session.rs`, `ssh/pump.rs`): the session is a thread `or2-terminal`
owning the engine and the `SessionDriver`, plus a task owning the channel; `ssh/pump.rs` is the
terminal pump M1's one-connection session shares (`TerminalPump`: engine, reply budget,
commands, frames; `pump_channel`: concurrent channel read and write). Order: for tmux and herdr
the probe (cached), then for a herdr pane the focus, then the channel (`pty-req` at the
requested size, `shell` or `exec`, then a window change if the size changed meanwhile), then
`Connected`. Anything that fails before the channel exists (missing program, failed focus,
failed probe) closes the session from `Connecting` without opening one. Every other way a
session ends (user disconnect, host close, protocol failure) closes its channel first, so the
program does not outlive the session on the host: a tmux client detaches, the tmux session
lives on. The program's own exit is `RemoteExited { exit_status }`. Channel setup (open,
`pty-req`, `shell`/`exec`, each awaiting the server's reply) is bounded by the host's exec
timeout (10 s): a server that never answers closes the session with `TimedOut`, after
closing the channel. A refused channel open (`MaxSessions`, see above) is `ShellRejected`,
never `ConnectionLost`.

### tmux

`list_tmux_sessions()` runs `<tmux> -u list-sessions -F <format>` (`or2_core::tmux`).
Lane A1 decision: the fields are joined by `:` with the name last (`#{session_windows}:
#{session_attached}:#{session_created}:#{session_activity}:#{session_name}`), not by U+001F,
because a control character cannot appear in a command line `RemoteCommand` renders for every
login shell. tmux turns `:` and `.` in names into `_`, so the delimiter never occurs in a
name, and the name being last keeps even a surprising one from shifting the numeric fields.
`-u` (also on `new-session`) makes tmux treat the terminal as UTF-8 whatever locale the
non-interactive login shell has, so non-ASCII names round-trip. "no server running", a missing
or stale socket ("error connecting to ... (No such file or directory)"), "no sessions" and
"server exited unexpectedly" are an empty list, not an error; any other failure is
`HostError::CommandFailed` with tmux's first stderr line. Lines that do not parse are skipped.
`TmuxSession { name, windows: u32, attached_clients: u32, created_unix: i64,
activity_unix: i64 }`, sorted by most recent activity (ties by name). tmux uses its default
socket; tests isolate it with `TMUX_TMPDIR` in the environment the commands run in.

## herdr (`or2_core::herdr`)

- **Types** are generated from `herdr api schema --json` by `scripts/gen-herdr-types.sh`
  (`scripts/herdr_schema.py` normalizes, then cargo-typify). The normalized schema
  `herdr/schema.json` (with the herdr version and protocol it came from: 0.9.3, protocol 22)
  and `herdr/generated.rs` are checked in; `generated.rs` has one module per schema family
  (`request`, `success_response`, `error_response`) and the
  constants `HERDR_VERSION` and `PROTOCOL`. Never hand-edit it: fix the script and
  regenerate (`--offline` regenerates from the checked-in schema, `--check` verifies both
  files). Normalization decisions: `$ref`s become local; validation keywords a client has no use
  for (`pattern`, `propertyNames`, `maxProperties`, `minProperties`) are dropped, so one odd map
  key cannot fail a snapshot and no `regress` dependency is needed; in every family herdr
  *sends*, a string enum becomes `oneOf [enum, string]`, which typify renders as an untagged
  enum whose second variant holds any unknown value (the projection maps it to
  `AgentStatus::Unknown`); the bundle's `request` (an `id` plus a `oneOf` of `{method,
  params}`) is split so typify generates an adjacently tagged `RequestBody` enum, and
  `herdr::wire` adds the `id`. Unknown fields are ignored (no `deny_unknown_fields`).
  *Event payloads are not typed at all*: events are invalidations, so the stream reader
  classifies a line by its top-level key (`error`, `result`, `event`) and never fails on an
  event it cannot decode. The schema's `event` and `subscription_event` families are not
  generated: their internally tagged `EventData` rejects an unknown event type, which would
  break the unknown-value rule for a later consumer (notifications) that trusted it. That
  consumer adds the families to `scripts/herdr_schema.py` with an open event-type tag.
- **Discovery:** `<herdr> session list --json` over `RemoteHost::exec` gives each session's name,
  `default`, `running` and `socket_path` (or2 reads only those; other fields are ignored).
  `herdr::list_sessions(host, herdr) -> Result<Vec<SessionEntry>, DiscoveryError>` is the one
  parser and is public: lane A1's capability probe builds `HostCapabilities.herdr_sessions`
  (`name`, `running`, `is_default`) from it, and the watch and `focus_pane` locate a socket
  through it. `SessionEntry { name, default, running }` is public; `socket_path` is
  crate-private. A
  `None` session is the entry with `default: true`, `Some(name)` the entry with that name.
  Never hard-code socket paths. Exit status 126/127 is `NotInstalled`; exit status 2, herdr's
  usage-error status, is `Unsupported` (a herdr that predates `session list --json`), which
  the watch reports as `IncompatibleProtocol { protocol: 0 }` (0: older than any protocol
  number; final); another failure or unreadable output is `Failed`; a missing or stopped
  session is `NotRunning`.
  `HostCapabilities.herdr_sessions` reports name, `running` and `is_default`; `socket_path` is
  not reported to Kotlin. `run` and `focus_pane` rediscover it themselves on every attempt,
  using the herdr path they are given.
- **Watch:** `herdr::watch(host, herdr, session, observer) -> HerdrWatchHandle` is `channel`
  plus a task running `run`. One *attempt*: discover; open one long-lived streamlocal event
  stream and `events.subscribe` (bounded by the 10 s request timeout), wait for the ack;
  then reconcile: `session.snapshot` on a separate short-lived stream, install it, and read
  again while an event arrived during the read (events are invalidations, never patches).
  Reads are at least 100 ms apart and events that arrive before a read starts are covered by
  it. Any line on the event stream other than the ack, including an unparseable or unknown
  one, is an invalidation.
  - *Subscriptions* (minimal set that keeps the view current): `workspace.created`, `.updated`,
    `.renamed`, `.moved`, `.reordered`, `.closed`, `.focused`; `tab.created`, `.closed`,
    `.focused`, `.renamed`, `.moved`; `pane.created`, `.closed`, `.updated`, `.focused`,
    `.moved`, `.exited`, `.agent_detected`; plus `pane.agent_status_changed` per pane (herdr
    requires the `pane_id`). Not subscribed: `workspace.metadata_updated`, `worktree.*`,
    `layout.updated`, `pane.output_matched`, `pane.scroll_changed` (nothing in the view depends
    on them).
  - *Panes appearing.* A subscription cannot grow, and herdr rejects the whole request, and
    closes the stream, if a named pane no longer exists. The first stream therefore carries
    only the lifecycle subscriptions. After each installed snapshot, if a pane `(pane_id,
    terminal_id)` is not covered, a new stream with the lifecycle subscriptions and every
    current pane replaces the old one, and the view is read again (events between the read and
    the new subscription are lost). A rejection (a pane closed meanwhile) re-reads the panes and
    retries; the fourth consecutive rejection fails the attempt (`Failed`). Only the
    `pane_not_found` error code means a pane vanished. Any other rejection of the per-pane
    request leaves the view `Live` on the stream that is still open (agent status then changes
    only through lifecycle-driven reads, and the request is tried again with every later read),
    so a persistent rejection cannot make the view flap between `Live` and `Unavailable`.
  - *Connect cost.* The first view is installed after one subscribe and one snapshot, as in
    the contract sequence; the per-pane resubscribe and its confirming read follow it, and
    every later new pane costs one more of each (a subscription cannot grow). Not yet
    measured over an OpenSSH-backed host: measure when lane A1 lands.
  - *Recovery.* `events_lost` (an `error` line on the event stream, after which herdr closes it),
    any other error line, or a dropped stream ends the attempt with a 500 ms pause and a new
    attempt (rediscover, subscribe, snapshot). The last view stays delivered meanwhile; the
    state changes only if the new attempt fails.
- **Protocol:** the snapshot's `protocol` must be at least `generated::PROTOCOL` (22); an older
  herdr is `IncompatibleProtocol { protocol }` (final), a newer one is accepted (herdr adds
  fields, which are ignored). A snapshot that cannot be read is `Failed`. A herdr older than
  the protocol may fail before it ever answers a snapshot: if the first (lifecycle-only)
  `events.subscribe` is rejected with a herdr error, the watch reads one snapshot to check the
  protocol, and an older one is `IncompatibleProtocol` instead of `Failed`.
- **States** (`HerdrState`): `Starting` (initial, not delivered), `Live { view }`,
  `Unavailable { reason, message }`, `Closed`. `reason`: `NotInstalled` and
  `IncompatibleProtocol { protocol }` are final; `NotRunning` (no such session, stopped, or its
  socket refuses connections) and `Failed` (anything else: unreadable output, timeout, refused
  channel, rejected request) retry every 10 s while the host is connected. `Unavailable` is
  redelivered only when its *reason* changes, so a message that differs between attempts does
  not repeat it. `Closed` is delivered once, last, after `stop()` or host close
  (`RemoteError::Closed` from any call), then the observer is released. A stop interrupts every
  wait, including a read in flight. **The host driver ends its watches itself.** The watch
  learns of a lost host only from a `RemoteError::Closed` that one of its calls returns, which
  can take a retry or resubscribe pause, and a watch parked at a final `Unavailable` makes no
  call at all. A host driver that holds only the `HerdrWatchDriver` (it passes it by value to
  `run`) therefore ends a watch on host close by aborting or dropping the `run` task: dropping
  the driver delivers `Closed`, once, last (tested under abort in every state), and the handle
  Kotlin holds then reads `Closed`.
- **Delivery.** `HerdrView` is delivered whole, at most once per 100 ms (the first view at once,
  a burst collapses to its latest view), and only when it differs from the last delivered view
  (compared without `version`, which is or2's own counter: it starts at 1 and increases with
  every delivery of one watch). After `Unavailable` the next `Live` is always delivered.
- **Rust API.** `herdr::channel(observer) -> (HerdrWatchHandle, HerdrWatchDriver)` (the same split
  as `session`: the handle's `state()` and `stop()` never block, the driver delivers states in
  order and `Closed` exactly once, then releases the observer; dropping the driver closes it;
  dropping every handle stops it). `herdr::run(host, herdr, session, driver)` drives a driver
  until it stops (`herdr` is the absolute path from the capability probe; a caller whose probe
  found none reports `Unavailable { NotInstalled }` itself and does not call it); the host driver
  uses `channel` and `run` itself so the handle can be returned synchronously. A state equal to
  the current one is not redelivered; after a final `Unavailable` only `Closed` is accepted.
  `focus_pane` returns `HerdrError` (`Remote`, `Failed`; lane A2 removed lane 0's
  `NotIntegrated`, which has no meaning any more). The retry, resubscribe and coalescing
  intervals live in `herdr::watch::Timing` (crate-private; unit tests shorten them or run under
  a paused clock; integration tests reach it through the `#[doc(hidden)]`
  `herdr::watch_with_timing` and `herdr::Timing`, behind the `test-support` feature).
  `herdr::list_sessions`, `SessionEntry` and `DiscoveryError` are public for lane A1.
- **View** (`HerdrView`) is or2's projection of the snapshot, in herdr's order: `version`,
  `protocol`, `focused_pane_id`, `workspaces` (`workspace_id, number, label, focused,
  agent_status`), `tabs` (`tab_id, workspace_id, number, label, focused, agent_status`), `panes`
  (`pane_id, tab_id, workspace_id, label, agent, agent_status, cwd, title, focused`) and `agents`
  (`pane_id, tab_id, workspace_id, name, agent, display_agent, status, cwd, title, focused,
  state_change_seq`). `AgentStatus`: `Idle`, `Working`, `Blocked`, `Done`, `Unknown`. Field types
  follow herdr's schema: `version: u64`, `protocol: u32`, `focused_pane_id` and pane/agent
  `label`, `name`, `agent`, `display_agent`, `cwd`, `title` are optional, `state_change_seq` is
  `u64`; the FFI records are `HerdrView`, `HerdrWorkspace`, `HerdrTab`, `HerdrPane`,
  `HerdrAgent`. `cwd` is the pane's `cwd` (not `foreground_cwd`) and `title` its `title`
  (not `terminal_title`).
- **Focus:** `herdr::focus_pane(host, herdr, session, pane_id)` rediscovers the socket and sends
  one `pane.focus` request on a short-lived stream (10 s bound). It changes what the user's
  herdr clients show; tests use isolated named sessions only. An error response (for example
  `pane_not_found`) is `HerdrError::Failed`.
- **Tests.** Unit tests run the watch against `herdr::testing::FakeHost`, a scripted
  `RemoteHost` (a fake herdr over in-memory streams) under tokio's paused clock, with sanitized
  fixtures in `herdr/fixtures/` (captured from isolated test sessions; the agent, unknown-value
  and `events_lost` fixtures are written from the schema and the socket documentation). They
  cover bootstrap order, events during a read, coalescing, `events_lost`, dropped streams,
  the 10 s retry, final reasons, rejected subscriptions and stop. The live test
  (`tests/herdr_live.rs`) starts `herdr --session or2-test-<pid>-<n> server` with every
  `HERDR_*` variable removed (so it starts empty and cannot reach the user's session), drives it
  through its socket, and stops and deletes exactly that session (its restart test shortens the
  10 s retry through `watch_with_timing`, and a stop waits until the session's socket is gone); it is skipped with a message
  when herdr is absent unless `OR2_REQUIRE_HERDR` is set. Tests never touch the default herdr
  session or any session they did not create.

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
    /// Programs and locale are probed once per connection; `herdr_sessions` is read afresh.
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
- API 4 enables UniFFI's `tokio` feature (new locked dependency `async-compat`) and adds
  `SessionFailure.NotInstalled { program }` and `SessionFailure.CommandFailed { message }`;
  `or2-ffi` also depends on tokio directly for the probe runtime. `or2-core`'s
  `test-support` feature (not the shipped library) enables tokio's `process` feature (locked
  dependency `signal-hook-registry`); both new dependencies are MIT/Apache-2.0.
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
