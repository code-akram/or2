# M1 shared contracts

These are the interfaces the M1 lanes build against: Rust sessions and terminal, Android
host/key/trust, and Canvas/IME. The Rust source is authoritative (`core/or2-core/src/*.rs` for
behaviour, `core/or2-ffi/src/*.rs` for what Kotlin sees); this document records the decisions
and the rules the types cannot express. Changing a contract means changing the code, this
document and the tests together, and bumping `API_VERSION` in `or2-ffi` when an export changes.

## Status

All M1 contracts below are implemented and tested. M2 changes are specified in
[M2: hosts, multiplexers and mosh](#m2-hosts-multiplexers-and-mosh); lane 0 has landed the
M2 contract types and the FFI API 4 surface (`API_VERSION` 5 since the M1 `connect` was
removed, 6 with `focus_herdr_pane`, 7 with `Session.submit_text`); lanes A1 (host driver behind `connect_host`: address
racing, host connection, probe, tmux, terminal targets), A2 (herdr client), A3 (mosh core, no FFI
export) and B (the Android app) have all landed, and the M1 `connect` export is gone (see
[The M1 path is gone](#the-m1-path-is-gone)).

| Contract | Implemented and tested | Open |
|---|---|---|
| Transport | `Transport` trait, `DirectTcp`, `Endpoint` validation, address racing (`transport::race`) | UDP (M2 core, M3 export), jump host |
| Key material | Ed25519 generation, OpenSSH import with passphrase, storage form, typed errors; Keystore/biometric vault in Kotlin | secure-element and FIDO2 keys (later) |
| Host-key trust | verdicts, prompts bound to the presented fingerprint, Kotlin persistence and UI; one trust set per host shared by all its addresses (`HostConnectRequest`, per-host trust in the app) | |
| Session lifecycle | state machine, handle/driver split, terminals as channels of a host connection, timeouts, keepalive | |
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

A `Session` is one terminal. The app gets it from `HostConnection.open_terminal` (see
[FFI API 6](#ffi-api-6-or2-ffi)); M1's `connect` export, which made one SSH connection per
shell, is gone. `contract_probe_session(ConnectRequest, listener)` is the only other source (a
test fixture, see [Contract probe](#contract-probe)); it validates synchronously
(`ConnectError`: `InvalidHost`, `InvalidPort`, `InvalidUsername`, `InvalidPrivateKey`,
`InvalidTrustedHostKey { index }`, `EmptyDimension`) and returns at once; everything else
arrives through the listener.

```text
Connecting ──▶ AwaitingHostKeyDecision ──▶ Authenticating ──▶ Connected
    │   └──────────────────────────────────────▲                  │
    └──────────────┴───────────────────────────┴──────────────────┴──▶ Closed { reason }
```

A terminal on a host goes straight from `Connecting` to `Connected`: host-key and
authentication states belong to the `HostConnection`. Only the probe session walks the full
diagram.

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
  Clipboard paste through the IME arrives here too; this path never brackets a paste.
- `submit_text(text)` (API 7): the composer's send. It types `text` and presses Enter such that
  agent TUIs with paste-burst detection (Codex, Claude Code) see a *submit*, not a pasted
  newline. Those programs treat a fast burst of input that contains Enter as a paste, so
  `send_text(text + "\n")` or text immediately followed by an Enter key inserts a literal
  newline and never submits. The driver therefore (1) writes the text: when the terminal has
  bracketed paste on (DECSET 2004, read from libghostty's modes at that moment), as
  `ESC[200~ text ESC[201~` with newlines as typed and any `ESC[201~` inside the text removed (so
  it cannot end the paste early and inject what follows), otherwise as typing with `send_text`'s
  newline mapping; then (2) after `SUBMIT_ENTER_DELAY` (100 ms, `or2_core::submit`), as a
  *separate write*, writes Enter encoded by the key encoder with the modes of that moment
  (Kitty and modifyOtherKeys apply, as for `send_key`). This is what herdr's `agent prompt`
  does. Empty text writes nothing for step 1 and presses just Enter. The pause is a driver timer,
  not a sleep: output, resizes and disconnects are served meanwhile, and input sent after the
  submit (`send_text`, `send_key`, scroll translated to keys, another submit) is held and written
  after the Enter, in order. Requires `Connected` (`NotConnected` before, `Closed` after), like
  `send_text`. The SSH and mosh drivers implement it through `SubmitSequencer`; over mosh the two
  writes are separate user-stream states but the server may batch them on a high-latency link,
  which can still defeat burst detection there.
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
input: row 2 shows the bytes `send_text` would write (`text c3 a9 0d`) or, for `submit_text`, the
typed text bytes then the separate Enter (`submit c3 a9 0d 78 | 0d`; the probe terminal never
enables bracketed paste and has no delay), row 3 the validated key. App code must
never call it. The JVM tests (`SessionContractTest`, `KeyContractTest`) and the device test
(`NativeDeviceTest`) use it and the key exports against the real native library.

# M2: hosts, multiplexers and mosh

M2 replaces "one `connect` = one SSH connection = one shell" with **one SSH connection per
host** that carries everything for that host: terminal channels, exec channels for tmux and
probing, and streamlocal channels to herdr. One connection means one biometric unlock and one
host-key decision per host, and sub-second terminal opens once the host is connected. FFI API
became **4**, and **5** when the M1 `connect` export was removed. The M1 sections above still govern frames, input, key material, trust and the
`Session` object; this section records what changes. Rust remains authoritative: when code and
this text disagree, fix one of them in the same change.

## Lanes and integration

| Lane | Owns | Depends on |
|---|---|---|
| 0: contract gate | `or2-core` `host`, `remote`, `herdr::view`, `tmux` types; `or2-ffi` API 4 surface (now 6); `contract_probe_host`; Kotlin JVM contract test | this document |
| A1: host connection | `ssh.rs` refactor into the host driver, address racing, exec, capability probe, tmux, terminal targets, `connect_host` | lane 0 |
| A2: herdr | `herdr` client: generated types, discovery, bootstrap/reconcile, projection, `watch` and `focus_pane` | lane 0 (`RemoteHost`) |
| A3: mosh core | vendored mosh-rs, `DatagramTransport`, `Screen` over libghostty, bootstrap, mosh session driver; **no FFI export** | lane 0 (`RemoteHost`) |
| B: Android | Room v2, host connection holder, multiple sessions, inbox, host screen with tmux picker, navigation | lane 0 (generated bindings, probe) |

All lanes have landed. `connect_host` is the host driver (A1), `herdr::run`, `watch` and
`focus_pane` are the real client (A2; the host driver treats a failed `focus_pane` as
`CommandFailed`), mosh is a core-only client with no FFI export yet (A3), and the app (B) uses
`connect_host` and `HostConnection.open_terminal` exclusively. The M1 `connect` export was
removed when A1 and B were integrated.

### The M1 path is gone

When A1 and B were integrated the M1 single-session path was removed, and `API_VERSION` became
**5**: the `connect` export; `ssh::connect` and its single-session driver (`start`, `drive`,
`network`, `shell`, `authenticated_shell`); the M1-only pump `Event` variants (`HostKey`,
`Authenticating`, `TransportEnded`) and their no-op arms; `ssh_tests.rs` and
`tests/openssh.rs`; `ConnectContractTest.kt` and the app's old connector. Their coverage moved:
the reply-budget unit test sits next to `ssh/pump.rs`; the prompt-time timer and EOF behaviour
is covered by `ssh/connection_tests.rs`; RSA client keys, key input and certificate-only hosts
are covered by `tests/host.rs`. `ConnectRequest` (FFI record and `session::ConnectRequest`)
stays because `contract_probe_session` takes it; it is no longer a connection request.

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
  the connection is gone. `open_unix` is `direct-streamlocal@openssh.com`; a refusal is `Io` when
  the channel-open failure reason is `CONNECT_FAILED` and `Rejected` otherwise (mapping below).
  The exec timeout also bounds `open_unix`. **It bounds the whole exec exchange**: opening the
  channel, sending the exec request, sending EOF and collecting the output share one deadline
  (`SshHost::run_exec`; the exec request and the EOF await russh's bounded outbound queue, which
  a stalled connection fills, so they are no more immediate than the reads), and the guard that
  closes the channel is itself bounded (250 ms). The first exec of a connection is the
  capability probe, whose cache initializer therefore ends within the exec timeout (plus the
  5 s herdr listing): a timed-out probe is not cached and a caller waiting behind it is never
  stuck, it simply probes in turn. A test stalls the client's writes, fills the outbound queue,
  and checks the exec request, the probe and a second probe caller all end `TimedOut` in time
  and that the next probe after the path recovers succeeds.
- `ExecOutput { status: Option<u32>, stdout: SecretBytes, stderr: SecretBytes }`; output is
  capped at 1 MiB per stream (excess fails with `RemoteError::OutputTooLarge`); each exec has a
  10 s timeout (`RemoteError::TimedOut`). **Output is secret-bearing** (`mosh-server new` prints
  its session key on stdout), so the streams are `remote::SecretBytes`: a byte buffer that
  derefs to `[u8]`, shows only its length in `Debug`, and wipes itself when dropped. The SSH
  collector appends through `SecretBytes::extend_capped`, which wipes the block it gives up
  when it grows (a `Vec` reallocation would free it unwiped) and refuses an append past the
  cap. Every way out of an exec drops what was collected, so it is wiped on success (when the
  caller drops the output), refusal, timeout, output cap, connection loss and a cancelled
  future alike (tests drive each path against the in-process server with a marker secret).
  This covers or2's own buffers; russh's packet payloads arrive in its own zeroizing
  `CryptoVec`, and the SSH transport's internal buffers are outside it.
- `RemoteError`: `Closed`, `TimedOut`, `OutputTooLarge`, `Unquotable`, `Rejected(String)` (channel refused,
  e.g. streamlocal forwarding disabled), `Io(String)`.
  **`open_unix` error mapping** (the herdr watch depends on it: for a session its listing calls
  running, both `Io` and `Rejected` become `Failed`, with different messages): a missing socket or one that refuses the connection is
  `Io`; `Rejected` is a refusal by the host's policy. Over SSH both arrive as a channel-open
  failure, told apart by its reason code: `SSH_OPEN_ADMINISTRATIVELY_PROHIBITED` is
  `Rejected`; `SSH_OPEN_CONNECT_FAILED` (nothing listens, no such file) is `Io`; any other
  reason (`UNKNOWN_CHANNEL_TYPE`, `RESOURCE_SHORTAGE`, an unknown code) is `Rejected`
  (`ssh::connection::streamlocal_error`; unit-tested for every reason against the in-process
  server). **OpenSSH caveat, found by the interop test:** OpenSSH (checked with 10.5) answers
  *every* refused direct-streamlocal open with `CONNECT_FAILED`, including one refused by
  `AllowStreamLocalForwarding no` or `DisableForwarding yes` (the sshd log says "refused
  streamlocal port forward"). Over OpenSSH a host that forbids streamlocal forwarding therefore
  reads as `Io` and cannot be told from a missing socket (the herdr watch therefore says so in
  its `Failed` message, see *Unreachable socket* under the watch); the
  `Rejected` path is for servers that do send `ADMINISTRATIVELY_PROHIBITED`. The interop tests
  in `tests/host.rs` cover a missing socket and a stale socket file (both `Io`) and a host with
  `AllowStreamLocalForwarding no` (refused, connection stays usable).
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
When herdr was found, `probe::herdr_sessions` lists the sessions with `herdr::list_sessions`
(lane A2's one parser of `<herdr> session list --json`; the probe has no parser of its own) as
a second exec bounded by `probe::HERDR_LIST_TIMEOUT` (5 s), so a wedged herdr costs only
`herdr_sessions` (empty), never tmux, mosh-server or the locale, and never makes the probe
fail and be retried at full cost on every terminal open. Only a connection that closes
mid-probe fails it. `name`, `running` and `default` are carried over; an unreadable listing
(including one with an entry that has no `socket_path`) is a failed listing, not a partial
one. (2) The cache holds programs and locale; **`capabilities()` reads herdr's
session list afresh on every call** (`probe::SessionsCache`), so `running` and sessions
started or stopped after connecting show on the host screen. A listing that fails (herdr
gone, hung, garbage) reports the **last list that was read successfully**, not the one from
connect time (until a read succeeds, the probe's own list): the app treats the list as
authoritative and stops the watch of a session missing from it, so a transient failure must
neither drop a session found since connecting nor bring back one that has gone. Reads can
overlap (several Refresh calls); each takes a ticket when it starts and a result is applied
only if no read that started later has already been applied, so a slow older read never
overwrites a newer list (and then reports the newer one). The programs and locale stay in the
immutable probe result. Live per-pane state is
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
  `Failed` reason. Only the queries (`Capabilities`, `ListTmux`, and API 6's `FocusHerdrPane`) carry a tokio
  `oneshot` reply; a dropped reply means `Closed`. (The lane 0 brief listed `observer` and `reply`
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
  with no herdr reports `Unavailable { NotInstalled }`. `HostError::NotInstalled`,
  `CommandFailed` (and `PaneNotFound`) therefore come out of the queries only; Kotlin need not check
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
  Each watch is one task racing `herdr::run` (or the no-herdr `Unavailable { NotInstalled }`
  wait) against the host's closing signal; when the signal wins the task's future is dropped,
  and dropping the `HerdrWatchDriver` delivers `Closed`, once, last. That is what ends a watch
  parked at a final `Unavailable` (no call is pending that could see the host go) and one
  that is `Live` mid-wait; tests cover both states against user disconnect and loss.
  The capability probe a watch needs first is raced against the watch's own stop too: stopping
  a watch while the probe runs (it can take the exec timeout plus the herdr listing) closes it
  at once, delivering `Closed` with no `Unavailable` on the way, and the abandoned probe is
  not cached (the next caller probes afresh); a host that closes mid-probe drops the whole
  watch task as above.
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

The pane focus in the last row happens once, when the terminal opens. It does not pin the
terminal to the pane: to reuse an agent terminal the app calls `focus_herdr_pane` (FFI section,
"Agent terminals and focus").

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
    measured over an OpenSSH-backed host: measure on the phone.
  - *Unreachable socket.* `session list --json` says `running: true` but opening its socket
    fails with `Io`: that is `Failed`, not `NotRunning`, because the listing already settled
    whether the session runs. The likeliest cause over OpenSSH is the host's forwarding policy
    (OpenSSH 10.5 answers a forbidden streamlocal open with `CONNECT_FAILED`, the same as a dead
    socket), so the message says: the session is listed as running but its socket cannot be
    reached over SSH, check `AllowStreamLocalForwarding` and `DisableForwarding` in the host's
    `sshd_config`. The watch keeps retrying every 10 s and recovers when the socket opens (a
    session that really stopped meanwhile is `NotRunning` on the next listing). Unit-tested with
    the scripted fake host, and end to end against sshd with `AllowStreamLocalForwarding no`.
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
  `IncompatibleProtocol { protocol }` are final; `NotRunning` (no such session, or the listing
  says it is stopped) and `Failed` (anything else: unreadable output, timeout, refused channel,
  rejected request, a running session's socket that cannot be opened) retry every 10 s while
  the host is connected. `Unavailable` is
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
  `focus_pane` returns `HerdrError` (`Remote`, `PaneNotFound`, `Failed`; lane A2 removed lane 0's
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
  herdr clients show; tests use isolated named sessions only. An error response with the code
  `pane_not_found` is `HerdrError::PaneNotFound` (the pane is gone); any other error response
  is `HerdrError::Failed`. The host exposes it as `HostHandle::focus_herdr_pane` (FFI API 6).
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

Lane A3 landed. `core/or2-core/src/mosh/` holds the vendored library under `ssp/` (see
`THIRD_PARTY_NOTICES.md`; each file's header names the pinned mosh-rs commit
`90b37125f5e4a598be91dec37d23921b6865276e` and what or2 changed) and or2's own modules
beside it: `bootstrap`, `ghostty`, `link`, `driver`.

```text
bootstrap ──▶ MoshParams ──▶ start / start_with ──▶ SessionHandle (+ LinkControl)
                                     │
                       driver thread (owns the terminal)
                          │                 │
                  ssp::Session         link ── DatagramTransport (DirectUdp)
                  (no sockets, no I/O)
                          │
                  ClientTerminal<GhosttyScreen>
```

**Vendored, not carried.** The CLI front ends, the platform terminal code, the `vt100-screen`
feature and the prediction engine (with its `vte` and `unicode-width` dependencies) are not
vendored. Local-echo prediction is off in M2; M3 can vendor the engine from the same commit if
wanted, though it needs `Screen` to read cells back. Dependencies added to the workspace,
pinned exactly: `aes 0.8.4`, `ocb3 0.1.0`, `base64 0.22.1`, `prost 0.14.4` (derive),
`flate2 1.1.10` (the version `Cargo.lock` already held; upstream's lock has 1.1.9), and `hex
0.4.3` for tests. All are MIT and/or Apache-2.0. Pure Rust: no OpenSSL, no system library.

**`DatagramTransport` (`transport.rs`).** The TCP `Transport` is untouched.

```rust
pub trait DatagramSocket: Send + Sync + 'static {   // connected to one peer
    fn local_addr(&self) -> io::Result<SocketAddr>;
    fn peer_addr(&self) -> io::Result<SocketAddr>;
    fn try_send(&self, datagram: &[u8]) -> io::Result<usize>;       // never blocks
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
}
pub trait DatagramTransport: Send + Sync + 'static {
    type Socket: DatagramSocket;
    fn bind(&self, endpoint: &Endpoint) -> impl Future<Output = io::Result<Self::Socket>> + Send;
}
pub struct DirectUdp;   // OS UDP sockets, first resolved address, fresh local port per bind
```

mosh code never creates a socket. A socket is *connected*: it sends only to, and receives only
from, the peer, which replaces mosh-rs's `from == server` check. `DirectUdp::bind` waits for
the socket to be writable so the first `try_send` is not refused. Roaming is a second `bind`:
the new socket carries everything sent from then on, up to ten older sockets keep being read
(a reply to a datagram sent from an old port returns to that port), and they are dropped once
the newest has *delivered an authenticated datagram* and has done so for 60 s (a newest socket
that has never received anything, because the new path is not routable yet, proves nothing and
the old ones stay). Sockets are polled newest first; an old socket that fails for anything but
a one-off ICMP error is dropped, and the newest socket's own error is reported only when no
other socket has data, so one failing socket never starves the live one. Opening a socket
resolves the host name and so can take as long as the resolver: the driver runs it beside
everything else (`Link::next_socket` is an owned future, `Link::adopt` takes the result), so
commands, input, frames, health callbacks and a disconnect are served while it runs. An attempt
is given up after 3 s; a failed or given-up rebind leaves the link as it was and is retried
after 1 s. **The remote address is pinned.** The server is one IP and UDP port: the IP the
SSH connection that ran the bootstrap actually reached, which the caller passes to `start`.
Roaming changes only the local socket (a fresh source port, or the new network on Android).
`Link::open` checks that the first socket reaches exactly that address and `Link::adopt`
refuses (leaving the link as it was) any socket whose peer IP or port differs: a name that
resolves to another address on a later rebind, a round-robin or changed DNS record, the other
address family. Sockets are opened from the IP literal, so no resolver is involved, and a
transport that connects elsewhere anyway is caught by the check (an IPv6 scope id or flow
label is not part of the identity; scoped link-local IPv6 is not supported for mosh). A
datagram refused as too large
(`EMSGSIZE`) drops the datagram size to 500 and stays there, as mosh does. The Android network
binding in M3 is another `DatagramTransport`.

**`Screen` over libghostty (`mosh/ghostty.rs`, `ssp/screen.rs`, `ssp/terminal.rs`).**
Terminal state stays in libghostty: `GhosttyScreen` wraps a `TerminalEngine`, so frames,
`encode_key` and `scroll` are exactly the SSH ones. Decisions:

- mosh keeps a copy of every state the server may still diff from, and a diff is applied to
  the state it names, not to the newest. libghostty terminals cannot be cloned, so `Screen` is
  not `Clone`: it has `snapshot()` and `restore()` (fallible), implemented with libghostty's
  terminal snapshot encoding (`TerminalEngine::{snapshot, from_snapshot, track_continuation,
  viewport_offset, set_viewport_offset}`, additive changes to `terminal.rs`). A snapshot of an
  80x24 screen is about 1.2 KB and takes 7 µs to encode, 100 µs for a full copy (release build
  on the dev machine). libghostty snapshots include the scrollback, but it caps that itself: a
  paced stream that never stops (an endless log tail) settles at about 1000 rows, a snapshot of
  about 70 KB and 0.2 ms to encode, measured by feeding 200,000 lines (a regression test
  asserts the snapshot stays under 1 MiB). That is the per-state cost at its worst, once per
  received state.
- **At most 32 older states are held** (`MAX_SAVED_STATES`). The receiver would let a server
  whose acknowledgements never arrive push this to 1024 snapshots. When full, the MIDDLE one is
  dropped, as the sender does with its own history: the server diffs from the oldest state it
  is still waiting on or from a recent one, never from one in between. A diff from a dropped
  state is refused (`apply_diff` returns `Ok(false)`).
- **A state is acknowledged only once its diff is applied.** `Session::handle_datagram` applies
  the diff first; if it is refused (`Ok(false)`) or cannot be decoded, the state is forgotten
  (`TransportReceiver::forget`) rather than held, and the client acknowledges the newest state
  it does hold, so the server's next diff starts from a state the client has. Acknowledging first would leave
  the screen stale with the server diffing from a state the client never built.
- **The reader keeps their place.** A snapshot does not record where the viewport is scrolled
  to, so when the live screen is replaced by one restored from an older base
  (`Screen::adopt_view_of`, implemented for libghostty with `viewport_offset` and
  `set_viewport_offset`) the new screen is scrolled to the same row offset from the top; a
  reader at the bottom stays at the bottom. Default colours and the reply callback are
  installed again by `from_snapshot`, and the first frame after a restore is a full one.
- `ClientTerminal` keeps the newest state as the one **live** screen and mutates it in place
  (so frames stay deltas and scrollback is kept); the usual diff starts from exactly that
  state, which is snapshotted first because the server may diff from it again if our
  acknowledgement is lost. Older states are held as snapshots and restored only when the
  server diffs from one (the live screen is then replaced and the next frame is full). A
  snapshot is skipped for states below the instruction's `throwaway_num`.
- Every `Bytes` event of a diff is fed in order (upstream fed only the last).
- **Geometry belongs to the renderer.** `Session::resize` queues the resize for the server and
  resizes the local screen at once, so frames keep the size Kotlin asked for (the Frames
  contract); the server's own resize reports are returned as events and not applied. A diff
  computed for the old shape that lands on the new one corrects itself with the server's next
  diff, which repaints every cell after a size change.
- Replies the local terminal would generate are discarded: the server's emulator already
  answered the program's queries and its diffs contain no queries.
- A copy that fails (out of memory) closes the session with `Failed { Internal }` rather than
  panicking.

**Sans-IO session (`ssp/session.rs`).** `Session` owns no sockets: `handle_datagram` takes one
received datagram (`Fault::Dropped` for a forged or corrupt one, which is ordinary;
`Fault::Screen` is fatal), `tick` returns the datagrams due and whether to rebind, and
`wait_time_ms` says when to call it again. `Tick::rebind` is true after ten seconds with no
completed round trip (mosh's rule) or when `request_rebind` was called.

**Fragment ids name one payload.** A state is sent again until acknowledged, and a large input
spans many datagrams, so the receiver may complete one instruction from fragments of several
transmissions of it; that only works if they are the same bytes. mosh gives every instruction
fresh random chaff, and recompressing a repeat would produce a different zlib stream under the
same fragment id, which the peer would combine into an undecodable instruction. `Fragmenter`
therefore keeps the compressed payload of the last instruction and resends exactly those bytes
(first transmission's chaff included) while the instruction is unchanged; a changed numeric
header field, diff or MTU allocates a new id and a new payload. A test splits complementary
fragments across transmissions with changing chaff.

**`mosh::bootstrap`.**

```rust
pub async fn bootstrap(host: &impl RemoteHost, caps: &HostCapabilities, size: TerminalSize,
                       target: &[String]) -> Result<MoshParams, BootstrapError>;
```

Runs `env 'LANG=<utf8>' '<mosh-server>' 'new' '-s' '-c' '256' '-l' 'LANG=<utf8>' ['--'
<target…>]` through `host.exec` (the locale through `RemoteCommand.env`). `mosh-server` and
the UTF-8 locale come from the capability probe; no `mosh_server` is `NotInstalled` and runs
nothing. `target` is the argv to run in the session, empty for the user's login shell; lane A1
builds the tmux or herdr argv. `size` is carried into `MoshParams`: `mosh-server` has no size
option (it starts at 80x24 and learns the real size from the client's first datagram, which the
driver sends). `MOSH CONNECT <port> <key>` is read from stdout and `[mosh-server detached, pid
= N]` from either stream. `MoshParams { port, key: MoshKey, size, server_pid: Option<u32> }`:
`MoshKey` is `Zeroizing`, validated as a canonical 128-bit key, has a redacted `Debug` and no
`Display`; stdout is never quoted in an error. The output (`ExecOutput`) wipes itself on drop,
including when the exec fails or is cancelled before `bootstrap` sees it (see *Remote commands*),
and the text copies made while parsing are `Zeroizing`. The vendored
`Base64Key` also zeroizes its bytes and printable forms; the AES key schedule inside `ocb3`
0.1 is not zeroized on drop (the crate has no such support).
`BootstrapError`: `NotInstalled`, `Remote(RemoteError)`, `Failed { status, detail }` (detail is
a 400-byte excerpt of stderr, for example the missing-UTF-8-locale message), `NoConnectLine`,
`InvalidPort`, `InvalidKey`; `into_failure()` maps them to `SessionFailure` (`NotInstalled`,
`TimedOut`, `ConnectionLost` for `Closed` and for `Io`, a broken transport under the exec
channel, otherwise `CommandFailed`).

```rust
pub async fn terminate(host: &impl RemoteHost, pid: u32) -> Result<(), RemoteError>;
```

`mosh-server` has no idle timeout and waits for its client for as long as it lives, so a start
that never connects (the UDP port is firewalled, the name is wrong, the user disconnects first)
would leave it and its shell on the host for good, once per retry. `terminate` sends `SIGTERM`
to `MoshParams::server_pid` through the exec channel, and only if the process there is named
`mosh-server` (a reused id is left alone; one already gone is not an error). **Whoever calls
`bootstrap` and `start` must call it when the session closes `Failed { TimedOut }` before ever
reaching `Connected`, or is disconnected before it did** (lane A1/M3 own that call site);
`bootstrap` itself calls it when the server started but its answer was unusable (it reads
the pid from either stream *before* wiping stdout, so a pid printed on stdout beside an unusable
CONNECT line is still cleaned up). A
`MOSH_SERVER_NETWORK_TMOUT` is deliberately not set: it would also end a healthy session whose
client was offline for a while. An exec that fails after the server started (a timeout) loses
the pid and cannot be cleaned up.

`-s` makes `mosh-server` bind to the server address in the exec channel's `SSH_CONNECTION`
(wildcard with a warning when it is absent). The UDP address given to `start` must therefore be
the address the SSH connection reached: with address racing, the winning address, not another
name or record for the host. The host connection carries it: `Transport::peer_addr(&stream)`
(`DirectTcp`: the TCP peer) fills `Raced::peer`, the host driver records it before `Connected`,
and `HostHandle::peer_addr() -> Option<SocketAddr>` exposes it (its IP is what `start` takes;
the port is SSH's). `None` for a transport that cannot say; M3 must then not start mosh, or
resolve the name once itself, and pin that.

**`mosh::start`.**

```rust
pub fn start(params: MoshParams, peer: IpAddr, observer: Arc<dyn SessionObserver>)
    -> Result<SessionHandle, EndpointError>;
pub fn start_with<T: DatagramTransport>(transport: T, params: MoshParams, peer: IpAddr,
    observer: Arc<dyn SessionObserver>, health: Option<Arc<dyn HealthObserver>>)
    -> Result<(SessionHandle, LinkControl), EndpointError>;
```

`peer` (the IP the SSH connection reached, see above) plus `params.port` is the pinned
address; a zero port fails synchronously. The
dedicated `or2-mosh` thread owns the `TerminalEngine` and drives the standard lifecycle, with
the same command, frame and publish rules as the SSH driver:

- `Connecting` until the first datagram from the server authenticates (there is no handshake:
  the SSH bootstrap agreed the key), then `Connected` and a full frame. Opening the first socket
  runs beside the command channel: a `Disconnect` (or a dropped handle) during it closes the
  session `Disconnected` at once, and a `Resize` is remembered for the first datagram. No authenticated
  datagram within `CONNECT_TIMEOUT` (15 s) is `Closed { Failed { TimedOut } }`; a socket that
  cannot be opened is `Failed { Unreachable }`. A wrong key never authenticates, so it also
  ends as `TimedOut`.
- After that the session never gives up on its own: a dead network is mosh's point. It closes
  with `RemoteExited { exit_status: None }` when the server announces the end of the session
  (mosh does not carry the exit status), `Disconnected` after `Disconnect` or when every
  handle is dropped, or `Failed { Internal }` if something breaks.
- Disconnect runs mosh's shutdown handshake so the server's session ends instead of lingering
  on the host, waiting at most one second for the acknowledgement.
- `Text` is written with `text_bytes`; `Key` goes through `TerminalEngine::encode_key` with the
  modes the server's diffs have set; `Scroll` and `FullFrame` as for SSH. Host-key commands
  are ignored: mosh has no host key.
- `LinkControl::roam()` is the hook for Android's network callback: open a new socket now
  instead of after ten silent seconds. `HealthObserver::link_health(LinkHealth { since_heard_ms,
  since_ack_ms })` is called about once a second from the driver thread: the hook for M3's
  network-health reporting.

**Findings.** mosh keeps no scrollback; see the M0 consequences in `design.md`. The live test
needed `SSH_CONNECTION` set (this machine's own login session made `-s` bind a public address).

**Tests.** Unit tests: vendored crypto/packet/sender/statesync/transport/key tests (the
prediction and vt100 tests are not carried), `ClientTerminal` state logic with a text screen,
`GhosttyScreen` snapshots (mid-escape-sequence and scrollback included), the bootstrap command
line and parsing with a fake `RemoteHost`, `DirectUdp`, the socket set, and the driver against
a fake server that speaks the server half of the protocol (lifecycle, input, resize, roam,
remote end, disconnect, timeout, wrong key, forged datagrams, a disconnect or resize while the
first socket opens, a rebind stuck on a resolver), the sans-IO session against a forged
server (an unapplicable or garbled diff is not acknowledged), the socket set with scripted
sockets (failing, starved, other address family, other IP of the same family, other port,
rotating DNS answers, a first socket that connects elsewhere), the driver refusing a roam to
another address, and `Debug` redaction of every protocol type
that holds keystrokes or host output. `tests/mosh_live.rs` (feature
`test-support`) bootstraps a real local `mosh-server` over `LocalHost` with `SSH_CONNECTION`
set to a loopback connection, connects over 127.0.0.1, runs a command, resizes and checks
`stty size`, roams (the server's replies move to the new socket and stop on the old one) and
disconnects (the server exits). A second test checks `mosh::terminate` (it stops a server, and
leaves a pid that is not a `mosh-server` alone). A guard kills exactly the process ids it
identified (the server and its shell, each re-checked against `/proc/<pid>/comm`) even on
panic; it is armed as soon as the bootstrap returns, from the printed pid or else from the
process holding the reported UDP port, and the test fails loudly (with the guard still armed)
if the server cannot be identified or printed no pid. The roam check lets in-flight datagrams
drain before it counts what the old socket receives. It needs `mosh-server`, `/bin/bash` and
the `kill` binary, skips with a message without `mosh-server`, and `OR2_REQUIRE_MOSH` makes the
skip a failure: set it in CI so the roaming and resize interop claim is never verified
vacuously.

## FFI API 6 (`or2-ffi`)

API 5 was API 4 without the M1 `connect` export; **API 6 adds `HostConnection.focus_herdr_pane`
and `HostError.PaneNotFound`**; **API 7 adds `Session.submit_text(text) -> Result<(), SessionError>`**
(see [Input and resize](#input-and-resize)). The rest of the surface below is unchanged.

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
    /// API 6. Focuses `pane_id` in herdr `session` (`None` = the default session); resolves
    /// once herdr acknowledged. See "Agent terminals and focus" below.
    async fn focus_herdr_pane(&self, session: Option<String>, pane_id: String)
        -> Result<(), HostError>;
}
// HostError: NotConnected, Closed, NoHostKeyPrompt, HostKeyMismatch, EmptyDimension,
//            InvalidName, NotInstalled { program }, PaneNotFound (API 6), CommandFailed { reason }
// HostCapabilities { tmux: Option<String>, herdr: Option<String>, mosh_server: Option<String>,
//                    utf8_locale: String, herdr_sessions: Vec<HerdrSessionInfo { name, running, is_default }> }

#[uniffi::export(callback_interface)] pub trait HerdrListener: Send + Sync {
    fn on_herdr_state_changed(&self, state: HerdrState) -> Result<(), ListenerError>;
}
#[derive(uniffi::Object)] pub struct HerdrWatch;   // fn state(&self) -> HerdrState; fn stop(&self);
```

- **Agent terminals and focus (API 6).** herdr's focus is shared state of the session: a
  terminal running `herdr` (`TerminalTarget.Herdr`) shows whichever pane is focused *now*, other
  herdr clients can move the focus, and the target's `pane_id` only focuses the pane once, at
  open (A to B to A in one session leaves a reused "A" terminal showing B, and a reply typed for
  A would reach B). **The app must therefore call `focus_herdr_pane(session, pane_id)` and
  await its success every time an agent-target terminal is activated or reused**: tapping an
  inbox row whose terminal is already open, choosing it in the switcher, or navigating back to
  it. Only then show the terminal and enable input for it. For a first open the target's
  `pane_id` already focuses before `herdr` starts (`open_terminal` unchanged).
  `focus_herdr_pane` validates names like `TerminalTarget` (`InvalidName`; session
  `[A-Za-z0-9_-]{1,64}`, pane `[A-Za-z0-9:_-]{1,128}`), needs a connected host
  (`NotConnected`/`Closed`), uses the probed herdr path (`NotInstalled { "herdr" }` without
  one), and sends one `pane.focus` on a short-lived socket stream. A pane herdr no longer has
  (`pane_not_found`) is the explicit **`PaneNotFound`**: the agent is gone, so refresh the inbox
  and do not show the terminal for it. A session that is not running, a missing socket or any
  other herdr error is `CommandFailed { reason }` (a diagnostic). Cancelling the coroutine only
  drops the reply; a focus already sent still happens. `contract_probe_host` answers
  deterministically: the probe view's panes `w1:p1`, `w1:p2` and `w2:p1` succeed (and become the
  focused pane of watches started afterwards), any other id is `PaneNotFound`.
  The app side is `TerminalActivations` (`app/`, reached as `HostConnections.activations`): the
  inbox row (a first open too, so a vanished pane never opens a terminal), the session switcher, a
  Home thumbnail and the host screen's recent list all go through it. It awaits the focus, then
  yields `Activation.Ready(terminal)` or `Activation.Failed(message)`; `PaneNotFound` says the
  agent's pane is gone, any other error is shown, and neither navigates. Terminals that are not for
  one pane (shell, tmux, a herdr session picked whole, a closed terminal) need no focus. While it
  waits, `pending` drives a non-modal progress card; leaving the screen cancels the wait (a focus
  already sent still happens).
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

Lane B's app connects through `connect_host` and `HostConnection.open_terminal`; no Kotlin
calls the removed M1 `connect` export. The loopback-sshd JVM tests (`HostConnectNativeTest`,
`HostConnectionsNativeTest`) run against the real host driver.

- **Room v2** with a real `Migration(1, 2)` (never destructive: Keystore-bound keys cannot be
  recreated). New `host_addresses(hostId -> hosts.id ON DELETE CASCADE, position, hostname,
  port, PRIMARY KEY(hostId, position))`; the migration moves each host's `hostname`/`port` to
  position 0 and drops those columns. `hosts` gains `showInInbox` (default true) and is altered in
  place (`ADD COLUMN`, `DROP COLUMN`), never dropped: with foreign keys on, dropping `hosts`
  would cascade away `trusted_host_keys`. Trust therefore survives the upgrade (the destination
  is unchanged). The schema is exported (`app/schemas/`, KSP arg `room.schemaLocation`; version
  1 is the shipped M1 shape). Tests: `MigrationSqlTest` (JVM, real SQLite) and
  `MigrationDeviceTest` (`MigrationTestHelper`, and the platform SQLite version).
- **Trust is keyed to the host's ordered address list.** Any change to it clears trust: a
  hostname, a port, an added, removed or reordered entry (a reorder changes nothing about what
  is trusted, but "any change" is the rule and the cost is one re-prompt). Label, username, key
  and inbox flag do not. `TrustStore.replaceTrust` takes the `Host` the prompt was raised for and
  fails if its address list changed meanwhile. A live connection ends when the destination,
  username or key changes (`connectionAffectedBy`), not for a label or inbox edit.
- **Holder:** an application-scoped `HostConnections` (main-dispatcher-confined) keeps at most
  one connection per host and any number of terminals per connection. A host that is not closed
  (or being disconnected) is never connected a second time; a closed one is replaced by a fresh
  connection when the user connects again. `HostPort` is the app's view of `HostConnection`
  (the generated class returns the concrete `Session` and `HerdrWatch`, which tests cannot
  fake); production wraps the native object, tests supply fakes.
- **Unlocking:** one biometric prompt per distinct key record (`planUnlock`, `connectGrouped`).
  Every requested host that shares the key connects from that one decryption: the array is
  wiped (in a `finally`) after the last `connect_host` call returns or throws, never between
  calls, because the request keeps a reference to it. A host whose connect fails does not stop
  the others (the first error is rethrown afterwards); a failed or cancelled unlock ends the
  batch; hosts without a key are reported at the end.
- **Host-key prompts live on the host**: persist trust before approving, bound to the presented
  fingerprint, with M1's expiry checks. The host screen shows the dialog; a prompt for a host
  whose screen is not showing appears as a dialog naming the host, and never over the shown
  host's own dialog (one dialog at a time).
- **Edits are applied, then acted on:** an edit that changes a live connection's destination,
  login or key, and a host deletion, end the connection (and, for a deletion, its terminals)
  only after the write succeeded; a failed write reports the storage error and leaves the
  connection alone.
- **Terminals** keep M1's per-session lifecycle: the final frame stays readable through
  `Closed`; a display lease delays native `close()` until the screen leaves composition.
  Closing a host closes its terminals through their own callbacks; they stay listed (with their
  final frame) until the user closes them, and forgetting a connection never touches them.
- **Capabilities and watches:** when a connection reaches `Connected` the holder calls
  `capabilities()` and, when herdr is installed (none otherwise), starts one `watch_herdr` for the
  default session (`None`, never its listed name, and whether or not it is listed or running) and
  one for every other listed session, **running or not**. The connection caches its capability
  probe, so the session list is the connect-time snapshot and "Refresh" cannot discover sessions
  started later; a watch on a stopped or missing session reports `Unavailable { NotRunning }` and
  retries every 10 s (A2), so those rows recover by themselves. A session started after connect
  under a name that was not listed is therefore only picked up by reconnecting. "Refresh"
  re-queries and reconciles the watches with the answer (adds new ones, stops those no longer
  listed). A watch the host refuses is shown as `Unavailable`. A watch's `stop()` or `close()`
  failing never aborts a sync or a teardown. When the host closes, its watch handles are released
  at once (the watch records keep their final state); the `HostConnection` object itself stays
  readable until the host is replaced or dismissed. Watches run only for hosts flagged
  `showInInbox` (a hidden host still gets its capability probe for the host screen): editing the
  flag on a live connection starts or stops them (`setWatching`), without ending the connection.
- **Screens:** Inbox (start destination): agents across all `showInInbox` hosts grouped blocked,
  working, done, idle (then unknown), each row with host, agent display name, workspace and
  tab, status chip and cwd; within a status by host, session, workspace, tab, pane, so a refresh
  never reshuffles rows. Per-host status with unlock/retry/open actions and "Connect all" (one
  prompt per key). Tapping a row opens `Herdr { session, pane_id }`, or returns to the terminal
  already open for exactly that host and target (one not closed or being closed). Host screen: connection
  state and address used, host-key prompt, Shell, tmux sessions (attach, create by name with the
  Rust name rules checked first, refresh) and herdr sessions. Terminal: the M1 terminal screen
  per session, a switcher among open sessions, and Back to the previous screen without
  disconnecting; open terminals are also listed on the inbox and host screens. Hosts and Keys
  remain tabs. Navigation is a small saved back stack rooted at a tab.
- **Host form:** ordered address list (add, remove, reorder, a port per address, 1 to 8), username,
  key, and the inbox switch. Hostnames and the username are trimmed before saving.
- The loopback-sshd fixture (`OpenSshFixture.kt`) gives its sshd sessions a private
  `TMUX_TMPDIR`, a temporary `$HOME` and a fake `herdr` first on `PATH`, so neither the user's
  tmux server nor a real herdr session can be reached by the holder's automatic probe and
  watches. Those tests skip without `/usr/bin/sshd` unless `OR2_REQUIRE_SSHD` is set.
- `MigrationDeviceTest.theDevicesSqliteSupportsDropColumn` checks on the phone that its SQLite
  is at least 3.35 (the version `DROP COLUMN` needs); no JVM test can.

# M3: stays connected

M3 makes sessions survive the phone's life: a foreground service owns every connection, mosh
carries terminals across network changes, and the app returns to the same pane. FFI API
becomes **8**. Everything in M1/M2 still applies unless changed here.

## Lanes

| Lane | Owns | Depends on |
|---|---|---|
| M3-A: Rust and FFI | mosh terminals on a host connection, transport choice, link health, roaming, `network_changed`, host-close semantics for mosh | this section |
| M3-B: Android | foreground service owning `HostConnections`, network callback, per-host transport preference (Room v3), Auto fallback, reattach, battery-optimisation prompt, notification | M3-A's API 8 surface (via `contract_probe_host`) |
| UI-C: compact UI | smaller default type scale, controls and popups everywhere ([ui](ui.md)) | none |

M3-A lands its FFI surface and probe support first (a small commit), so M3-B builds against
real generated bindings; the implementation follows on the same branch.

## Mosh terminals (`or2_core`, FFI)

```rust
#[derive(uniffi::Enum)] pub enum TerminalTransport { Ssh, Mosh }

impl HostConnection {
    fn open_terminal(&self, target: TerminalTarget, transport: TerminalTransport,
                     columns: u16, rows: u16, listener: Box<dyn SessionListener>)
        -> Result<Arc<Session>, HostError>;
}
impl Session {
    fn transport(&self) -> TerminalTransport;
    /// mosh: open a new UDP socket now (the network changed). SSH: no-op.
    fn roam(&self);
}
#[uniffi::export] pub fn network_changed();   // every live mosh session roams; every host
                                              // connection sends an SSH keepalive at once
pub trait SessionListener {                   // added method
    /// mosh only, at most once a second and only when a value changes.
    fn on_link_health(&self, health: LinkHealth) -> Result<(), ListenerError>;
}
#[derive(uniffi::Record)] pub struct LinkHealth { since_heard_ms: u64, since_ack_ms: u64 }
```

- **Bootstrap** runs over the host's SSH connection with the probed `mosh-server` and UTF-8
  locale (`mosh::bootstrap`), then `mosh::start_with` pinned to the host's
  `peer_addr()` (the IP the SSH connection actually reached, never re-resolved). The target's
  command (tmux attach, herdr) is passed as mosh-server's command. A missing `mosh-server`
  closes the session `Failed { NotInstalled { program: "mosh-server" } }`; no authenticated
  datagram within the connect timeout closes it `Failed { TimedOut }` (UDP blocked) after
  `mosh::terminate` cleans up the server. Any failure before `Connected` terminates the server.
- **Host close does not close mosh sessions.** A mosh session needs the SSH connection only to
  start. If the host connection is lost, its mosh sessions keep running; if the user
  disconnects the host, its mosh sessions close too (`Disconnected`, shutdown handshake so the
  server exits). SSH-transport terminals keep M2's rule.
- **Roaming:** Kotlin calls `network_changed()` from its connectivity callback (default network
  changed or lost-then-available). New sockets follow the process's current default network.
- `contract_probe_host` supports `Mosh` deterministically (connects, echoes like SSH, reports a
  fixed health sequence, `roam()` is counted and shown in the echo row). Exactly:
  - `capabilities().mosh_server` is `Some("/usr/bin/mosh-server")` (it was `None` before API 8),
    so AUTO picks mosh against the probe.
  - After its first frame a Mosh probe terminal delivers three `on_link_health` calls, in order,
    `(since_heard_ms, since_ack_ms)` = `(300, 300)`, `(6000, 9000)`, `(400, 400)`: healthy,
    stale (past the 5 s grey-out), recovered. SSH probe terminals never call it.
  - Each `roam()` (or `network_changed()` while the session is open) adds one to a counter shown
    in row 2, the echo row: `roams N` when nothing was typed, else the latest text echo then
    `| roams N` (`text 78 | roams 2`). SSH probe terminals ignore `roam()`.
  - `transport()` returns what `open_terminal` was given.

### M3-A FFI surface status (API 8, first commit on `m3/a`)

The FFI surface above is real and final; the mosh implementation behind it follows on the same
branch. Until then, deviations to know about:

- `HostConnection.open_terminal(.., Mosh, ..)` on a real host validates like SSH, then closes
  the session `Failed { Internal { message: "mosh terminals land with M3-A" } }` from a Rust
  thread. This is an honest failure, not a stub that succeeds; M3-B must not rely on it (AUTO
  falls back only on `TimedOut` or `NotInstalled`).
- Core: `HostHandle::open_terminal_with(target, transport, size, observer)` carries the choice
  (`HostCommand::OpenTerminal.transport`); `open_terminal` is the SSH shorthand.
  `SessionHandle::roam()` sends `Command::Roam`, which the SSH pump ignores and the mosh driver
  turns into a socket rotation (`request_rebind`). `SessionObserver::link_health` defaults to a
  no-op.
- `network_changed()` calls `roam()` on every live mosh session (the FFI keeps weak references
  to them) and sends an SSH keepalive (`keepalive@openssh.com`, reply requested) on every
  established host connection (russh `Handle::send_keepalive`). The keepalive makes a connection
  that the network change silently broke fail within the existing keepalive/TCP timeouts of the
  write instead of waiting for the next 15 s tick; it does not by itself close a connection,
  and a healthy one just gets a reply that is ignored.

## Android

- **Foreground service** (type `specialUse`, subtype documented in the manifest) owns the
  application's `HostConnections` while any host or session is open; it stops itself when the
  last one closes. Its ongoing notification shows hosts and sessions and offers "Disconnect
  all". Request `POST_NOTIFICATIONS` (Android 13+) the first time a connection starts; the
  service still runs if it is denied.
- **Battery optimisation:** a one-time explanation and `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`
  the first time a session is open while the app goes to the background; never nag again.
- **Network callback:** `ConnectivityManager.registerDefaultNetworkCallback` → `network_changed()`
  on every default-network change; debounce 500 ms.
- **Transport preference** per host in Room v3 (`transport`: `AUTO` default, `SSH`, `MOSH`), real
  `Migration(2, 3)` with a migration test. AUTO uses mosh when `capabilities().mosh_server` is
  present and falls back to SSH (remembered for that connection, explained in muted text) when
  mosh fails with `TimedOut` or `NotInstalled`. The terminal header's transport badge shows the
  actual transport; link health greys the badge and shows "Last heard 12 s ago" when
  `since_heard_ms > 5000`.
- **Keys stay per-use.** No private key is retained to reconnect in the background. When the app
  returns to the foreground and an inbox host's SSH connection was lost, the app offers one
  grouped unlock (one biometric per distinct key) to reconnect; mosh terminals need no unlock.
- **Reattach:** the app remembers the last focused terminal (host id, target, transport) in
  app-private preferences. On return, if that session is alive it is shown directly
  (`request_full_frame`); if only its host is connected, the same target is reopened (herdr pane
  focused first); otherwise Home shows a "Resume" card that does both after unlocking.
