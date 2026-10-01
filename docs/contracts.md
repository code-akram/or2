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

Non-interactive SSH does not load the user's `PATH`. Two execs per connection **run at once**
(one round trip pair, not two), cached in memory for the connection's lifetime (Rust has no
storage); the first finds `tmux`, `herdr` and `mosh-server`:
`command -v`, then `$HOME/.local/bin`, `$HOME/.cargo/bin`, `/opt/homebrew/bin`,
`/usr/local/bin`, `/usr/bin`, `/bin`, `$HOME/.nix-profile/bin`, `/run/current-system/sw/bin`.
It also reports a UTF-8 locale (`C.UTF-8`, else the first `*.UTF-8`/`*.utf8` in `locale -a`,
else `en_US.UTF-8`). Every later tmux/herdr/mosh command uses the absolute path found: the
host driver passes `HostCapabilities.herdr` to `herdr::run_in` and `herdr::focus_pane_in` (as
`mosh::bootstrap` takes `caps`), so no client repeats the PATH search.
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

Lane A1 decisions, both from review: (1) herdr's session list is **not** part of that script.
`probe::HERDR_SCRIPT` is a second fixed script, run in its own channel **at the same time** (the
probe finished in two round trips instead of four): it finds herdr with the same search, prints
`or2:herdr:<path>`, and when found `or2:list-begin`, `herdr session list --json`'s stdout and
`or2:list-end:<exit status>`. The text between the markers is read by `herdr::parse_listing`
(lane A2's one parser of `<herdr> session list --json`, shared with `herdr::list_sessions`; the
probe has no parser of its own) and the whole exec is bounded by `probe::HERDR_LIST_TIMEOUT`
(5 s), so a wedged herdr costs only
`herdr_sessions` (empty), never tmux, mosh-server or the locale, and never makes the probe
fail and be retried at full cost on every terminal open. Only a connection that closes
mid-probe fails it. `name`, `running` and `default` are carried over; an unreadable listing
(including one with an entry that has no `socket_path`) is a failed listing, not a partial
one (the first script still reports herdr's path when the second hangs). (2) The cache holds
programs and locale; **`capabilities()` reads herdr's session list afresh on every call**
(`probe::SessionsCache`), so `running` and sessions started or stopped after connecting show on the
host screen, **except the first call after the probe**: the probe's listing is as fresh as a read
made now, so that call reports it without a third exec (`Directory::seed` / `take_unread`). A listing that fails (herdr
gone, hung, garbage) reports the **last list that was read successfully**, not the one from
connect time (until a read succeeds, the probe's own list): the app treats the list as
authoritative and stops the watch of a session missing from it, so a transient failure must
neither drop a session found since connecting nor bring back one that has gone. Reads can
overlap (several Refresh calls); each takes a ticket when it starts and a result is applied
only if no read that started later has already been applied, so a slow older read never
overwrites a newer list (and then reports the newer one). The programs and locale stay in the
immutable probe result. Live per-pane state is
still `watch_herdr`'s job; terminal opens and watches use only the cached paths. The listing the
probe read (with each session's socket path, which never reaches Kotlin) also seeds the
connection's `herdr::Directory` (next section), which is where the herdr watches and pane focuses
find sockets.

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
  started or immediately when it fails (only the most recently started address's failure brings
  the next forward). The first TCP connection wins; the others are dropped. **Each address has its
  own allowance** of `transport::ADDRESS_TIMEOUT` (6 s, `HostOptions::address_timeout`), name
  resolution included, inside the overall 20 s connect timeout: an address that silently drops
  packets (an overlay IP with no route while its VPN is off, a sleeping machine) fails at its own
  limit (`no answer within 6 s`) instead of holding the race until the overall timer, and the
  attempts' limits run side by side, so with up to 8 addresses the race ends within about 8 s. If
  all fail, close with `Unreachable` whose message lists **each address's outcome by position, in
  words and with no host names or addresses**: `TCP connection failed: address 0: name not
  resolved (mDNS) after 3 tries; address 1: no answer within 6 s` (`transport::describe_error`:
  `connection refused`, `no route to the host` for ENETUNREACH/EHOSTUNREACH, which fail at once and
  start the next address, `no answer`, or the text of an error `transport` raised, including a
  hostname's several resolved addresses summed up as `2 addresses: connection refused, no answer
  within 5 s`: that text survives into the host's message, not only the kind of its first failure). The app puts
  the host names back for its own screen. If the **overall connect timer fires while the race is
  still running** (an allowance longer than the timeout, a test), the close is also `Unreachable`,
  `no address answered within 20 s: address 0: still trying after 20 s; ...`
  (`transport::RaceReport`, which the race updates and the driver reads); `TimedOut` is only for a
  TCP connection that then stalls in the SSH handshake or authentication.
  The SSH handshake runs only on the winner. `Connected.address_index` reports which one won.
- **One endpoint** (`DirectTcp::connect`, `transport::dial`): the name is **resolved once** and the
  resolved addresses are raced, they are not tried in turn. A `.local` name (mDNS; the first lookup
  an app makes can fail after a second or two and succeed from the cache on the next) is resolved
  up to 3 times within 4 s, 250 ms apart (`name not resolved (mDNS) after 3 tries`, or `within
  4 s`); any other name once (`name not resolved`). **IPv6 link-local results without a scope id
  are dropped** (an app socket cannot connect to them; the same name usually has an IPv4 address);
  if nothing else remains, the endpoint fails with that explanation. The usable addresses race
  Happy-Eyeballs style: families alternate starting with the resolver's first, 250 ms apart, a
  refusal starts the next at once, and all of them stop at the endpoint's budget (5 s from the
  start, inside the race's 6 s allowance, so the endpoint explains itself first). When every
  resolved address fails the error is one line (`3 addresses: connection refused, no answer within
  5 s`). `Resolver` and `Connector` are traits, so all of it is tested against a scripted resolver
  on a paused clock (`transport_dial_tests.rs`, `transport_race_tests.rs`).
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
the probe (cached), then the channel (`pty-req` at the requested size, `shell` or `exec`, then a
window change if the size changed meanwhile), then `Connected`; a herdr pane's focus runs **beside
the channel open** (they do not depend on each other: the herdr client follows herdr's focus, so a
focus that lands a moment after the client starts only changes what it shows next), and the session
is `Connected` when both are done. A focus that fails closes the channel that was opened and the
session from `Connecting` with `CommandFailed` (`terminal_session::plan` returns the command and the
pending `PaneFocus` separately). Anything that fails before the channel exists (missing program,
failed probe) closes the session from `Connecting` without opening one. Every other way a
session ends (user disconnect, host close, protocol failure) closes its channel first, so the
program does not outlive the session on the host: a tmux client detaches, the tmux session
lives on. The program's own exit is `RemoteExited { exit_status }`. Channel setup (open,
`pty-req`, `shell`/`exec`, each awaiting the server's reply) is bounded by the host's exec
timeout (10 s): a server that never answers closes the session with `TimedOut`, after
closing the channel. A refused channel open (`MaxSessions`, see above) is `ShellRejected`,
never `ConnectionLost`.
A user disconnect that arrives while the focus is still pending closes a channel that was accepted
meanwhile (the open's result is kept outside the join of the two, which a raw russh channel would
otherwise leave unclosed: it holds a `MaxSessions` slot), exactly once; test
`a_terminal_given_up_while_its_focus_waits_closes_the_channel_opened_beside_it`.

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
  not reported to Kotlin. **The connection's `herdr::Directory`** (one per host connection, inside
  `probe::SessionsCache`) holds the last listing that was read successfully, seeded by the
  capability probe: a watch's first attempt and every pane focus take a **running** session's
  socket from it and run no `session list`. A session the list does not know or calls stopped is
  not answered from it (it may have started since): that lookup reads the listing. A socket from
  the directory that does not open (`Unreachable`, the path went stale) sends that attempt back to
  the listing **once** (`Directory::invalidate`, then a fresh read); a socket from a listing read
  just now that does not open is the answer (`Failed`, with the forwarding-policy hint), not a
  reason to list again. Every later watch attempt (a retry after `Unavailable`, a recovery after
  `events_lost` or a dropped stream) reads the listing again: failure is the only trigger for
  re-discovery. The free functions `herdr::run`, `herdr::watch` and `herdr::focus_pane` (no
  connection) use a directory of their own and so still read the listing first.
- **Watch:** `herdr::watch(host, herdr, session, observer) -> HerdrWatchHandle` is `channel`
  plus a task running `run`. One *attempt*: find the socket (the connection's directory, see
  Discovery); open two streamlocal streams at once, a long-lived one for events and a request
  stream for the first snapshot; `events.subscribe` on the first (bounded by the 10 s request
  timeout), wait for the ack;
  then reconcile: `session.snapshot` on the request stream, install it, and read
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
    the contract sequence, and **nothing else delays it**: the socket comes from the directory (no
    listing), the subscription's stream and the snapshot's stream are **opened together** (the
    snapshot request itself still waits for the subscription's acknowledgement, so no event can be
    missed), and the 100 ms between reads and the per-pane resubscribe with its confirming read
    all follow the first delivery (a test asserts the first `Live` is delivered at the same
    instant the bootstrap starts, with only those two requests served). Every later new pane costs
    one more of each (a subscription cannot grow). Measured over a 120 ms round trip against a
    real sshd: a watch is live 3 round trips after the capability probe (see the latency
    fixture under "M3 polish").
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
- **Focus:** `herdr::focus_pane_in(host, herdr, directory, session, pane_id)` (and the
  connection-less `herdr::focus_pane`, which uses a directory of its own) takes the socket from the
  directory and sends one `pane.focus` request on a short-lived stream (10 s bound): two round
  trips (open, request) and no listing. **`herdr::FocusGate`** (one per connection) keeps the app's
  focus and the terminal's own from both reaching herdr: a focus for a pane whose focus is already
  in flight **joins it** and shares its answer (a failure included, and a cancelled leader leaves
  its followers to focus for themselves), and a terminal's focus (`from_terminal`) is satisfied by
  an acknowledgement younger than `focus::RECENT` (2 s) for the same pane **that is still the
  session's latest focus**: a herdr session has one focused pane, so the start of a focus of
  another pane (each focus takes a per-session generation) invalidates every older acknowledgement
  of that session, and an acknowledgement is remembered only if no later focus of the session
  started meanwhile (overlapping A and B leave only B recent). Focus A, focus B, open a terminal on
  A therefore sends a focus of A. The app's own request
  (`HostHandle::focus_herdr_pane`) is never answered from memory, because the user's desktop may
  have moved the focus meanwhile. It changes what the user's
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
label is not part of the identity). `start` takes an `IpAddr`, which has no scope, so a
scoped link-local IPv6 host cannot be named to it; the host driver passes the SSH peer's whole
`SocketAddr` (`run_session`'s `Plan::peer`), and `Link` opens its sockets from the literal with
the scope id (`fe80::1%3`, read numerically by the resolver), so such a host works over a
host connection. A
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
reaching `Connected`, or is disconnected before it did, or ends `Failed` after it did** (the host
driver does: M3, "Mosh terminals");
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
  since_ack_ms })` is called once a second from the driver thread, every sample and before
  `Connected` as well (a caller can show "waiting"): the hook for M3's network-health
  reporting. (The session observer's own link health, M3, is throttled; see "Link health"
  under "M3-A implementation".)

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
  `pane_id` focuses the pane as part of `open_terminal` (beside the start of the program, see
  "Terminal sessions on a host"); the app's own focus of the same pane and the terminal's are
  **one request** (the connection's `FocusGate` joins a focus in flight, and a terminal's own focus
  accepts one the app finished within 2 s), so the app may start the focus and the open together.
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
  inbox row (a first open too: the terminal is started beside the focus and dismissed when the pane
  turns out to be gone, so a vanished pane leaves no terminal), the session switcher, a
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
becomes **8** (M3-A), **9** with the follow-up's deadline and **10** with orphan cleanup, both below.
Everything in M1/M2 still applies unless changed here. All of it is integrated on one branch; the
sections below describe that integrated state.

## Lanes

| Lane | Owns | Depends on |
|---|---|---|
| M3-A: Rust and FFI | mosh terminals on a host connection, transport choice, link health, roaming, `network_changed`, host-close semantics for mosh | this section |
| M3-B: Android | foreground service owning `HostConnections`, network callback, per-host transport preference (Room v3), Auto fallback, reattach, battery-optimisation prompt, notification | M3-A's API 8 surface (via `contract_probe_host`) |
| UI-C: compact UI | smaller default type scale, controls and popups everywhere ([ui](ui.md)) | none |

UI-C is a pure token-level change (no FFI, schema or behaviour change): `Or2Dimens`, `Or2Shapes`
and `Or2Type` in `ui/Theme.kt` carry the compact scale, which is the only scale and the default
(documented in `docs/ui.md`, "Compact scale"). Decision: visible sizes shrink, but hit areas stay
usable because the platform grows every clickable's touch target to 48 dp; primary controls are
drawn at least 40 dp tall. The terminal's 12 dp default cell size is unchanged.

Review follow-up (UI-C, deviations from the first compact pass, all recorded in `docs/ui.md`): the
arrow pad's keys and grip sit on an opaque `crust` backing; the terminal header discs are 18 dp (12 dp
glyph) in 48 x 36 dp boxes so neighbouring touch targets do not overlap, and the `SSH` pill uses full
`text` at 11 sp; dialogs are an `Or2Dialog` card with the 12 dp gutter (not Material's `AlertDialog`
padding); kickers are 11 sp in full `accent`; the connecting spinner is the server icon's 20 dp; the
session-picker minimum height is 40 % of the screen and herdr rows are title-only with the state at the
right; `Retry` / `Unlock` are chip-scale; the thumbnails are 38 % of the width; the composer is one row
(text, close, send), and its paste and panes actions, which duplicated the toolbar's, are removed
(`Composer` loses its `paste` and `panes` parameters); disabled primary labels and field placeholders
get their own tokens (`OnAccentDisabled`, `Placeholder`; `ThemeTest` checks the contrast); the toggle
knob is `text` when on. Not taken as proposed: "disabled content at 0.5 of Text", which computes to 3.2:1
on `accentMuted` (worse than the 3.9:1 it replaced), so 80 % is used (5.5:1).

M3-A landed its FFI surface and probe support first (a small commit), so M3-B could build against
real generated bindings; the implementation followed on the same branch.

## Mosh terminals (`or2_core`, FFI)

```rust
#[derive(uniffi::Enum)] pub enum TerminalTransport { Ssh, Mosh }

impl HostConnection {
    fn open_terminal(&self, target: TerminalTarget, transport: TerminalTransport,
                     columns: u16, rows: u16, mosh_budget_ms: Option<u32>,   // API 9, see the follow-up
                     listener: Box<dyn SessionListener>)
        -> Result<Arc<Session>, HostError>;
    /// API 10, async: stop a mosh-server an earlier process left running (see "Orphan cleanup").
    async fn stop_mosh_server(&self, pid: u32) -> Result<(), HostError>;
}
impl Session {
    fn transport(&self) -> TerminalTransport;
    /// API 10: a mosh session's mosh-server pid on the host (set before `Connected`, kept after
    /// `Closed`); None for SSH, and when the bootstrap did not report it. Not a secret.
    fn server_pid(&self) -> Option<u32>;
    /// mosh: open a new UDP socket now (the network changed). SSH: no-op.
    fn roam(&self);
}
#[uniffi::export] pub fn network_changed();   // every live mosh session roams; every host
                                              // connection sends an SSH keepalive at once
pub trait SessionListener {                   // added method
    /// mosh only, at most once a second, and only when what the UI shows changes: the first
    /// sample, the link turning stale (`since_heard_ms > 5000`), each further whole second of
    /// silence while stale, and recovery. See "Link health" under "M3-A implementation".
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

### M3-A implementation (API 8)

The FFI surface is final and the mosh implementation is behind it. `HostConnection.open_terminal(..,
Mosh, ..)` on a real host now bootstraps a real `mosh-server`; the placeholder that closed the
session `Failed { Internal { "mosh terminals land with M3-A" } }` is gone.

**Open path** (`ssh/mosh_session.rs`, one thread `or2-mosh-open` per terminal, the same shape as
`or2-terminal`):

1. `peer_addr()` of the host connection, or the session closes
   `Failed { Internal }` (a transport that cannot say which address it reached; mosh must not
   run on a re-resolved name). `DirectTcp` always can.
2. The capability probe (cached). No `mosh-server` closes the session `Failed { NotInstalled {
   program: "mosh-server" } }` **before anything else runs**, so a host without mosh gets no pane
   focus and no exec channel, whatever the target is (AUTO then falls back to SSH, which reports
   a missing tmux or herdr itself).
3. The target's command as `RemoteCommand::argv()`, built by the code the SSH path uses
   (`terminal_session::plan`, which also returns the pane focus a herdr pane owes): `<tmux> -u new-session -A -s <name>`, `<herdr>` or `<herdr>
   --session <name>`, nothing for `Shell` (the login shell). `argv()` returns `None` for a
   command that carries environment assignments (an argument vector cannot), and the session
   closes `Failed { Internal }` rather than run the command without them; none do today. A
   herdr pane is focused over SSH, exactly as for an SSH terminal, **beside** step 4 (the two do
   not depend on each other; the app's own focus of the same pane, if in flight, is joined, and
   one it finished moments ago satisfies it): a focus that fails (`CommandFailed`, the pane is
   gone) stops the server step 4 already started before the session reports its failure.
4. `mosh::bootstrap(host, caps, size, argv)` with the probed path and UTF-8 locale; then
   `mosh::run_session` (the driver of `mosh::start_with`, see below) with `DirectUdp` and
   `Link` pinned to `peer_addr()` with the bootstrap's port (`set_port`, so an IPv6 scope id
   and flow label are kept). A resize during the bootstrap
   is carried into the session (latest wins); other input during it is dropped (the session is
   `Connecting`, as for SSH).
5. **Cleanup.** `mosh::terminate(host, server_pid)` runs **before** the session reports `Closed`
   whenever the session ends without ever having been `Connected` (UDP blocked: `Failed {
   TimedOut }` after `HostOptions::mosh_connect_timeout`, 15 s in production; any other failure;
   a disconnect) **and whenever it ends `Failed` after `Connected`** (an internal error such as
   a screen fault: the key lived only in memory, so nobody can reattach, and `mosh-server` has
   no idle timeout), **and whenever a user `Disconnected` after `Connected` was not confirmed
   by the peer**. The shutdown handshake ends the server only if the server acknowledges it:
   the driver reports an `Ended { reason, server_gone }` where `server_gone` is true only when
   the peer acknowledged the goodbye or announced its own end (`RemoteExited`); a goodbye that
   timed out (`GOODBYE_TIMEOUT`) or never got an answer (say the outbound UDP path broke while
   SSH stayed up) keeps the user-facing reason `Disconnected` but leaves `server_gone` false,
   and the bounded `terminate` over SSH then runs (`Session::finished()` is not proof: it is
   also true when the handshake merely gave up). The decision is
   `owes_cleanup(connected, reason, server_gone)`. This covers a terminal disconnect, the
   release of its last handle and a host disconnect.
   **Bounds.** `terminate` is bounded by `CLEANUP_BUDGET` (5 s, or the host's exec timeout when
   that is shorter), not by the exec timeout alone, so closing has a known worst case:
   `ABANDON_GRACE` (2 s) + `GOODBYE_TIMEOUT` (1 s) + `CLEANUP_BUDGET` (5 s) =
   `mosh_session::CLOSE_BUDGET` (8 s). A disconnect (session or user host disconnect) while the
   bootstrap exec is still running lets it finish for up to `ABANDON_GRACE` more so the server
   it started can be stopped; cutting the exec shorter loses the pid and the server stays (the
   limitation documented under `mosh::terminate`). A bootstrap that has **already returned**
   its pid never loses it: `prepare` runs the herdr pane focus beside the bootstrap, and the
   pid is recorded (`mosh_session::Started`) the moment the bootstrap finishes, outside that
   join, so a budget, dismissal or host disconnect that gives the start up while the focus is
   still pending (the grace then runs out on the focus) still stops the server (or owes the
   stop, see "Cleanup debt"). A stop cut short in the middle (the failed-focus cleanup itself)
   leaves the pid recorded, and the caller stops it again. **A disconnect starts nothing new:**
   `prepare` carries a cancellation flag the abandon sets, checked after the capability probe
   (before the pane focus and the exec, which start together), so a disconnect during the probe
   focuses no pane and starts no server; only an exec already running is waited for.
   **The host waits for this.** On a user disconnect of the host the host driver waits
   `SESSIONS_CLOSE_GRACE` (3 s) for its SSH terminals and watches as before, and for mosh
   sessions until `CLOSE_BUDGET` (8 s) after the close began (a second drain tracker, held by
   each mosh session's watcher), so the SSH connection stays up while a session stops its
   server over it, and the session's `Closed` still precedes the host's, also on a slow link.
   Past the budget the host closes anyway (a wedged session must not hold it open). A loss of
   the host releases the tracker at once, as before.
   **Cleanup debt (channel exhaustion).** `terminate` needs a fresh SSH exec channel, and an
   sshd at `MaxSessions` (OpenSSH's default 10, all taken by terminals on a healthy
   connection) refuses it. The stop is then **not** forgotten with the session: the server's
   pid goes into the host object's `ServerDebt`, and a background task retries it every second
   (bounded: 20 attempts, each bounded by `CLEANUP_BUDGET`) while the connection is up, so it
   goes through as soon as a terminal closes and frees a channel. On a user disconnect of the
   host, once its terminals have closed, the host makes one last bounded try (`CLEANUP_BUDGET`)
   at everything still owed before it tears the connection down. A stop that cannot be done
   (connection lost, no channel in time) is **stranded**: recorded in the host object
   (`ServerDebt::stranded`; `SshRemote::stranded_servers` for tests), not silently dropped; it
   is not surfaced through the FFI, and the server stays on the host until someone stops it.
   The retry task holds the host object (memory only) for at most the attempts' span. The
   exec inside `mosh::bootstrap` that stops a server whose answer was unusable is not covered
   (rare, and it ran with a channel a moment before).
   **Known leak windows.** If the host connection is already gone `terminate` fails quietly and
   the server stays until someone stops it. That is not only a narrow window: a host lost after
   the bootstrap but before the first datagram (the very case AUTO's fallback targets, UDP
   blocked on cellular, together with a flaky link) leaves the session waiting out
   `mosh_connect_timeout` and closing `Failed { TimedOut }` with a server nobody can stop from
   inside the app: the pid is known, but or2 keeps no key to reconnect with and Rust keeps no
   storage, so there is nobody to retry. Likewise a session that ends `Failed` after the host
   is lost, and a pid lost by a bootstrap cut off before it returned. Accepted for M3: the server's shell is the one
   the user would have been given anyway, and Android's reconnect (M3-B) uses a new server.
   **M3-B hand-off:** a `Failed { TimedOut }` close does not promise the server was stopped
   (a stop may still be waiting for a channel, see "Cleanup debt"); a
   transport fallback to SSH after it is still right, and the UI must not claim cleanup.
   The retained pid for a later best-effort terminate over a fresh host connection exists now, for
   the one case that matters most: a session that reached `Connected` and whose process then died
   (see "Orphan cleanup" under the M3 follow-up). The windows above, which end without a
   `Connected` session, still keep no record.

**Host close semantics.** A mosh session ignores the host connection once it is running.
`HostDriver` gives every terminal the host's `closing` watch, and a small watcher task per mosh
terminal reads it together with a second signal, the **user-cancellation signal**
(`host::UserCancel`, a `watch<bool>` in the host's shared state): `HostHandle::disconnect()`
sets it directly, and so does the release of the last handle (`Drop`), independent of the
command queue, so it still works after the SSH host driver has exited. A `Disconnected`
reason on `closing` (user disconnect, or release) or the cancel signal makes the session
disconnect with mosh's shutdown handshake (the server exits; the session is `Closed {
Disconnected }` before the host's `Closed`, as for SSH terminals, because the watcher holds
the host's drain tracker until the session has closed). Any other reason on `closing` is a
loss: the watcher drops its tracker at once (the host does not wait for the session) and the
session carries on, **but the watcher stays subscribed to the cancel signal**: a later
`HostHandle::disconnect()` or release of the lost host closes the surviving session
`Disconnected` with the handshake (and, if the goodbye is not confirmed, the cleanup over SSH
is attempted, which fails quietly with the connection gone). The host's own `Closed` is still
delivered exactly once (it happened at the loss). A disconnect that races the loss reaches the
session whichever the host driver sees first, since the signal does not depend on the driver.
The tracker is the mosh one (see "Bounds" above), waited for up to `CLOSE_BUDGET`, not the 3 s
of SSH terminals. SSH terminals keep M2's rule unchanged. After a loss the host takes no more
terminals (`HostError::Closed`), the mosh session remains usable, and its own `disconnect()`
closes it `Disconnected` with the handshake. The session keeps an `Arc` to the (dead) SSH host
object for its lifetime; that costs memory only.

**Shared driver.** `mosh::driver` was split so the host driver can own the `SessionDriver`:
`start_with` still creates its own channel and thread; `run_session(Plan, &mut SessionDriver) ->
CloseReason` (crate-private) runs the same loop on a driver the caller holds and does **not**
close it, so the caller can clean up first (`driver.state()` is still `Connected` iff the session
ever connected). `Plan` bundles the transport (`Arc`), the pinned peer, `MoshParams`, the
optional `HealthObserver`, the roam signal and `shutdown`, a `Notify` that ends the session like
`Command::Disconnect`, with the goodbye handshake (a permit given early is kept). There is no
`LinkControl` for host sessions: roaming goes through the session (`Command::Roam`, below).
`HostOptions` gained `mosh_connect_timeout`; `ssh::connect_host_with_datagrams(transport,
datagrams, request, observer, options)` chooses the datagram transport (tests, and Android's
network-bound transport later). `connect_host`/`connect_host_with` use `DirectUdp`.

**Link health.** `SessionDriver::publish_link_health` only delivers while `Connected`. The
mosh driver reports through a `HealthThrottle`: the first sample after `Connected`; the link
turning stale (`since_heard_ms > 5000`, the grey-out threshold, `STALE_AFTER_MS`); then once per
further whole second of silence while stale (so "Last heard 12 s ago" counts); and recovery. A
healthy link is **not** reported again: its values are millisecond-exact and would change on
every sample, which is a callback and a recomposition per session per second for nothing (the
intent of "only when a value changes"; the FFI contract's "at most once a second" holds as an
upper bound). `since_ack_ms` alone does not trigger a report; it is delivered with the next
one. The throttle also schedules the driver's wake-ups (`next_check`): a healthy link is looked
at again when it would turn stale, a stale one every second, so an idle healthy session wakes
only for its own protocol timers (and a session with no health duty at all, with no observer
and not yet connected, not for health). `mosh::start_with`'s separate `HealthObserver` hook is
unchanged: every sample, once a second, including before `Connected`, with no throttle. FFI
`SessionListener.on_link_health` is the observer's `link_health`. Never called for SSH
terminals.

**Roaming.** `Session.roam()` sends `Command::Roam` (any state; a closed session ignores it),
which the mosh driver turns into `request_rebind` (a new socket now, as `LinkControl::roam`);
SSH ignores it. `network_changed()` (FFI) calls `roam()` on every live mosh session through a
process-wide registry of `Weak<Session>` (registered when a Mosh session is created, pruned on
every call and registration of closed or released sessions; it never keeps a session alive), and
then `or2_core::ssh::network_changed()`, which sends an SSH keepalive on every established host
connection (the same weak registry, `LIVE_HOSTS`). What russh 0.63.3 offers: `Handle::send_keepalive(want_reply)`
queues a `keepalive@openssh.com` global request (reply requested), serviced by the connection's
own loop; `Handle::send_ping()` additionally waits for the reply. or2 uses `send_keepalive`: the
request and the server's reply cross the connection at once (a live test counts the bytes through
a relay). It does **not** give the connection a deadline: russh counts unanswered *scheduled*
keepalives (15 s, 3 misses) and an immediate one is not counted, so a connection that the
change silently blackholed is still noticed by the TCP/keepalive timeouts the write meets, not
by this call alone. The queueing itself is bounded (`KEEPALIVE_QUEUE_TIMEOUT`, 5 s): russh
stops reading its outbound queue while a TCP write is blocked, so on a blackholed link each
call's task would otherwise wait forever and a flapping network would pile them up; one that
times out is dropped (the connection is not draining, and the TCP and scheduled keepalive
timeouts already apply). That is the contract's wording ("does not by itself close a connection") and
is deliberate: a hard deadline here would turn every brief handover into a lost host.

**Tests.** `core/or2-core/tests/host_mosh.rs` (loopback sshd + real `mosh-server`, both
`OR2_REQUIRE_*`-gated): echo, submit, resize, roam (a second socket carries the output), link
health, session disconnect (the server process exits) and the connection serving the next
terminal; the tmux attach and fake herdr commands (and a failed focus starting no server);
host connection loss through a cuttable relay (the mosh session stays `Connected` and usable,
SSH terminals fail with the host's reason, the host takes no new terminals, the server exits
after the session's own disconnect); user host disconnect (sessions `Disconnected` before the
host, servers exit); blocked UDP (a transport that drops everything the server sends) gives
`TimedOut` with the server terminated and the connection intact; a disconnect, and a host
disconnect, before the first datagram terminate the server; a user disconnect of the host
while the bootstrap exec is still running, over a relay that holds every chunk 500 ms (a slow
link, with a `mosh-server` wrapper that waits first), takes longer than the terminals' 3 s grace
and still closes the session before the host with the server stopped; a disconnect during the
probe executes no `mosh-server` at all; host loss while the session is still connecting leaves
it to time out (`TimedOut`, not the host's `ConnectionLost`; the server then leaks, as
documented above); an unconfirmed goodbye (a `TestUdp` whose outbound path is muted while SSH
stays up) still stops the server over SSH after a terminal disconnect, the release of the last
handle and a host disconnect; a lost host's later `disconnect()` and release close its surviving
mosh session, and a disconnect racing the loss does too; with all ten SSH session channels
taken, the server of a mosh session that timed out is stopped once channels free up (on
terminal close, or in the host disconnect's last try), asserted by the server's disappearance,
not by the fixture's reaper. The mosh-server processes are
found by the fixture's private `TMUX_TMPDIR` in their environment and killed by exact pid when
the test ends however it ends. `connection_tests.rs`: `NotInstalled` for every target
without opening a channel or focusing a pane. `mosh::driver` tests: throttle, health through the
session observer (silent before `Connected`; healthy once; stale then every second; recovery),
the hook (every sample, also before `Connected`), `Session.roam`, the shutdown signal.
`mosh_session`: which session ends owe a cleanup, and the close budget covers its steps.
`remote`: `argv()` refuses an environment. `link`: the scoped IPv6 literal.
FFI unit tests: the registry (roams live mosh sessions only, forgets closed and released ones).
`HostConnectNativeTest` runs a real mosh terminal through the generated Kotlin bindings.

## Android

- **Foreground service** (type `specialUse`, subtype documented in the manifest) owns the
  application's `HostConnections` while any host or session is open; it stops itself when the
  last one closes. Its ongoing notification shows hosts and sessions and offers "Disconnect
  all". Request `POST_NOTIFICATIONS` (Android 13+) the first time a connection starts; the
  service still runs if it is denied.
- **Battery optimisation:** a one-time explanation and `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`
  **up front**, the first time the user starts a connection, in the foreground and before the
  first unlock (see "M3 polish"); never nag again.
- **Network callback:** `ConnectivityManager.registerDefaultNetworkCallback` → `network_changed()`
  on every default-network change; debounce 500 ms.
- **Transport preference** per host in Room v3 (`transport`: `AUTO` default, `SSH`, `MOSH`), real
  `Migration(2, 3)` with a migration test. AUTO uses mosh when `capabilities().mosh_server` is
  present and falls back to SSH (remembered for that connection, explained in muted text) when
  mosh fails with `TimedOut` or a missing `mosh-server`. The terminal header's transport badge shows the
  actual transport; link health greys the badge and shows "Last heard 12 s ago" when
  `since_heard_ms > 5000`.
- **Keys stay per-use.** No private key is retained to reconnect in the background. When the app
  returns to the foreground and an inbox host's SSH connection was lost, the app offers one
  grouped unlock (one biometric per distinct key) to reconnect; mosh terminals need no unlock.
- **Reattach:** the app remembers the last focused terminal (host id, target, transport) in
  app-private preferences. On return, if that session is alive it is shown directly
  (`request_full_frame`); if only its host is connected, the same target is reopened (herdr pane
  focused first); otherwise Home shows a "Resume" card that does both after unlocking.

### M3-B Android status and decisions (integrated on `m3/integrate`)

Built against API 8 and merged with M3-A (real mosh terminals, roaming and link health) and with UI-C
(the compact scale). Its JVM tests drive `contract_probe_host` and fakes; the Rust mosh path is
covered by M3-A's own tests, and no test here ties the service or the network callback to a real
mosh session. Where the text above left a choice open, this is what the code does.

- **Who owns the connections.** The process owns the one `HostConnections` (`Or2Application`,
  created lazily, so a service restart can never lose it); `ConnectionService` owns *when the
  process stays alive and what the user sees*: the foreground state, the ongoing notification,
  the network callback and "Disconnect all". This is the deviation from "service owns the
  `HostConnections`": the object stays in the Application, because the activity needs it before
  any service exists and after the service stopped (closed terminals keep their final frame
  there). The service starts when something opens and stops when nothing is open, which is the
  same lifetime.
  - Open means: a host connection that has not closed, or a terminal that has not closed (a mosh
    session whose SSH connection was lost still counts, and its host stays listed). Closed hosts
    and terminals the user has not dismissed hold nothing open.
  - `ServiceStarter` (Application, watching `HostConnections.serviceSnapshots()`) calls
    `startForegroundService` whenever something is open and the service is not running
    (`ServiceRunState`, cleared by the controller *before* it asks to stop, so a connection that
    opens a moment later starts a fresh service). Starting from the background (not allowed)
    is swallowed: connections only begin in the foreground. A snapshot only arrives when the open
    set changes, so `ServiceStarter.recheck()` covers a service that is gone while connections are
    still open (stopped from outside, or a start that was refused in the background): it runs when
    the service is destroyed (`ConnectionService.onDestroy`, after the controller closed, so a
    self-stop on the idle snapshot finds nothing open) and whenever the activity starts
    (`Or2Application.reviveService()`). It restarts nothing when the latest snapshot is idle.
  - `ServiceController` (JVM-testable, extracted from the `Service`) posts the first notification
    in `onStartCommand` at once, updates it on every change, and stops on the first idle
    snapshot. `START_NOT_STICKY`: a killed process has no connections to restore.
  - Notification (channel `connections`, importance low, ongoing): title `Connected to <host>` or
    `Connected to N hosts`, text `N open sessions`, one inbox line per host (`Alpha · 2 sessions`),
    action "Disconnect all" (`HostConnections.disconnectAll()`: every open terminal, then every
    live host; closed states stay visible as after any disconnect, and the service stops when all
    report `Closed`). Tapping the notification opens the app.
  - Manifest: `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_SPECIAL_USE` (with the
    `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` property), `POST_NOTIFICATIONS`, `ACCESS_NETWORK_STATE`,
    `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` (lint `BatteryLife` is suppressed on the one request:
    the Play policy does not apply to F-Droid).
- **Notification permission:** asked once (persisted flag `notifications_asked`), the first time
  the user starts a connection, then the connect proceeds whatever the answer. The hosts waiting
  for the answer are kept as ids in the activity's saved instance state, so an activity recreated
  while the dialog is up (rotation, a theme change, process death) still connects them when the
  answer arrives. `busy` is set from the tap until the connect starts: a second Connect tap does
  nothing meanwhile, and a Resume or reconnect that is waiting on the connect sees `busy` and not
  an idle host, so it is not abandoned (`resumeStep` gives up only when nothing is in flight).
- **Battery optimisation:** *superseded by "M3 polish" below.* The first M3-B text marked the
  explanation due on `onStop` and showed it on the next return, which put a modal dialog over the
  terminal; it is now asked up front, before the first unlock, and a declined exemption leaves a
  small non-blocking card on Home.
- **Network callback:** registered with the service on the main looper and removed with it.
  `NetworkChanges` tracks the default network's handle: a different network, or the same one
  after it was lost, is a change; the callback's first report of the network that was already the
  default is not. Changes within 500 ms collapse into one `network_changed()` (500 ms after the
  last event). *Extended by the follow-up:* a changed transport set or interface of the default
  network, and every return to the foreground, count too (see "M3 follow-up").
- **Room v3:** `hosts.transport TEXT NOT NULL DEFAULT 'AUTO'` (`TransportPref`: `AUTO`, `SSH`,
  `MOSH`), `Migration(2, 3)` is one additive `ALTER TABLE ... ADD COLUMN` (never destructive),
  schema exported as `3.json`. Tests: JVM (`MigrationSqlTest`: v2 and v1 databases migrate and
  equal a fresh v3, data and cascades intact) and device (`MigrationDeviceTest`:
  `MigrationTestHelper` for 2 to 3 and 1 to 3, then the DAO round trip). A transport change on a
  host does not end its connection (`connectionAffectedBy` ignores it); it applies to terminals
  opened afterwards, on a live connection too: `ActiveHost.host` is the snapshot taken at connect,
  so the preference that counts is `ActiveHost.transportPref`, which `MainActivity.saveHost`
  refreshes through `HostConnections.setTransport` (as it does the inbox flag). Terminals already
  open keep what they run over, and the change drops the connection's memory of an earlier mosh
  failure (below): a new preference is a fresh decision. The host form has the Auto / SSH / Mosh
  segmented control with one muted sentence under it.
- **Transport choice** (`chooseTransport`): `SSH` and `MOSH` are explicit and never
  second-guessed. `AUTO` uses mosh when the connection's capability probe found `mosh-server`
  and mosh has not already failed on this connection; before the probe has answered it uses SSH.
  **Every way of opening a terminal under AUTO waits for the probe first** (up to 3 s,
  `HostConnections.awaitTransportChoice`; an explicit SSH or Mosh never waits): the host screen's
  shell, tmux and herdr rows (`TerminalActivations.launchOpen`, with the usual `Opening host:
  target…` progress card), an inbox tap, and reattach. Only a probe that failed or timed out
  leaves AUTO on SSH, and no note is shown for that (there is nothing to fall back from).
  **Decision (deviation from the first M3-B text):** the transport a target ran over before
  (`LastTerminal.transport`) is *not* an input to the choice. Honouring it under AUTO pinned a
  target to SSH for good after one UDP-blocked fallback or after a tap that beat the probe, and
  carried that across connections and process restarts, which contradicts "remembered for that
  connection" and "a new connection starts without the memory". `LastTerminal.transport` is only
  what the Resume card shows (`Mosh`/`SSH`: how it was reached).
- **AUTO fallback:** a mosh terminal opened under AUTO that closes `Failed { TimedOut }` or
  `Failed { NotInstalled { program: "mosh-server" } }` *before it ever connected* (a missing `tmux`
  or `herdr` is not mosh's fault: the SSH retry would fail the same way, so it is shown as it is and
  does not mark mosh as rejected) is reopened over SSH on the *same*
  `ActiveTerminal` (the id, Home thumbnail and navigation stay; the failed state is never shown;
  the old native session is closed). This is remembered for the connection
  (`ActiveHost.moshFallbackNote`), so later AUTO terminals on it go straight to SSH, and every one
  shows the note in muted mono under the header (`Mosh could not reach the host over UDP. Using
  SSH for this connection.`). A new connection starts without the memory *(follow-up: a timeout
  is now also remembered per host in Room for 24 h, and AUTO's start has a 5 s budget; see "M3
  follow-up")*. Any other failure, a
  failure after the terminal connected, a user disconnect, and an explicit `Mosh` preference never
  fall back. If the SSH retry cannot even be started (the host closed meanwhile) the mosh failure
  is shown as it was, but an observed `TimedOut` is still remembered (in memory and in Room, see the
  follow-up): the memory records what UDP did, independent of whether the fallback could start, as long as the
  terminal is still its connection's, was not ended by the user and the host was not edited since.
- **Badge and link health:** `ActiveTerminal.transport` is what the session object reports; the
  header badge and the Home thumbnail's pill follow it (so they change on a fallback).
  `ActiveTerminal.linkHealth` holds the latest `on_link_health`; when `since_heard_ms > 5000`
  (exactly 5000 is still healthy) the badge is greyed (muted text on `surfaceTrack`, still
  reading `Mosh`; it also reports the state description `No word from the server`, which is what
  the device test asserts) and `Last heard N s ago` shows under the header in the attention colour;
  recovery clears both. The health resets on a fallback and when the session closes (a closed
  session hears nothing, and must not keep a stale line). The stale line and the fallback note are
  drawn *over* the first terminal row, on a `terminalBackground` scrim at 85 %, not above the
  terminal: showing or hiding them must not resize the grid, because a flapping link would send a
  resize (and a tmux/herdr redraw) each time.
- **Reconnect offer:** when the app returns to the foreground, stored hosts that are in the inbox,
  have a key and whose SSH connection *was lost* (connected once, then closed `Failed`; a deliberate
  disconnect, a remote exit and a connect that never worked are not losses) are offered in a
  dialog: "Reconnect" runs the existing grouped unlock (`connectGrouped`: one biometric per
  distinct key; the dialog says how many), "Not now" does nothing *(follow-up: a non-modal chip
  instead of a dialog, and never for a host marked as sleeping; see "M3 follow-up")*. No key is retained. When the
  user was on the terminal screen of one of those hosts when the app left, accepting also
  resumes that terminal once the host is connected. The offer is computed once, when the app
  returns; a connection that only fails a few seconds later (the keepalive after a network change)
  gets no dialog, and Home's host card (Failed, tap to connect) and Resume card are what remain.
- **Reattach:** `ReattachMemory` keeps `LastTerminal(hostId, target, transport)` in the app's
  private preferences (`or2-app`, key `last_terminal`, URL-encoded parts). It is written once a
  terminal screen is showing a terminal that is *connected now* and is not being closed (and again if its
  transport changes): `ReattachMemory.rememberShown` checks `ActiveTerminal.isOpenForReattach` (state
  `Connected`, not `disconnectRequested`, not retired) when it runs, not the history `hasConnected` and not the
  state the effect was queued with, and the screen's effect takes the terminal's state as an input. A terminal
  the user disconnected or closed, or whose shell exited, stays on screen with its final frame while the effect
  starts again on every recreation or revisit; remembering it from `hasConnected` would resurrect it and the
  next foreground return would reopen what was ended on purpose. A close processed before the effect runs wins.
  `ReattachRememberTest` runs the effect against the holder's real state (user disconnect, remote exit, a fast
  exit, dismissal, a lost connection) and the decision on return.
  **Host-wide closes mark every terminal of the host synchronously.** The user's Disconnect of a host, a
  dismissed or released connection (host deleted; destination, login or key edited) and an edit of the
  destination or login set `disconnectRequested` on **every** terminal of that host id, on whichever
  connection generation it was opened (the current one and older ones retained for surviving mosh terminals),
  before the native disconnect and long before `Closed` arrives: a queued or revisited remembering effect then
  cannot make one the Resume target again (`Closed(Disconnected)` does not forget), and the memory stays empty
  after `Closed`. The terminals stay listed with their final frames. An ordinary SSH loss or replacement does
  not mark anything: a surviving mosh terminal stays eligible and remembered. `HostCloseRememberTest` covers the
  Disconnect and the destination and login edits (current and older generations, before and after `Closed`), the
  final frames and the surviving-terminal case. It is
  forgotten when the user ends it: Disconnect or Close of the session, Disconnect of its host,
  deleting the host, "Disconnect all", or the remote shell exiting. Losing the network or the
  connection never forgets it.
  - Decision (`decideReattach`), in order: nothing remembered or its host is deleted, nothing;
    a live session for that host and target, **show it**; the host connected, **reopen the same
    target**; otherwise **Resume**.
  - On return to the foreground *after the terminal screen was showing when the app left*, "show"
    goes through `TerminalActivations.reuse` (a herdr pane is focused first, as every M2 way into an
    agent terminal is) and then calls `request_full_frame`; "reopen" goes through
    `TerminalActivations.reopen` (herdr pane focused first, the capability probe awaited for up to
    3 s so AUTO can still choose mosh; the transport is chosen afresh, see above).
    - **Without its SSH connection a terminal is shown as it is.** A herdr-pane terminal is focused
      only while its host connection is `Connected` (`TerminalActivations.needsFocus`). A mosh
      session outlives its SSH connection, so after the network dropped, "show", a Home thumbnail
      and the switcher all open the live terminal without a focus (and "show" still sends
      `request_full_frame`) instead of failing on the closed host. Decision: herdr's focus is shared
      state, so until the host is connected again that terminal may show a different pane than the
      one it was opened for; showing the live session beats refusing it. Once the host is connected
      again every activation focuses first, as before.
    - When only a Resume is left, a terminal screen for a terminal that is *gone* (dismissed, or a
      terminal id saved by a process that died) gives way to Home. A terminal that merely closed
      stays on screen with its reason and final frame until the user dismisses it, as in M2
      (the first M3-B text bounced it to Home and lost the failure text). After the app
      process died, a saved terminal destination that is not open any more is replaced by Home.
    - **Return state survives recreation.** The service keeps the process alive, so the system can
      destroy and recreate the activity while it is in the background. `returning` and
      `stoppedOnTerminal` are saved state, and the return work (reconnect
      offer, show/reopen) runs once the stored hosts have been read (a recreated activity starts
      with none), not at the `ON_START` itself.
  - Home shows a **Resume** card (`Alpha: herdr w1:p2`, `Mosh`) whenever the decision is reopen or
    resume. Tapping it reopens at once when the host is connected; otherwise it unlocks and
    connects through the usual grouped unlock and opens the terminal when the host reaches
    `Connected` (`resumeStep`; it gives up when the unlock is cancelled or the connect fails).
- **API changes of the review pass:** `HostConnections.openTerminal` lost its `remembered`
  parameter; `awaitTransportChoice`, `setTransport`, `TerminalActivations.open`/`launchOpen`,
  `ServiceStarter.recheck` and `ServiceRunState.begins` were added; `isMoshFallback` accepts
  `TimedOut` or a missing `mosh-server` only.
- **Compact scale (UI-C).** Everything M3-B added to the UI uses the compact theme tokens and no
  literal sizes of its own: the terminal header stays a 36 dp row (`Or2Dimens.HeaderRow`) with the 4 dp
  grab-handle inset, and the `SSH` badge uses full `text` (stale: `textMuted`); the stale and note
  lines are `MonoSmall` with the 12 dp `Gutter`; the host form's Auto / SSH / Mosh control is the
  32 dp `Segmented` with a `Secondary` caption; the Resume card is an `ActionCard`; the reconnect
  offer is an `Or2Dialog`; the notification is not themed.
- **Not done / left for acceptance on the phone:** the notification, service start from the
  foreground, the permission and battery dialogs and the network callback are exercised by
  `ConnectionServiceDeviceTest` and by unit tests of their logic but need the phone to see; the
  M3-A mosh path and the service have not run together against a real device, and the visual
  check of the compact scale for M3-B's additions is pending phone.

## M3 follow-up (advisor review, owner decisions 2026-10-01)

A Fable 5.1 strategy review found that, with ZeroTier carrying both hosts, the default network
rarely changes on Wi-Fi to mobile handover, and that OxygenOS process death is the most likely way
to fail v0 step 3. These changes landed after lanes M3-A and M3-B and are integrated with them
(FFI API **9**, then **10** with the orphan cleanup). Implemented, in the order of the review:

- **`resume_mosh`: rejected after review, not implemented.** The owner approved persisting a
  per-session ticket `{ host_id, target, transport, server_port, key, peer_ip, server_pid }` under a
  non-authentication-bound Keystore key and resuming a mosh session from it with no SSH
  connection. An external review proved that unsound, and a read of the vendored protocol code
  (`mosh/ssp`) agrees:
  1. **Nonce reuse.** The datagram layer is AES-128-OCB3 whose nonce is the direction bit plus the
     packet sequence number (`ssp/crypto.rs`, `ssp/packet.rs`); `PacketState::new_packet` counts
     from 0 for every `Session`. A client started from a persisted key restarts at sequence 0 and
     re-encrypts different plaintext under nonces the dead client already used with the same key.
     OCB nonce reuse leaks plaintext relations and allows forgeries. Persisting the counter does
     not fix it: the process dies at an arbitrary moment, so the last persisted value is always
     behind the last one used.
  2. **Missing transport state.** `mosh-server` diffs against the states the client acknowledged
     (the user-stream state numbers it received, the screen state the client holds). A fresh client
     has neither: the server drops its instructions (it no longer holds the `old_num` they name) and
     the client cannot apply the server's diffs (their base is the dead client's screen). Resuming
     would need the sender and receiver state, the acknowledged screen and the sequence counter
     persisted continuously, which is neither small nor safe.
  3. **A key at rest** that opens a live shell on a server that never times out, for little gain over
     what tmux and herdr already keep.

  **Chosen instead: a fast, prompt-free-but-for-the-fingerprint Resume after process death.** The
  last focused terminal (host, target, transport) is already remembered (`ReattachMemory`); the
  remote state lives in tmux or herdr anyway. When the system killed the process and the user comes
  back through the recents list (the saved destination is a terminal that no longer exists), the app
  goes Home and resumes at once, with no tap: one grouped biometric (one prompt per distinct key,
  `connectGrouped`), then the host connects, the remembered target reopens (a herdr pane is focused
  first, the capability probe awaited briefly so AUTO can still choose mosh) and the terminal is
  shown. `shouldAutoResume` decides: a remembered terminal whose host still exists, has a key and
  is not connected. *A cold start from the launcher has no saved destination; OxygenOS removes a
  killed app from recents, so that is the usual way back, and "M3 polish" resumes there too (a
  "sessions open" marker tells a process that died with sessions from one that ended in order).*
  Cancelling the prompt leaves the Resume card. SSH keys
  stay per-use; no mosh key is ever stored. After process death the old `mosh-server` is orphaned (its
  key died with the process; `mosh-server` has no idle timeout), and the reopened terminal starts a new
  one; the orphan is stopped by pid over the new SSH connection (see "Orphan cleanup"). The pane's tmux
  or herdr session is untouched.
- **Roaming triggers.** `network_changed()` also fires when the default network's **transport set**
  changes (`onCapabilitiesChanged`: Wi-Fi to cellular under a VPN keeps the same default network,
  and this is the only signal) or its **interface** changes (`onLinkPropertiesChanged`), not on every
  bandwidth or signal-strength callback (the first report is a baseline, a repeat of the same value
  does nothing), and on **every return to the foreground** (`MainActivity.onStart`), all through the
  one 500 ms debounce (`NetworkChanges`, now owned by `Or2Application` so the service's callbacks
  and the activity share it). `NetworkChanges.seed` starts tracking the network that is already the
  default without counting it. A call with nothing live is free (the registry is empty).
- **Auto fallback.** Under AUTO, mosh gets a **5 s budget for the whole start** (explicit Mosh keeps
  the 15 s default). It is an **absolute deadline** counted from the `open_terminal` call, so the
  capability probe, the pane focus, the `mosh-server` bootstrap, the UDP socket and the first
  authenticated datagram all spend from it (see "Deadline" below). A mosh terminal that closes
  `Failed { TimedOut }` before it connected is reopened over SSH on the same `ActiveTerminal` (as
  before), and the timeout is remembered **per host in Room**: `hosts.mosh_failed_until`, epoch
  milliseconds, now plus 24 h (`MOSH_PAUSE_MS`). Under AUTO a host with an unexpired memory skips
  mosh at once, with the muted note `Mosh could not reach this host over UDP recently. Using SSH.`
  (`MOSH_PAUSED_NOTE`), and tries again once the time has passed (expiry needs no write). The memory
  is cleared when the host's transport preference or its address list changes (`AppDao.saveHost`,
  in the same transaction; `HostConnections.setTransport` drops the in-memory copy). An explicit Mosh
  preference ignores it. Only `TimedOut` is remembered: a missing `mosh-server` is already known from
  the capability probe on every connection, and installing it must just work. A storage failure while
  remembering changes nothing else (the connection still remembers in memory). `mosh::terminate` runs
  on every path that ends without a connected session, the deadline included.
- **Return never blocks.** A live mosh pane is shown as it is on return, a resumable one resumes (see
  above), and nothing modal stands in the way: the reconnect offer for an inbox host whose SSH
  dropped is a **non-modal chip** (`ReconnectChip`, `Reconnect Alpha · 1 fingerprint`, a tap runs the
  grouped unlock, the close glyph dismisses; it floats with the other notices, at the top of a
  full-screen terminal), not the dialog of M3-B. The chip lasts while its hosts are still lost. A
  per-host **sleeps** flag (`hosts.sleeps`, the host form's "Host sleeps when idle" toggle, for a laptop)
  shows such a host's lost connection as muted `Asleep` (`LinkStatus.ASLEEP`: Home's card, the inbox
  row, the host screen) instead of a failure, and the reconnect offer never includes it. It still
  reads asleep only when the host went quiet (connection lost, unreachable, timed out): a rejected
  key or host key is a failure whatever the flag says. A tap on the host, or `Unlock` in the inbox,
  still connects it. The one-time battery explanation is asked up front, once (see "M3 polish").
- **Multi-address mosh.** Mosh pins to the address SSH actually reached. The host form says so under
  the address list: `In order of preference. All are tried; the first to answer wins. Mosh stays on
  the address SSH reached, so list the one that works on every network first.`
- **Reconnecting keeps the surviving mosh terminals.** A mosh session outlives a lost SSH connection, but
  disconnecting or releasing the native host object is the user's cancellation in Rust and closes every mosh
  session of that host (see "Host close semantics"). Reconnecting (the chip, a tap, Resume) used to retire the
  lost connection that way and so killed the very terminal it was meant to preserve. `HostConnections.replace`
  now tells *replacing the SSH connection* from *the user ending the host*: when mosh terminals opened on the
  lost connection (`ActiveTerminal.origin`) are still running it keeps that native object, untouched, in a
  `lingering` list and only the new connection becomes the host's. The old object is released (disconnect, then
  close) when its last mosh terminal has closed or been dismissed. An SSH-only lost connection is retired at once
  as before. The explicit paths still reach older generations: `disconnect(hostId)` disconnects every lingering
  connection of that host as well as the current one, `dismissHost`/`release` (host edit, deletion) retire
  them, and `disconnectAll` disconnects them (their terminals close through Rust's cancellation, each
  `Disconnected`, and the objects are released as they close). New input and output on the *original* terminal
  after a reconnect work: `HostConnectionsNativeTest` (real FFI, loopback sshd, local `mosh-server`) cuts the
  SSH connection by killing the fixture's session processes, reconnects, drives the original terminal, and then
  checks that an explicit host disconnect, and separately "Disconnect all", close it and release the old
  connection; `HostConnectionsReconnectTest` covers the bookkeeping on fakes (survivor kept, released on the
  last close or dismissal, several generations, delete, edit, disconnect all).
- **Manifest.** `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_SPECIAL_USE`, `POST_NOTIFICATIONS`,
  `ACCESS_NETWORK_STATE` and `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` were all already declared by M3-B
  (with `INTERNET` and `USE_BIOMETRIC`, seven in all); `ManifestTest` pins the list, the service's
  `specialUse` type and subtype, and the absence of Play Services and FCM.

**Deadline (FFI API 9).** `HostConnection.open_terminal` gained `mosh_budget_ms: Option<u32>` (the new
fifth parameter, before the listener): for `Mosh` only, ignored for `Ssh`. Core:
`HostHandle::open_terminal_within(target, transport, size, budget, observer)` (and
`HostCommand::OpenTerminal.deadline`) turns the budget into an absolute `tokio::time::Instant` at the
call; `open_terminal_with` is the same with no budget. In `mosh_session::drive` the deadline is a
branch of the loop that waits for the bootstrap: when it fires first, nothing new starts (the pane
focus and the exec check the same cancellation flag a disconnect uses), a bootstrap exec already
running gets up to `ABANDON_GRACE` (2 s) to report its server's pid, that server is stopped
(`mosh::terminate`, bounded by `CLEANUP_BUDGET`, 5 s) and the session closes
`Failed { TimedOut }`. The reason is the budget's whatever the overdue bootstrap then does: success
(its server is stopped), an error during the grace (a `CommandFailed` it returns late is dropped, so
AUTO still falls back) or running out the grace all close `Failed { TimedOut }`; a user disconnect that
won keeps `Disconnected` the same way. Once the bootstrap is over, the same instant bounds the socket open and the
first authenticated datagram (`Plan.deadline` replaces `connect_timeout`, which applies only without
one). The session therefore reports `Closed` at the budget in the usual blocked-UDP case plus one
exec to stop the server, and at worst at budget + 2 s + 5 s when the host is slow or half-gone (the
terminate that cannot reach a lost host is the documented limit). Tests: Rust live
(`host_mosh.rs`: a blocked start with a 1.2 s budget ends `TimedOut` well before the 15 s default and
the server is stopped, the connection serves the SSH fallback; a 0.3 s budget spent inside a 1 s
bootstrap still finds and stops the server the cut exec started; a generous budget is no delay and
the session lives on past it), core unit tests (`driver_tests`: the absolute deadline ends a silent
server, a past one at once; `host`: a budget becomes an absolute deadline at the call), and the real
FFI (`HostConnectNativeTest`: a 1 ms budget closes `TimedOut`, the connection intact).

**Room v4.** One schema bump for the follow-up: `hosts.sleeps INTEGER NOT NULL DEFAULT 0` and
`hosts.mosh_failed_until INTEGER NOT NULL DEFAULT 0`, `MIGRATION_3_4` (two additive `ALTER TABLE ... ADD
COLUMN`s), exported as `4.json`. Tests: JVM `MigrationSqlTest` (v3 to v4 and v1 to v4 equal a fresh v4,
data, trust and cascades intact) and device `MigrationDeviceTest` (`MigrationTestHelper` v3 to v4 and
the full chain, then the DAO round trip including the failure memory's clearing).

**Orphan cleanup (FFI API 10).** After the process dies the old `mosh-server` keeps running on the host.
The app records its pid and stops it over the next SSH connection to that host.

- **Core and FFI.** `Session.server_pid()` (`SessionHandle::server_pid`, `SessionDriver::set_server_pid`:
  an atomic in the session's shared state, `0` meaning none) is set by `mosh_session::drive` as soon as the
  bootstrap reported the pid, so it is readable before `Connected` and after `Closed`. `async
  HostConnection.stop_mosh_server(pid: u32)` (core `HostHandle::stop_mosh_server`, command
  `HostCommand::StopMoshServer`, answered through a `oneshot` like the other queries, bounded by
  `QUERY_TIMEOUT`) runs `mosh::terminate` on the host's connection. `terminate` only signals a process
  that `ps` names `*mosh-server`, so a pid that was reused by another program is left alone and a server
  that is already gone is not an error: `Ok` means "no such server runs any more". The stop script's exit
  status is read: `terminate` fails (`RemoteError::Failed`, `CommandFailed` over the FFI) when the script
  could not inspect the process (`ps` missing, or failing while `kill -0` shows the pid alive; status 3),
  when the signal was refused and the process is still there (status 4), on any other nonzero status and
  on a signalled exit. An empty `ps` answer counts as gone only from a `ps` that has shown it can
  answer a query by pid (it names the script's own shell) and only when `kill -0` does not find the
  pid either; with a `ps` that cannot, the stop fails (status 3) even if `kill -0` fails, because
  `kill -0` fails alike for a gone process and for one the user may not signal. After a refused
  `TERM` the second `ps` is read as strictly as the first: a status above 1 is a failure (status 4);
  only a clean answer naming no process (status 0 or 1, from the `ps` that already named it) counts as
  gone. Both ledgers (the
  host's `ServerDebt`, the Android record) therefore keep the debt of a stop that did not happen. An error
  (`CommandFailed`: no free SSH channel, a slow host, a failed stop; `Closed`: the connection ended;
  `InvalidName` for pid 0) means it may still run. Not-connected hosts answer `NotConnected`. The stop is a plain exec: it does not go through
  the host's `ServerDebt`, because the caller (the app) is the one that keeps the record and retries.
- **Record (`MoshServerLedger`, Kotlin).** `(host id, pid, identity)` entries in the app's private preferences
  (`mosh_servers`, `1:4242:9f2c...,7:555:41ab...`; no key, nothing secret). **The identity binds a record to the
  destination and login that created it:** `Host.moshIdentity()`, the first 128 bits of a SHA-256 over the
  username and the *ordered* address list with ports (the id, label, key, inbox flag and transport are not part
  of it). A pid only names a process on the machine and account that started it, and an edited host keeps its id,
  so `pids(host)` returns only entries whose identity equals the host's current one: a stop is never sent to an
  edited destination or another login, where the same number may be somebody else's live `mosh-server`.
  `HostConnections.hostEdited(previous, updated)` (the host form's save) also **invalidates**: when the address
  list or the username changed it purges the host's entries and forgets the remembered terminal (it named a pane
  of the old destination), and reverting the edit does not bring them back; a label, key, inbox or transport
  edit keeps both. Entries without an identity (written before it existed) are dropped on read: they cannot be
  tied to a destination. `HostConnections` records the pid when a mosh
  session reaches `Connected` (the process can die at any moment after), and clears it when the session
  closes `Disconnected` (Rust stopped the server before reporting the close, or the peer confirmed) or
  `RemoteExited` (the server announced its own end). A session that closes `Failed` stays recorded: its
  stop may not have reached the host (for example the connection was lost), and a repeat is harmless. A
  host that is deleted forgets its entries.
- **Stop on reconnect.** When a host reaches `Connected` (the Resume path, the reconnect chip, a tap),
  `HostConnections.reapOrphans` stops every pid recorded for that host except those of this process's own
  sessions **on that host** that have not closed (`orphanedServers`: a mosh session outlives a lost SSH
  connection, so a reconnect must not stop its own server; another host's live session with the same pid
  protects nothing here, since a pid only names a process on its own machine). It runs beside the connect (a launched coroutine on the
  connection's port, ahead of the capability probe's answer and of any reopened terminal), so Resume does
  not wait for it. A stop that succeeds clears its record; one that fails keeps it for the next connection.
- **Probe.** `contract_probe_host`'s mosh terminals report `server_pid` 4242 (`PROBE_SERVER_PID`) before
  `Connected`; `stop_mosh_server` succeeds for any pid but 13 (`PROBE_UNSTOPPABLE_PID`), which fails with
  `CommandFailed`, so the keep-on-failure path is testable without a host.
- **Tests.** Rust live (`host_mosh.rs`, real `sshd` and `mosh-server`): a session's `server_pid` is the
  fixture's `mosh-server`; after its SSH connection is cut and its UDP muted (a client that is gone without a
  goodbye) a new connection stops it by that pid and the process is gone; a pid naming another process (a
  `sleep` the test owns) is not signalled, a server already gone and a pid nothing has are `Ok`, pid 0 is
  `InvalidName`, a closed host answers `Closed`. Core unit tests (`host`: the command carries the pid and is
  answered through its reply; `session`: the pid reads before and after the close). JVM: `MoshServerLedgerTest`
  (per host and destination, durable, tolerant of foreign text, purge), `HostConnectionsMoshServerTest` (recorded at
  `Connected`, cleared on `Disconnected` and `RemoteExited`, kept on `Failed`, a new process stops the old
  process's server and forgets it, a failed stop is retried on the next connection, a live session's server
  is spared on a reconnect but only on its own host, other hosts' servers are untouched, deleting a host forgets
  its servers, a record is never sent to an edited destination or another login, also after a process death,
  and a destination or login edit purges the records and the remembered terminal) and the
  real FFI through the probe (`HostConnectionsProbeTest`).
- **Limits, accepted.** The record is written only for a session that reached `Connected`: a start that failed
  with a pid the stop could not reach (host lost during the bootstrap) is not recorded. A user disconnect
  whose goodbye was unconfirmed while the SSH connection was already lost is stranded in Rust
  (`ServerDebt::stranded`) and the app clears it as a normal close. Either leaves a server until someone stops
  it, as before. A pid is only a pid: stopping needs the host's `ps` to name it `mosh-server`, which is
  what makes a stale record safe.

**Not done / for the phone.** The service, the network callbacks (the transport-set trigger needs a
real VPN-carried handover), the chip, process-death auto-resume through the recents list (and the orphan
stop it now triggers), and the 5 s Auto budget on cellular have run only on fakes, the Rust suite and
compile-checked device tests; the acceptance protocol below is where they meet a phone.

### M3 acceptance protocol

Setup recorded once: battery exemption for or2 (and the ZeroTier app if used), OxygenOS sleep
standby setting, mobile data only, screen off while waiting, target the always-on host.
1. Cold open → inbox populated (logcat timestamps); ≤ 2 s when SSH is alive, else "unlock + N s".
2. Tap an agent → first frame of its pane ≤ 2 s; composer submit answered.
3. Background 10 min, screen off; at T+10 record service alive, process PID, SSH close time,
   max `since_heard_ms`.
4. Return: time to first full frame, number of prompts (target 0), focused pane matches
   (verified through herdr).
5. Variants: Wi-Fi→mobile mid-wait; airplane mode 2 min; process killed (`am kill`/force) → the
   implemented recovery: a fresh SSH reconnect (one grouped unlock) and the Resume path (auto-resume
   through the recents list or, with sessions open when it died, from a cold launcher start; Home's
   Resume card when the fingerprint is cancelled), which reopens the remembered
   target over a new mosh session, with the old `mosh-server` stopped by its recorded pid over the new
   connection (`resume_mosh`, resuming the dead client's session from a stored ticket, was rejected and is
   not implemented). Record the time to the first frame, the prompts, and that the orphan is gone. Three
   runs each, report p50 and max.

## M3 polish: latency, battery, cold-launch resume (integrated on `m3/polish`)

Phone acceptance (Wi-Fi, OnePlus/OxygenOS 16, about 117 ms RTT to the host) found the critical paths
far from 2 s and two Android behaviours that defeat "stays connected". This section records what
changed; the rules it touches are also updated where they live (probe, herdr, mosh, Android).

The FFI is unchanged (API stays **10**): the latency work is inside `or2-core`, the timing markers use the
existing callbacks, and the Android changes are app-side.

### Latency fixture and measured numbers

`core/or2-core/tests/latency.rs` (feature set of the other sshd tests; needs `sshd` and
`mosh-server`) puts a real disposable OpenSSH behind the shared relay (`common::Proxy`) holding
every chunk 60 ms each way (120 ms RTT), a fake `herdr` script that lists one running session, a
real Unix-socket server speaking herdr's wire format (subscribe acknowledged, the checked-in
snapshot, `pane.focus`, `pane_not_found` for `w9:p9`) and a real `mosh-server` (UDP is local: add
one round trip on a real link). It prints the table below (`-- --nocapture`) and asserts the
protocol work exactly (how many `herdr session list` runs, herdr connections and `pane.focus`
requests) and the times with room for a loaded machine. Measured on the runner, before this
change (main `9039f61`) and after, same fixture, same run:

| Path (120 ms RTT) | before | after |
|---|---|---|
| `capabilities()` right after `Connected` | 928 ms (7.7 RTT, 3 sequential execs) | 283 ms (2.4 RTT, 2 execs at once) |
| inbox: `Connected` to first `Live` view | 1815 ms (15.1 RTT, 3 `session list` runs) | 726 ms (6.1 RTT, 1 run) |
| reuse: pane focus only | 564 ms (4.7 RTT) | 241-282 ms (2.0-2.4 RTT) |
| tap: focus, then a mosh terminal on the pane, `Connected` | 1454 ms (12.1 RTT) | 608 ms (5.1 RTT) |
| tap: focus and open started together | n/a | 322 ms (2.7 RTT) |
| SSH terminal on a pane (focus beside the channel) | n/a | 523 ms (4.4 RTT) |

What was cut: the probe's herdr listing runs beside the probe script (not after it) and seeds the
connection's directory; `capabilities()` right after it does not list a third time; a watch and a
focus take their socket from the directory (no `session list`, no discovery per call); a watch's
two streams open together; the app's focus and the terminal's own focus are one request (the
`FocusGate`); the pane focus runs beside the mosh bootstrap or the SSH channel open instead of
before it. The unreachable-host test connects a second, healthy host while a first one accepts TCP
and never answers the handshake (20 s timeout) and checks the healthy host's connect and inbox are
unaffected: each host is its own driver thread, and the Android flows are per host.

### Battery exemption up front

OxygenOS lets the SSH connections die within about ten minutes in the background unless the app is
exempt from battery optimisation (mosh survives; with the exemption SSH does too). The explanation
used to appear as a modal dialog over the terminal on the first return from the background. Now:

- **When.** The first time the user starts a connection (any Connect, Resume, reconnect chip or
  automatic resume), after the `POST_NOTIFICATIONS` request and **before the first unlock**, in the
  foreground: `MainActivity.connectAfterNotifications`, while `busy` is set. If `BatteryPrompt.shouldExplain()`
  (never explained, and not already exempt) it shows the explanation ("Keep sessions connected",
  Allow / Not now) and holds the connect (the host ids are saved state, like the notification
  request's, so a recreated activity still connects them). "Allow" opens
  `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` through an activity-result launcher and the connect
  (and its fingerprint prompt) goes on when the system dialog closes; "Not now" (or back, or a device
  with no such screen) goes straight on. The explanation is recorded as asked when it is answered, **once,
  ever**, whatever the answer; an already exempt app is never asked.
- **Surviving recreation and process death.** The saved host ids (`pending_battery`) keep `busy` set
  across a recreated activity, so what stage the flow was in must be recoverable:
  `BatteryPrompt.restoreStage()`, called from `onCreate` when `pendingBattery` was restored, reads it from
  the persisted "asked" flag (the explanation itself is memory-only). *Not asked yet*: the explanation
  never got an answer, so it is raised again (`EXPLANATION`; a rotation in the same process finds it
  still up). *Asked*: "Allow" launched the system request, whose result the re-registered launcher
  delivers and carries the connect on (`SYSTEM_REQUEST`); the request is never launched twice. *Not asked
  and the app is exempt meanwhile*: nothing to ask, the connect goes on (`PROCEED`). Test: `ReattachTest`.
- **Never on return.** `onStop`/`onStart` no longer touch the battery prompt: nothing modal appears
  over a terminal.
- **If it was not granted.** `BatteryPrompt.card` is true when the exemption was declined (or the
  system dialog was refused) and is not in place: Home shows a small card above SESSIONS, "Background
  connections may drop", with **Allow** (the system request again, as a plain `startActivity`) and a
  close glyph that dismisses it for good. It blocks nothing, and goes away by itself once the
  exemption is in place (re-read on every `onStart`). Tests: `ReattachTest` (the prompt's states and
  persistence), `HomeUiDeviceTest` (the card, compile-checked here).

### Cold-launch auto-resume

After the process is killed OxygenOS removes or2 from recents, so the user comes back from the launcher,
which has no saved destination, and used to get Home's Resume card (a tap, then the fingerprint). Now
a cold launcher start resumes at once, exactly as the recents path does:

- **The marker.** `SessionMarker` (app-private preference `sessions_open`) is written by `Or2Application`
  from the service's snapshots: set while any terminal is open (`ServiceSnapshot.sessions > 0`) and
  cleared the moment none is (Disconnect all, the last close, a remote exit, loss of every session).
  A killed process leaves it set. The next process reads it **once, when it is created**
  (`diedWithSessions`, before it writes anything) and hands it to the first activity that asks
  (`takeColdResume`, one-shot per process, so rotation or a recreated activity never resumes twice).
- **The decision.** `shouldAutoResumeOnLaunch(diedWithSessions, last, hosts, connectedHosts)` is
  `diedWithSessions && shouldAutoResume(...)`: a remembered terminal whose host exists, has a key and is
  not connected. A target the user closed on purpose is not remembered (`ReattachMemory` forgets it),
  so it never resumes, and an orderly end cleared the marker. `Or2App` evaluates it once the stored
  hosts are read (the same effect as the recents path, which still handles a saved terminal
  destination), starts `resumeLast()` (one grouped biometric, connect, reopen the remembered target)
  and leaves Home's Resume card as the fallback when the fingerprint is cancelled.
- **Surviving recreation.** The marker is consumed once, so the continuation it starts is saved state:
  `pendingResume` (the terminal to reopen once the host is connected) is `rememberSaveable` through
  `PendingResumeSaver` (`LastTerminal.encode`/`decode`), not `remember`. A rotation or restored process
  while the biometric, the battery explanation or the connect is pending therefore still reopens the
  remembered target exactly once when the host connects (`ResumeStep.OPEN` clears it), with no extra
  tap; a connect that ended without a connection still gives up (`ABORT`) and clears it. Tests:
  `ReattachTest` (the saver), `PendingResumeDeviceTest` (state restoration, compile-checked here).
- Tests: `ReattachTest` (marker lifecycle across "processes", the one-shot, the decision, a closed
  target never resumed).

### Timing markers (`or2.timing`)

One logcat tag, **`or2.timing`**, debug builds only (`Or2Application` passes a sink to `Timing` only when
the app is debuggable; a release build records nothing). Read it with:

```sh
adb logcat -v time -s or2.timing:D
```

One line per marker, `<path> <event> ms=<since the path began>`, naming hosts by id and herdr panes by
pane id only (no label, address, user name, key or output):

| Path | Events |
|---|---|
| `connect host=N` | `unlocked` (the biometric is done; ms=0), `authenticating`, `connected`, `capabilities` (the probe answered), `live` (the first herdr view of the host), or `failed` (closed before it connected, e.g. 20 s for an unreachable host) |
| `tap host=N pane=P` (inbox tap) | `begin`, `focused` (herdr acknowledged the pane focus), `terminal-connected`, `frame` (a frame was drawn: see below) |
| `reuse host=N pane=P` (an open terminal) | `begin`, `focused`, `frame` |
| `reopen host=N` (return to the foreground) | as `tap` |
| `resume host=N` (Resume card or automatic resume) | `begin`, `host-connected`, then the reopen's `focused`, `terminal-connected`, `frame` |

**`frame`** is reported by the terminal view after a frame it applied has been drawn and committed
(`onDraw`, then `ViewTreeObserver.registerFrameCommitCallback`), not when the frame was only asked
for: rendering and scheduling delay are included. The view reports every such frame
(`TerminalView.onFrameDrawn`; `Timing.terminalFrame` ignores it unless a path is watching that
terminal), so a view that is retained (the app returns from the background, the same terminal is
reused) ends each later activation's span too: `watchTerminal` arms it again, and a retained view
shown again reports its next draw even when its content did not change (`AppliedFrames`, tests
`AppliedFramesTest`, `TimingTest`). A blank draw before the first frame is not reported.

For a new agent terminal the focus and the terminal start together, so `focused` and
`terminal-connected` may come in either order. `connect` shows how long each host took from the
fingerprint (an unreachable host's 20 s never delays the others: they are separate paths). No FFI change:
the markers use the existing callbacks. Tests: `TimingTest`, `TerminalActivationsTest`.

### Agent taps start the terminal beside the focus

`TerminalActivations.openAgent` and `reopen` (`openOrReuse`): a terminal that is already open is reused
after its pane is focused again; a new one is **opened while the pane is being focused** (Rust joins the
two, see the herdr focus gate), and the wait ends when both are done. A pane that cannot be focused (it
vanished) leaves no terminal: the one that was opened is dismissed, as is one whose wait was cancelled.

### Unreachable hosts explain themselves

The host card (Home) and the host page show, under the failure and in muted mono, what **each address**
did: `blackstark.local:22 \u00b7 name not resolved (mDNS) after 3 tries` and `10.255.255.1:22 \u00b7 no answer
within 6 s`. The core names addresses by position only; `unreachableDetail` puts the host's own names
back (an `Unreachable` message of the form `address N: <outcome>; address M: ...`, see "Address
racing"). A host with the **sleeps** flag whose connection ended `Unreachable`, `TimedOut` or lost reads
as muted `Asleep` rather than an error (the detail still shows underneath); a rejected key or host key
is a failure whatever the flag says. Tests: `SessionMessagesTest`, `HomeModelTest`.

### Not done / for the phone

- The measured numbers are over a modelled 120 ms link on the runner; the phone's own times (its CPU,
  the ZeroTier path, the UDP round trip of a mosh start) are what `or2.timing` is for: read
  `connect host=N connected / capabilities / live`, `tap ... focused / terminal-connected / frame`
  and `resume ...` on a debug build, and compare each leg with the round trips counted above.
- The cold-launch resume, the up-front battery request (and OxygenOS's own dialog), and the Home card
  have run only on fakes and compile-checked device tests (`HomeUiDeviceTest`); the biometric prompt
  at a cold start is the thing to watch.
- The per-address timeout and the `.local` retry are tested against scripted resolvers and a
  blackholing transport; the real Android resolver (and a first mDNS lookup that fails after 1.5 s) is
  the phone's to confirm.
- `tests/mosh_live.rs::terminate_stops_a_server_nobody_connected_to` failed once in one full workspace
  run (a UDP-port-to-pid lookup of the test's own helper) and passed on every rerun, alone and in the
  suite; it does not touch this change and is recorded here rather than hidden.

# Easy pair (QR onboarding)

Pairing a new host should take one command on the host and one scan on the phone, without
weakening M1's trust model. Manual host entry stays available.

## Host side: `or2-pair` CLI

A small Rust binary in a new workspace crate `core/or2-pair` (the one justified new crate: it is
a separate host-side tool, not part of the app library). Builds for macOS, Linux and Windows
(OpenSSH for Windows). Installed with `cargo install`, a Homebrew formula building from source,
or release binaries later. GPL-3.0-or-later; dependencies exactly pinned (e.g. `qrcode` for
terminal QR rendering).

1. **Checks** and reports, without changing anything: sshd reachable on the chosen port
   (macOS: Remote Login; Linux: sshd running), the user's `~/.ssh/authorized_keys` writable,
   tmux/herdr/mosh-server presence, and a firewall hint for mosh UDP 60000–61000.
2. **Gathers** username, SSH port, the host's ED25519 public key (from `/etc/ssh` or via
   `ssh-keyscan` of localhost; RSA/ECDSA only if no ED25519 exists), and every address: LAN IPs,
   the mDNS name (`<name>.local`), overlay IPs (ZeroTier `zt*`, Tailscale `tailscale*`/100.64/10),
   ordered with addresses that work on every network first (overlay, then LAN, then `.local`).
3. **Prints a QR code** (UTF-8 half blocks; `--ascii` fallback) and the same payload as text for
   manual entry. Payload (URI, ≤ 1 KB):
   `or2-pair:1?name=<label>&user=<u>&port=<p>&a=<addr1>&a=<addr2>…&hk=<algo> <base64>&pair=<ip>:<port>&otp=<base32 128-bit>`.
   `hk` is the host's full public key, so the phone can trust it from the scan.
4. **Listens once** on `pair` (a random port, bound only to the LAN/overlay addresses listed,
   never 0.0.0.0 on a public interface) for at most 120 s. Exchange (newline-delimited JSON):
   server → `{"v":1,"nonce":<base64 32B>}`; phone → `{"v":1,"key":"<openssh public key line>",
   "device":"<label>","mac":<base64 HMAC-SHA256(otp, nonce || key)>}`; server verifies the HMAC
   in constant time, prints the key's SHA-256 fingerprint and the device label, and asks
   `Authorize this key for <user>? [y/N]`. On `y` it appends
   `no-agent-forwarding,no-X11-forwarding <key> or2-<device>-<date>` to `authorized_keys`
   (creating `~/.ssh` 0700 / file 0600 if needed, backing up the file first, skipping
   duplicates) and replies `{"ok":true}`; otherwise `{"ok":false,"reason":…}`. One attempt;
   any failure or timeout ends the listener. The OTP never crosses the network.
   `--no-listen` prints the QR without a listener (the phone then shows its public key line for
   the user to install by hand).

## Phone side

- **Scan:** "Add host → Scan QR" uses CameraX (AndroidX) + ZXing core (Apache-2.0); no Google
  Play Services. "Paste pairing code" accepts the text payload.
- **Parsing and the exchange are Rust** (`or2_core::pair`, FFI `parse_pair_payload(text)` and
  async `pair_submit_key(payload, public_key_line, device_label)` through `Transport`): strict
  validation of every field (addresses as `Endpoint`s, `hk` as an OpenSSH public key, OTP
  length, URI version), bounded reads, 10 s timeout.
- **Flow:** scan → review screen (name, user, addresses, the host key's fingerprint, which key
  will be authorized; the user may pick an existing key or generate a new one) → submit →
  "Confirm on the host: fingerprint SHA256:…" → on `ok`, the host is saved with its addresses and
  **`hk` is persisted as a trusted host key** before the first connection (so no first-use
  prompt; a different presented key is the M1 changed-key path, never auto-accepted) → connect.
- Never log the payload or OTP; wipe the OTP after use.

## Tests

CLI: payload round trip, address ordering, HMAC verification (good/bad/replayed nonce),
`authorized_keys` append semantics (create, permissions, backup, duplicate) in a temp HOME,
listener timeout and one-shot behaviour, bind-address policy. Rust core: parser fuzz-ish table
tests, exchange against the real CLI listener in-process (loopback). Kotlin: review-screen and
persistence logic with fakes; JVM end-to-end against a CLI listener on loopback; device test
compiles (camera needs the phone).
