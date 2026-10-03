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
- **The host presents a trusted key if it has one.** sshd presents its key of the first host-key
  algorithm in the client's list that it has a key for, and most hosts have ED25519, ECDSA and
  RSA keys. The client therefore lists the algorithms of the trusted keys first (an RSA key
  counts for `rsa-sha2-512`, `rsa-sha2-256` and `ssh-rsa`), in russh's default order among
  themselves, then every other default algorithm (`ssh::client::config`). A host trusted by its
  RSA or ECDSA key is never asked for its ED25519 key first (which would be a false "changed"
  prompt); a host whose trusted key is ED25519, or with nothing trusted, sees russh's default
  order (ED25519 first). The other algorithms stay on offer, so a host that has none of the
  trusted keys any more still handshakes and gets the changed-key prompt, about its key of the
  trusted kind when it has one. The negotiation is one round: with trusted keys of several kinds
  the host presents its key of the first kind it has, trusted or not (trust holds one key per
  host in practice: approving replaces it). Tests: `ssh::client` unit tests (the order),
  `or2-core/tests/host_keys.rs` (a disposable sshd with all three kinds of host key).
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
  for a host. The host-key algorithms are offered trusted kinds first (M1 "Host-key trust"), so
  a host with several keys presents the trusted one. Everything after `Connected` (the orphan
  reap, the capability probe, tmux, herdr watches, SSH terminals, mosh bootstraps) runs on this
  one connection: nothing else handshakes, so nothing else can prompt.
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
A terminal's channel open belongs to the **host connection**, not to the terminal
(`SshHost::start_open`): it runs as a task of the connection's own task set until the server
answers, and the terminal only waits for its answer (a timeout gives up at once). A user
disconnect that arrives while the open (or the focus beside it) is still pending first waits up to
`CHANNEL_CLOSE_GRACE` (250 ms) for the answer, so a channel confirmed in that moment is closed
before the session reports `Closed`; the grace is only that ordering courtesy, not the bound on
the cleanup. Whenever the confirmation arrives later, however late, the connection's task finds
nobody waiting and closes the channel itself (an error answer needs nothing), so no session
channel leaks onto the live connection and holds a `MaxSessions` slot. The tasks end with their
answer or with the connection (the connection's `hold` drops the set when it ends, also when the
host driver aborts it, and a connection that is over starts none), so none outlive the host.
The channel is closed exactly once and the terminal's `Closed` is unchanged. Tests
`a_terminal_given_up_while_its_focus_waits_closes_the_channel_opened_beside_it`,
`an_open_confirmed_within_the_grace_of_a_given_up_terminal_is_closed`,
`an_open_confirmed_after_a_given_up_terminal_closed_is_still_closed` and
`an_open_the_server_never_confirms_ends_with_the_connection`. An exec's channel open
(`exec_rendered`: the probe, tmux and herdr execs) takes the same path (the connection holds a
`Weak` to its own `Arc`, so the `&self` trait method can start one): a confirmation that arrives
after the exec's deadline (`TimedOut`) is closed by the connection, test
`an_exec_whose_open_is_confirmed_after_its_deadline_has_the_channel_closed`. So does the
direct-streamlocal open (`open_unix`: the herdr client, watch and focus sockets), test
`a_streamlocal_open_confirmed_after_its_deadline_has_the_channel_closed`. Every channel open on
the connection is therefore the connection's own task.
**Ownership across the hand-off.** The task delivers the confirmed channel to the caller inside a
guard (`OpenedChannel`) that derefs to the channel; `PendingOpen::wait` returns the guard itself,
and a caller cancelled after the task delivered but before it received (an exec or a streamlocal
open dropped, a `PendingOpen` dropped) leaves the answer queued, which `PendingOpen`'s `Drop`
receives and drops, so the guard closes the channel. Tests
`a_delivered_session_open_dropped_unconsumed_is_closed`,
`a_delivered_streamlocal_open_dropped_unconsumed_is_closed`,
`an_exec_cancelled_after_its_open_was_delivered_has_the_channel_closed`,
`a_streamlocal_open_cancelled_after_delivery_has_the_channel_closed` (and, as controls, the two
`..._taken_and_closed_by_the_caller_is_closed` and
`pending_opens_end_on_host_close_without_retaining_the_host`).
**One close obligation, no deadline.** The invariant: *from the moment the server confirms a
session channel until either its `Close` is queued or its connection ends, the channel is owned by
a guard or by a task of the connection's own set; no path drops it with the `Close` unqueued.*
Closing is `SshHost::close_owned`, the only place a channel's `close()` is awaited: the future
that owns the channel is moved into a task of the connection's set which awaits it with **no
deadline**, because russh's `close` waits for room in its bounded command queue (ten messages,
full when the shared reader is stuck behind a slow write or another terminal) and dropping that
await, by a cancellation or a timeout, drops the raw channel without a `Close` ever queued. The
task ends when the `Close` is queued, or with the connection (a channel goes with its connection,
and `hold` ends the set). Everything else hands the channel over: `OpenedChannel::close` and
`OpenedWriter::close` (the write half of a split channel that `pump_channel` runs) await a oneshot
from that task, and may be bounded or cancelled by the caller (the terminal's `Closed` waits
`CHANNEL_CLOSE_GRACE` for it and no longer), the task keeping the obligation; the `Drop` of both
guards, `ExecChannel`'s drop and a cancelled or aborted pump or terminal task start the same task.
A guard is disarmed only by `into_inner` (a hand-over to the next owner with no `await` in
between: `open_unix` into a `ChannelStream`, which russh itself closes on drop with an unbounded
send, and `ExecChannel::finished`, once the server has closed the channel) or when the server's
own `Close` ended the pump's read side (`pump_channel`). Nothing else disarms: the pump's local
input ending (the terminal's write queue closed while the connection stays healthy), a failed
write, a stop and a cancellation all hand the channel to the close task, which on a connection
that is gone simply ends. There is no exception to the invariant. A terminal task that the host aborts
(the 2 x `CHANNEL_CLOSE_GRACE` wait in `drive` ran out) therefore still closes its channel, once,
wherever it was; a terminal `Closed` can precede the `Close` reaching the server when the queue is
full, never replace it. Tests (the queue is filled by freezing the shared reader at an open
confirmation and sending ten keepalives): `a_cancelled_guard_close_with_a_full_queue_still_closes_the_channel`,
`a_timed_out_guard_close_with_a_full_queue_still_closes_the_channel`,
`a_dropped_guard_with_a_full_queue_still_closes_the_channel`,
`a_cancelled_pump_with_a_full_queue_still_closes_the_channel`,
`a_pump_stop_with_a_full_queue_still_closes_the_channel`,
`a_cancelled_exec_close_with_a_full_queue_still_closes_the_channel`,
`a_terminal_disconnect_with_a_full_queue_still_closes_its_channel_once_the_queue_drains`; and
`an_aborted_terminal_task_closes_a_running_channel_exactly_once`,
`an_aborted_terminal_task_closes_a_channel_still_waiting_for_its_focus_exactly_once`,
`a_terminal_task_aborted_while_pty_or_shell_reply_is_pending_closes_its_channel_once`,
`a_pump_whose_local_input_ended_closes_its_channel_on_the_healthy_connection`,
`a_running_pump_cancelled_under_a_full_queue_still_closes_the_channel`, with the
controls `a_guard_close_the_queue_lets_finish_closes_the_channel_once` and
`a_guard_dropped_after_the_host_is_gone_does_nothing`.

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

- **Types** are generated from `herdr api schema --json` by `cargo xtask gen-herdr-types`
  (`core/xtask/src/herdr.rs` normalizes, then cargo-typify). The normalized schema
  `herdr/schema.json` (with the herdr version and protocol it came from: 0.9.3, protocol 22)
  and `herdr/generated.rs` are checked in; `generated.rs` has one module per schema family
  (`request`, `success_response`, `error_response`) and the
  constants `HERDR_VERSION` and `PROTOCOL`. Never hand-edit it: fix the generator and
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
  consumer adds the families to `core/xtask/src/herdr.rs` with an open event-type tag.
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
    only through lifecycle-driven reads, and the request is tried again only once a read finds
    other panes), so a persistent rejection cannot make the view flap between `Live` and
    `Unavailable`.
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
  focus and the terminal's own from both reaching herdr. A herdr session ends on the pane whose
  request it received last, so every decision about a session's focuses is made by **one actor
  task per session** (spawned by the first request; it ends when idle with no acknowledgement
  left to keep). Callers (`FocusGate::focus(&Arc<host>, herdr, &Arc<Directory>, session, pane,
  accept_recent)`) send `(pane, reply)` to it over a channel; nothing else touches its state. It
  owns a FIFO of pending requests (a pane and the callers waiting for it), the one request in
  flight, and `recent`: the pane herdr was last successfully focused to by us, valid only while
  the FIFO is empty and nothing is in flight. A request for the pane of the FIFO's tail (or of the
  in-flight request, FIFO empty) **joins** it and shares its answer (a failure included); a
  terminal's (`accept_recent`) request with the FIFO empty, nothing in flight and `recent` the
  same pane and younger than `focus::RECENT` (2 s) is answered from memory; anything else is
  queued and clears `recent` (A, B, A sends A, B, A). Requests are sent strictly one at a time in
  FIFO order, so the order they reach herdr is the order they were asked, whatever the
  socket-open latencies. On success `recent` is the pane only if the FIFO is empty by then; on
  failure it is cleared (herdr may have acted). **Cancellation** only drops a caller's reply
  channel and never mutates the actor from the caller side: a queued request whose callers have
  all gone is skipped (never sent, not pending), one with a live caller is sent, and a request
  already in flight completes and updates `recent` like any other (a cancelled tail therefore
  neither blocks the queue nor lets an older pane's acknowledgement stand while a newer request is
  pending, and a same-pane request after it still joins the pending one). Focus A, focus B, open a
  terminal on A therefore sends a focus of A; with A's request held open while B starts, B is sent
  after A, and a terminal on B is then satisfied from memory but one on A is not. Sessions have
  their own actors and are independent (a held focus in one does not delay another). The app's own request
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
  all". `POST_NOTIFICATIONS` is **never asked on connect**: the service starts and runs without it
  (on Android 13+ only its notification is not shown), and Home offers it in context, as a small
  dismissible card while a host is connected (see [Permissions: none on connect](#permissions-none-on-connect)).
- **Battery optimisation:** a one-time explanation and `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`
  as the **last step of adding a host** (after Easy pair, after the manual form's save), never during a
  connect; never nag again (see [Permissions: none on connect](#permissions-none-on-connect)).
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
- **Notification permission:** *superseded by [Permissions: none on connect](#permissions-none-on-connect):
  nothing is asked on connect any more; the flag below is read only so that a user who answered is not
  offered it again.* Asked once (persisted flag `notifications_asked`), the first time
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
  still connects it. The one-time battery explanation is the last step of adding a host, once (see
  [Permissions: none on connect](#permissions-none-on-connect)).
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
  session reaches `Connected` (the process can die at any moment after; since API 14 it is recorded
  earlier and durably, at `on_server_pid`, see "v0.1.1 review fixes" under Instant opens), and clears it when the session
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
  with a pid the stop could not reach (host lost during the bootstrap) is not recorded (API 14 lifts this: every
  session whose pid is known is recorded, and one that fails before connecting keeps its record). A user disconnect
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
`FocusGate`, which also sends a session's focuses one at a time so herdr receives them in the order
they were asked and an acknowledgement is trusted only as the session's last; see "Focus" under the
herdr client); the pane focus runs beside the mosh bootstrap or the SSH channel open instead of
before it. The unreachable-host test connects a second, healthy host while a first one accepts TCP
and never answers the handshake (20 s timeout) and checks the healthy host's connect and inbox are
unaffected: each host is its own driver thread, and the Android flows are per host.

### Battery exemption up front

*Superseded by [Permissions: none on connect](#permissions-none-on-connect): the explanation is now the
last step of adding a host, never part of a connect, and the saved host ids (`pending_battery`,
`pending_connect`) and `restoreStage()` are gone. The Home card below is unchanged. The text is kept as
the record of the M3 polish.*

OxygenOS lets the SSH connections die within about ten minutes in the background unless the app is
exempt from battery optimisation (mosh survives; with the exemption SSH does too). The explanation
used to appear as a modal dialog over the terminal on the first return from the background. Then:

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
did: `workstation.local:22 \u00b7 name not resolved (mDNS) after 3 tries` and `198.51.100.20:22 \u00b7 no answer
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
- The cold-launch resume, the battery request (and OxygenOS's own dialog; now the last step of adding a
  host), and the Home card have run only on fakes and compile-checked device tests (`HomeUiDeviceTest`); the biometric prompt
  at a cold start is the thing to watch.
- The per-address timeout and the `.local` retry are tested against scripted resolvers and a
  blackholing transport; the real Android resolver (and a first mDNS lookup that fails after 1.5 s) is
  the phone's to confirm.
- `tests/mosh_live.rs::terminate_stops_a_server_nobody_connected_to` failed once in one full workspace
  run (a UDP-port-to-pid lookup of the test's own helper) and passed on every rerun, alone and in the
  suite; it does not touch this change and is recorded here rather than hidden.

# Easy pair (QR onboarding)

Pairing a new host takes a code from the phone typed into one command on the host, then one scan,
with no flags, without weakening M1's trust model. Manual host entry stays available. The user guide
is [Pair a host](pairing.md).

**Pairing needs exactly the reachability SSH needs, and nothing else.** It runs over the host's own
sshd, on the port or2 uses afterwards. This is version 2 (pairing code `or2-pair:2`, FFI API 13). It
replaces version 1 (FFI API 12), which ran a one-shot TCP listener on a random port: that extra port
failed wherever SSH worked but the random port did not (a rented server reached over its public
address, a provider or cloud firewall that opens only 22, corporate Wi-Fi, an IPv6-only host, the
macOS "accept incoming connections?" dialog). Version 1's listener, its bind policy, `--bind`,
`--pair-port`, the HMAC exchange, the `y` confirmation and their tests are removed, not kept beside
version 2.

## The flow

1. **Phone:** Add host → **Easy pair**. The screen shows a **pairing code** `K` such as
   `7KQ4-M2XD-9PTM`, with the camera below it.
2. **Host:** `or2-pair` runs its checks, then asks **Code shown on your phone** (see "Host CLI", "Output"). The
   person types `K`.
3. **Host:** derives a throwaway Ed25519 **bootstrap key** from `K` and a fresh pairing id, adds it to
   `~/.ssh/authorized_keys` restricted to one forced command (`or2-pair enroll <id>`) with an expiry
   (OpenSSH 9.1 or newer),
   prints the QR (addresses, SSH port, host key, pairing id: **nothing secret**) and waits.
4. **Phone:** scans, shows the review, **Pair**. It derives the same bootstrap key from `K` and the id,
   logs in on the SSH port (host key pinned from the QR) and sends its own public key.
5. **Host (the forced command):** replaces the bootstrap line with the phone's key in one locked, crash-safe write
   and answers. The waiting `or2-pair` sees the result and prints it.
6. **Phone:** saves the host with its trusted host key and connects with its own key (no first-use
   prompt, whatever other host keys the host has: the connection asks for the trusted key's algorithm
   first, M1 "Host-key trust").

**Why the secret goes from the phone to the host.** The QR is the part most likely to leak (a screen
share, a recording, a screenshot, terminal scrollback, someone behind you). In version 2 the QR is
public data; the one secret is `K`, shown on the phone and typed at the host by the person running
`or2-pair`, never printed by the host. Typing it is the approval: it replaces version 1's
`Authorize this key? [y/N]` and its fingerprint comparison, which people skip, and a pipe cannot type
it. The honest limit: `K` is one secret. Someone who reads it off the phone and can reach the host's
SSH port could enrol a key within the window; afterwards it is worthless, because the bootstrap entry is
gone.

## The pairing code `K` (phone)

- 12 characters of Crockford base32 **without `Z`** (the 31 symbols `0-9`, `A-Y` without `I`, `L`, `O`,
  `U`), shown as three groups of four (`7KQ4-M2XD-9PTM`): 11 random data characters from the OS CSPRNG,
  each uniform over the 31 symbols (11·log₂ 31 ≈ 54.5 bits), and a check character,
  `c = (Σ i·vᵢ for i = 1..11) mod 31`, where `vᵢ` is the value of the i-th character (its index in
  `0123456789ABCDEFGHJKMNPQRSTVWXY`: `0`-`9` are 0-9, `A` is 10, `B` 11, `C` 12, `D` 13, `E` 14, `F` 15,
  `G` 16, `H` 17, `J` 18, `K` 19, `M` 20, `N` 21, `P` 22, `Q` 23, `R` 24, `S` 25, `T` 26, `V` 27, `W` 28,
  `X` 29, `Y` 30), and `c` (0 to 30) is the character with value `c`. Every value is below the prime 31,
  so the 31 characters are distinct modulo 31 and weights 1 to 11 catch every single wrong character and
  every swap of two different neighbours, without exception, and `c` is never `Z`: `Z` never appears in a
  code. (Before this change `Z` was a data character with value 31, which is 0 modulo 31, so a `0` typed
  for a `Z` passed the check; review of the v2 integration.) Both crates test these vectors (data
  characters → check character): `7KQ4M2XD9PT` → `M` (1136 = 36·31 + 20; so the code is
  `7KQ4-M2XD-9PTM`), `00000000000` → `0`, `YYYYYYYYYYY` → `V` (30·66 = 1980 = 63·31 + 27),
  `11111111111` → `4`, `0123456789A` → `6`.
- **Drawing.** Each data character comes from one random byte: a byte below 248 (8·31) gives the value
  `byte mod 31`, so every symbol is equally likely; a byte of 248 or more is dropped and another byte is
  drawn (rejection sampling, never folding). The phone (`PairCode`) and the host's own generator (used by
  its tests) draw the same way.
- Typed input is read leniently: case-insensitive, hyphens and white space ignored, `I`/`L` read as
  `1`, `O` as `0`. What is left must be 12 characters (checked first: a wrong length is a length
  error whatever the characters), then each must be a code character: `Z` (like `U` or punctuation) is
  a character codes never use, refused as invalid (the host re-prompts as for any typo; the phone's
  `PairCode::parse_typed` returns `Character`; both messages say codes use "0-9 and A-Y, never U or
  Z"), never read as another character; then the check. A failed check re-prompts on the host ("That
  code has a typo") without spending anything. On the host the typed characters are normalized into a
  zeroizing buffer, wiped on every way out (a refused character included).
- **One reading on both sides.** The host's `code::PairCode::parse` and the phone's
  `PairCode::parse_typed` give the same answer (the same code, or the same one of length, character
  and check) for every input; `or2-pair`'s `tests/code_agreement.rs` holds them to it (the five
  vectors and their bootstrap keys, every character up to U+024F and a few others in every position of
  every vector, a table of lenient and wrong inputs, every code the host's drawing makes from each
  byte value, the refused-character message). (Integration of the v2 review fixes: the phone ignored
  only spaces and hyphens, not other white space, and refused a character before it counted the
  length.)
- Generated in Rust (`PairCode`, zeroized on drop). A new `K` is drawn each time the Easy pair screen
  opens and after every pairing that reached the host, successful or not. It is never logged or saved.

## The QR code

`or2-pair:2?name=<label>&user=<u>&port=<p>&a=<addr>…&hk=<algo> <base64>&id=<pairing id>`

- Encoding and limits as in version 1: values percent-encoded (`A-Za-z0-9-._~` and `:` stay, everything
  else `%XX`; `+` is a plus); at most 1024 bytes and at most eight `a` addresses, the CLI dropping the
  last (lowest priority) until both hold and saying so; the CLI checks the whole code with the phone's
  rules (`Payload::validate`) before drawing it and refuses with the field's name; tests hold the two
  parsers in step. `name` and `user`: 1 to 64 characters, no control characters.
- `a`: one to eight addresses (names or IP literals, no duplicates; IPv6 literals without brackets), all
  at the one SSH `port`, in the order the phone tries them.
- `hk`: one plain host public key (`ssh-ed25519`, `ecdsa-sha2-nistp256/384/521`, `ssh-rsa`), no comment.
  It is pinned: the phone trusts it from the scan and accepts no other key.
- `id`: the pairing id, 8 random bytes from the OS CSPRNG as exactly 13 lowercase RFC 4648 base32
  characters, no padding. Not secret.
- **`--manual` (alias `--no-listen`) omits `id`**, asks for no code and changes nothing on the host. The
  phone then saves the host with its key trusted and shows its public key line to install by hand.
- The parser is strict (unknown fields, repeated single fields, a bad escape, a malformed `id` are typed
  errors) and accepts only version 2. Any other version is `UnsupportedVersion { version }`; for
  version 1 the phone says "This code is from an older or2-pair: update it on the host", for a higher
  one "Update or2 to use this code".

## The bootstrap key

`seed = HKDF-SHA256(ikm = the 11 data characters of K as ASCII uppercase, salt = id as its 13 ASCII
characters, info = "or2-pair/2 bootstrap ed25519")`, 32 bytes, is the Ed25519 secret key (RFC 8032
seed). The host needs only its public half; the phone uses it for one login and zeroizes it. Both crates
test one fixed vector (`K`, `id` → seed → public key) computed with an independent implementation, not
with or2's own code. Salting with the id binds the bootstrap key to one run: a `K` typed into two runs
yields two unrelated keys.

**The vector** (computed with the OpenSSL CLI; the host crate's `bootstrap` tests and the phone's
`or2_core::pair` tests hold exactly these constants):

```text
K data characters   7KQ4M2XD9PT          (the code 7KQ4-M2XD-9PTM)
pairing id          abcdefghijklm
seed                734c24be4849a8de10227c52bd2530221cd6d2e62062c650886f0db0b99aeed0
public key          02d8bd7aee56213d1bcdd3f38649a9b749904226955d0d460f400ba630e0d628
OpenSSH line        ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo
fingerprint         SHA256:SQBrFEwlegpfzBKzGHE7T/RYggIFZfhD6+32GxJok20   (ssh-keygen -l)
```

```sh
openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt key:7KQ4M2XD9PT -kdfopt salt:abcdefghijklm \
    -kdfopt "info:or2-pair/2 bootstrap ed25519" -kdfopt mode:EXTRACT_AND_EXPAND -binary HKDF | xxd -p -c64
# the public key of that seed: PKCS#8 DER = 302e020100300506032b657004220420 || seed
openssl pkey -inform DER -in seed.der -pubout -outform DER | tail -c 32 | xxd -p -c64
```

The OpenSSH line is the key's wire form (`0000000b "ssh-ed25519" 00000020 <public key>`) in base64. A phone's
test client that derives the same bytes by an independent route (its own HKDF and Ed25519 library)
confirms both ends; the host crate also checks them against a real `sshd` (`tests/sshd.rs`).

**Why about 54.5 bits is enough.** Recovering `K` offline needs the bootstrap public key, a signature made with
it, or the fingerprint sshd logs. The phone authenticates only after the handshake presented the pinned
`hk`, so a man in the middle sees none of them, and it cannot relay the phone's signature (it covers the
session id). Only the account itself, root and readers of sshd's log see the fingerprint, and `K` is
dead once the run ends. Online guessing goes through sshd's authentication (`MaxAuthTries`,
`MaxStartups`, fail2ban) within the window.

The line `or2-pair` appends (one line, shown wrapped):

```text
restrict,command="<exe> enroll <id>",expiry-time="<YYYYMMDDHHMM>Z"
    ssh-ed25519 <base64> or2-pair-bootstrap-<id>
```

- **Options by sshd version**, read from the banner the checks already fetch:
  - OpenSSH 9.1 or newer: `restrict,command="…",expiry-time="…Z"`, the time in **UTC** with the `Z`
    suffix. **Verified** against the OpenSSH 9.1 release notes (2022-10-04): `expiry-time` dates "may be
    suffixed with a "Z" character to cause them to be interpreted in UTC"; without it they are read in
    sshd's own local time zone. With `Z`, sshd's time zone and `or2-pair`'s cannot disagree (review of
    the v2 integration: sshd under `TZ=UTC` and `or2-pair` under `TZ=Etc/GMT+5` gave a key that sshd
    took as expired five hours before it was written). The disposable test sshd (OpenSSH 10.5) honours
    it whatever its own `TZ` (`tests/sshd.rs`: a host in another time zone than sshd pairs; a `Z` time
    in the past is refused, one in the future accepted, with sshd five hours ahead of UTC).
  - OpenSSH 7.2 up to 9.0: `restrict,command="…"`, **no `expiry-time`**, and a printed note that the
    key is written without an expiry in the file (this sshd is older than 9.1, the first that reads an
    expiry time in UTC): `or2-pair` removes it when it ends, and the forced command still stops
    answering at the deadline, and as soon as the run is gone. `restrict` is from 7.2 (2016-02-29).
    7.7 (2018-04-02) added `expiry-time`, but up to 9.0 it is read only in sshd's own local time zone,
    which `or2-pair` cannot know for sure (its `TZ` may not be sshd's, nor its service's zone), so none
    is written: the run's lock on its state file and the deadline are the boundary, the expiry is
    hygiene. (Fix check of the v2 fixes: version 2 as first fixed wrote the host's local time for 7.7
    up to 9.0 unless `TZ` was set, which still guessed at sshd's zone.)
  - Older OpenSSH: `command="…",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-user-rc`,
    same note.
  - Not OpenSSH, or no readable banner: refuse automatic pairing and point to `--manual`. An option
    sshd does not know makes it ignore the whole line, so nothing is emitted blind.
- `expiry-time` (9.1 or newer only): the start of the run plus the window plus 10 minutes, in UTC with
  `Z`.
- `<exe>`: `std::env::current_exe()`, canonicalized (an absolute path: non-interactive shells often lack
  `/opt/homebrew/bin` or `~/.cargo/bin` in `PATH`). sshd runs the command through the account's login
  shell, so the path must consist only of `A-Za-z0-9/._+-`; anything else is refused before anything is
  written, naming the character and suggesting an install location (`~/.local/bin`, `~/.cargo/bin`).
  That set and the id's need no quoting in sh, bash, zsh or fish.
- A login shell that cannot run a command (`nologin`, `false`, from the account database) is refused,
  and so is one that does not really start this program: the checks run `<shell> -c "<exe> --version"`
  (the account's shell, `/bin/sh` when the field is empty) with a 5 s limit, and anything but exit 0
  with a line `or2-pair <this version>` in its output (rc-file noise around it is fine) is a `fail`
  naming the shell, the command and why (the exit status, the time limit, the missing line). The limit
  covers the output too: the shell runs in a process group of its own and its output is read inside
  the same 5 s; at the limit the group is killed and what was read is used, so a job an rc file starts
  in the background that keeps the output open does not hold the check (a shell that exited 0 with the
  line passes). (Fix check of the v2 fixes: the output was read to its end after the shell exited.)
  That group does not get the terminal's Ctrl-C, and the run's own handlers are not armed yet, so while
  the shell runs SIGINT, SIGTERM, SIGHUP and SIGQUIT (those not ignored) go to a handler that kills the
  group with SIGKILL and then ends `or2-pair` with the signal's default action, as before; a signal
  that arrives while the shell is being started is acted on as soon as its group is known, and the
  previous actions come back when the check ends. (Fix check of the v2 fixes, round 2: Ctrl-C during
  the check ended `or2-pair` and left the shell running.)
- **The lock.** Every change of `authorized_keys` and of a run's state is made while holding an
  exclusive `flock` on `~/.ssh/or2-pair/lock` (a stable file, created 0600 and never removed, opened with
  the checked-handle rules: no link followed, regular, owner, one name, StrictModes). Its mode is 0600
  whatever the umask: a new one is set to 0600 (`fchmod`), and so is any regular file of the account's
  own with one name and another mode, before the StrictModes check: one the account cannot open (it is
  checked by name first, then `fchmodat`) and one that group or others may write (`0666`, `0620`,
  `0060`; it holds nothing, and `~/.ssh/or2-pair` is the account's, 0700). What cannot be repaired is
  refused with a message about the lock file, not about `authorized_keys`, that says what it is and to
  remove it: a symbolic link, a directory, anything else that is not a regular file (a FIFO), a file of
  another account, a file with another hard link; its mode is left as it is. (Fix check of the v2
  fixes: under umask 0777 it was created 000, and every later lock failed; round 2: a lock writable by
  group or others was refused with the StrictModes message for `authorized_keys`.) One lock for
  everything (the bootstrap append, `enroll`'s commit, the foreground's cleanup, the sweep), so no two of
  them interleave. A lock that is taken is tried again every 50 ms: `or2-pair` waits up to 3 s (a second
  signal during a cleanup ends the waiting after one more try), `enroll` up to 2 s. (Version 2 as first
  integrated locked `authorized_keys` itself; that lock could not cover `enroll`'s commit and the
  cleanup together, and a file that is replaced has no stable inode to lock.)
- **Writing.** Through the checked handles (below), under the lock: the state file first (a state file
  without a key is harmless), then one backup per run before the first change of `authorized_keys`, then
  the append. If the append fails the state file is removed again.
- **Replacing the file, crash-safely.** Every change of `authorized_keys` (the append, a removal,
  `enroll`'s replacement, the sweep) re-reads the file and installs the whole new contents as a **new
  file**: created in the same directory (`~/.ssh`) relative to its handle with `O_CREAT|O_EXCL`, 0600, a
  unique name (`.authorized_keys.or2-tmp-<pid>-<n>-<ns>`), fully written and `fsync`ed, given the old
  file's mode (and group, where the account may) and on Linux its `security.selinux` extended attribute
  when it has one and the new file's differs (while SELinux is active, a label that cannot be set
  refuses the change: sshd might not be allowed to read the file; without SELinux the attribute means
  nothing and only a privileged process may set it, so a stale one is not copied), and its POSIX access
  ACL (`system.posix_acl_access`) when it has one (an ACL that cannot be set always refuses the change:
  whatever it let read the file could not any more). A label or ACL that cannot be set is reported in
  words (what could not be copied, that nothing was changed, and the fix: `restorecon -v <file>`,
  `setfacl -b <file>`, or `--manual`), not as a bare error number, and the old file stays as it was; then the target is
  checked again (the name is still the file that was read, same device and inode, not a link, regular,
  owner, one hard link, StrictModes), so a hard link added or a file swapped in meanwhile is refused
  with nothing changed; then `renameat` over it and an `fsync` of `~/.ssh`. A
  missing file is created by linking the new one to its name, which fails if the name appeared
  meanwhile (the temporary name is then removed). A crash, a kill or a full disk leaves the old file or the new one, never a mix; a failure
  removes the temporary file. Between the second check and the rename a process of the same account can
  still swap the name (it could edit the file anyway). The backup rule is unchanged (one per run, before
  its first change). (Review of the v2 integration: the in-place rewrite, `ftruncate` and write on the
  same descriptor, could leave new and old bytes mixed, and rewrote a hard link added after the check.)
- **A failure after the rename is not "nothing changed".** Once the new file has the name (the
  `renameat`, or the link of a created file) the change is made. A step that fails after that (the
  `fsync` of `~/.ssh`, removing the temporary name of a created file) is a **warning**, not an error:
  "`<file>` was changed, but `<dir>` could not be synced to disk (…), so a crash before the system
  writes it may undo the change" (or that the temporary name is left for the next run). The change
  stands and is reported as made; the warning is printed by the run that made it (a `warn` line, or
  "Warning: …" after the report), and `enroll`'s goes into its `.done` (below). (Fix check of the v2
  fixes: a failed directory sync after the rename made `enroll` take back its `.done` and tell the
  phone `failed` while the phone's key was installed and the bootstrap entry gone.)
- **Removing.** One change under the lock drops every entry whose key type and key data equal the
  bootstrap key's (any options, any comment), and reports how many it dropped.
- **Every ending removes it** (if the forced command has not already replaced it): the timeout, an
  error, Ctrl-C, SIGTERM, SIGHUP (a handler only counts; the wait loop acts on the count within 250 ms) and
  a panic (a drop guard on the live run). The handlers are installed after the code is typed and before
  anything is written, without `SA_RESTART`. The ending takes the lock (above), then decides (see
  "The state file and `or2-pair enroll`"). The report says "The temporary key was removed" only when the
  removal dropped an entry; when there was none it says the key was already gone and nothing was removed.
  That holds for every ending that removes, the recorded pairing whose key is not in the file included
  (fix check of the v2 fixes: that one always said it was removed).
  If the lock cannot be taken or the removal fails, `or2-pair` prints the exact line to delete, says that
  the next run removes it, and that a phone pairing at that moment may have finished (look for its
  line).
- **SIGKILL** (or any death without the handlers or the drop guard) removes nothing, and needs not to:
  the run's lock on its state file goes with the process, so `enroll` refuses at once and the next run
  sweeps the entry and the state (tested with SIGKILL of the built testhost).
- **Sweep.** Every run (not `--check`, which only reports them) first takes the lock and removes
  `or2-pair-bootstrap-*` entries whose run is not live, and those state files (and any state file of a
  run that is not live, a `.done` without its `.json`, and temporary files a crash left in
  `~/.ssh/or2-pair/` and next to `authorized_keys`). **Live** is: the state file is there and readable,
  names its id, its deadline has not passed, and its run still holds its lock on it. A live run of another
  terminal is left alone; a run killed with SIGKILL is not live although its deadline is ahead. The
  sweep happens after the code is typed (so an empty line still changes nothing) and shares the run's
  one backup. `--check` takes no lock and changes nothing.
- **The security boundary is the forced command, the run's lock on its state file and the deadline,
  not the cleanup.** The bootstrap key can only start `or2-pair enroll <id>`, which refuses when the
  state file is missing, not held by its run, or past its deadline, and decides under the lock. Expiry,
  removal and the sweep are hygiene.

## The state file and `or2-pair enroll`

- `~/.ssh/or2-pair/` (mode 0700) holds the lock file `lock` and `<id>.json` (mode 0600) for each live
  run: the id, the deadline (Unix seconds), the account's uid, the bootstrap key's fingerprint. All are
  opened relative to the `~/.ssh` handle with the same checks as `authorized_keys` (`O_NOFOLLOW`, owner,
  mode, regular file, one name).
- **Published complete, once.** `<id>.json` (and `<id>.done`) is written under a temporary name in the
  directory (`tmp.<id>.<ext>.<pid>-<n>-<ns>`, `O_CREAT|O_EXCL`, 0600), `fsync`ed, then linked to its name
  (`linkat`, which fails if the name exists: an id is used once), the temporary name removed and the
  directory `fsync`ed, all under the lock. No reader ever sees a partial file (review of the v2
  integration: the name existed, empty, while it was written, and another run's sweep took it for dead
  and removed it).
- **Held for the run's life.** The foreground takes an exclusive `flock` on its state file before it
  gets its name and keeps that descriptor open until the run ends. A state file whose lock can be taken
  (a shared non-blocking `flock` succeeds) belongs to a run that is gone, however it ended, SIGKILL
  included: `enroll` refuses it (`expired`) and the sweep removes it with its bootstrap entry.
- `or2-pair enroll <id>` is an internal subcommand (listed in `--help` under "Internal"; exactly that
  argument, the id validated before use). It runs under sshd with the phone's exec channel as stdin and
  stdout and no terminal:
  1. Writes the hello (it needs only the id, which is its argument) and reads one bounded request line
     (10 s). **Order, decided in implementation:** every refusal below, `expired` included, comes after
     the request has been read, so the phone always receives a verdict; a phone that has sent its
     request and finds the channel closed could not tell `expired` from a lost connection. A request
     that is not one line of at most 2048 bytes, not JSON, or not `"v":2` is `request`.
  2. Reads `<id>.json`; refuses (`expired`) if it is missing or unreadable, not held by its run (see
     above), past the deadline (checked with `now <= deadline`), names another uid or another id.
  3. Validates the key with `keyline.rs`; `key` when it is not a key this tool authorizes, **or when it
     is the bootstrap key itself** (that would make the temporary key permanent). The device label is
     reduced to ASCII as in version 1 (`phone` when nothing is left).
  4. Takes the lock (2 s, then `failed`), so a second phone waits for the first one's commit and then
     sees `gone` rather than a lock error. **Under the lock, as one transaction:**
     1. Checks the state again exactly as in step 2 (`expired` otherwise) and that it is the state of
        step 2: a cleanup or a sweep that ran in between has removed it, and this `enroll` refuses.
     2. Reads `authorized_keys` on the checked handles and finds the bootstrap entry by the
        **fingerprint in the state file** (every entry whose parsed key has that fingerprint, any
        options, any comment); if it is gone, answers `gone`. The new contents: without it, plus the
        phone's line (`no-agent-forwarding,no-X11-forwarding <key> or2-<device>-<UTC date>`, as in
        version 1; a key already present only loses the bootstrap entry).
     3. Publishes `<id>.done` (complete, created once, 0600: device label, key fingerprint). If that
        fails, nothing else is done: `failed`.
     4. Installs the new `authorized_keys` (one crash-safe replacement). If that fails, `<id>.done` is
        removed again: `failed`. Either both happened or neither (a crash between the two leaves a
        `.done` without the key, which the foreground does not report as a pairing, below). A failure
        after the new file has its name is not a failure of the replacement (see "A failure after the
        rename"): the key is installed, so `<id>.done` stays, the phone is told `ok`, and the warning
        is added to the record (`<id>.done` published again with it, renamed over the old one; best
        effort, the record as first written stays right without it).
     5. Lets the lock go.
  5. Answers the phone `ok`; exits 0, or 1 on a refusal. (Review of the v2 integration: `enroll`
     checked the state without the lock, and wrote `.done` after the key and ignored its failure, so a
     cleanup could report "Cancelled" while the key was installed, and the phone could be told `ok`
     while the host timed out.)
- The foreground run polls for `<id>.done` every 250 ms (a local file, no network, no lock), and ends on
  it, on the timeout (the window on the monotonic clock, **or the wall clock past the deadline**, which
  is what a machine that slept through the window sees first), or on a signal. **Every ending takes the
  lock** (that waits for an `enroll` in its transaction) and then decides:
  - `<id>.done` is there (on any ending: an `enroll` that committed just before is reported as a
    pairing, not a timeout or a cancel): it reads it and checks that `authorized_keys` has an entry
    with the phone's key fingerprint. Present: it prints `Paired …` (and the record's warning, if it
    has one, as "Warning: …"). Not present (or the record unreadable): it prints that the phone was
    recorded as paired but its key is not in the file, that nothing was paired, what became of the
    temporary key (removed, already gone, or the line to delete when it could not be removed), and
    exits 1. Either way it removes the bootstrap entry if it is still there, and the state files.
  - `<id>.done` is not there after a `.done` was seen (the `enroll` took it back): the run goes on
    waiting.
  - Otherwise (timeout, signal): it removes the state file **first** (that closes the boundary), then
    the bootstrap entry, then what is left; an `enroll` that takes the lock afterwards finds no state and
    refuses. The run's lock on the state file goes with it.
- The state file's `id`, `deadline` (Unix seconds, the start plus 5 minutes), `uid` and `fingerprint`
  (`SHA256:…` of the bootstrap key) are JSON; `<id>.done` is `{"device":…,"fingerprint":…}`, plus
  `"warning":…` when something failed after the key file took its new contents: control characters
  replaced by spaces, and cut at a character boundary so that its JSON form (escapes included,
  without the quotes) is at most 1024 bytes. Both are read with a 4 KiB limit, which a rewritten
  `.done` therefore always fits. (Fix check of the v2 fixes, round 2: the warning was cut to 1024
  characters, and a control character is six bytes of JSON.)

## The exchange

Newline-delimited JSON over the SSH session channel between the phone and `or2-pair enroll`:

```text
host  -> {"v":2,"hello":"or2-pair","id":"<id>"}
phone -> {"v":2,"key":"<algo> <base64>","device":"<label>"}
host  -> {"v":2,"ok":true,"user":"<account>","fingerprint":"SHA256:…"}
       | {"v":2,"ok":false,"reason":"expired|gone|key|failed|request"}
```

- **No MACs.** SSH authenticates both ends (the host by the pinned `hk`, the phone by the bootstrap key
  only a holder of `K` can derive) and encrypts the channel.
- The phone checks that the hello's `id` is the scanned one (`Protocol` otherwise).
- **Bounds.** The hello 256 bytes, the request 2048, the verdict 512.
- **Timing.** The host's window is **5 minutes** from the moment the bootstrap entry is written; the
  waiting line shows the deadline. The phone gives each connect, the SSH handshake, the authentication,
  the hello and the verdict 10 s.
- **Shell noise.** sshd runs the forced command through the login shell, and some rc files print. The
  phone skips any bytes before the first line that starts with `{"v":2,"hello"`, up to 4 KB; more is
  `NotOr2Pair`.
- **Two phones.** Whichever `enroll` takes the lock first wins; the other answers `gone` ("Another device
  already used this pairing").

## The phone's connection

- Through `Transport`: the `a` addresses at `port` are raced with the existing TCP race, and **one** SSH
  handshake runs on the winner (never one per address: fail2ban and `MaxStartups`).
- The handshake accepts only `hk`, with `hk`'s algorithm as the preferred host-key algorithm (a host with
  several keys must present the pinned one). Any other key is `HostKeyMismatch`: the connection ends
  before authentication and nothing is sent or saved.
- Authentication offers only the bootstrap key, once (no stored key, no password, no
  keyboard-interactive). A refusal is `BootstrapRefused`: the code typed on the host was different, the
  run ended or expired, or sshd ignores `~/.ssh/authorized_keys`.
- One session channel, no PTY, `exec "or2-pair"` (the forced command runs whatever is asked). A first line
  that is not the hello within the noise budget is `NotOr2Pair` (a `ForceCommand` in `sshd_config`, or
  another program answered).
- The derived key is zeroized once authentication completes. The pairing connection is short-lived and
  closes its channel and connection through the connection-owned close path when the exchange ends or
  the coroutine is cancelled.

**As implemented** (`or2_core::pair`, with the SSH side in `ssh/pair_client.rs` on the host
connection's `Client`, `relay`, `authenticate` and `SshHost` open and close paths). Decisions where
the text above left room:

- **Host key.** The handshake offers only the algorithm of `hk` (for RSA, `rsa-sha2-512`,
  `rsa-sha2-256` and `ssh-rsa`). A host with several keys therefore presents the pinned one, a host
  with no key of that algorithm fails the handshake, and both are `HostKeyMismatch`, as is any other
  key of that algorithm. Nothing is authenticated or sent in those cases.
- **Noise.** A line is the hello if it starts with `{"v":2,"hello"` at the start of a line, and it
  must start within the first 4096 bytes of output (so at most 4096 bytes of noise, counting the
  newlines); the hello line itself is at most 256 bytes (more is `Protocol`). Only standard output is
  read; standard error is ignored. A hello must be exactly `v` 2, `hello` `or2-pair` and the scanned
  `id`, else `Protocol`. An unfinished line is waited for (within the 10 s step), not guessed at.
- **Channel ends.** The command ending (EOF or close of the channel while the connection is up)
  before the hello is `NotOr2Pair`, and before the verdict `ConnectionLost`; a refused `exec` is
  `NotOr2Pair`; a refused session channel (`MaxSessions`) is `Protocol`; a connection that ends under
  any step is `ConnectionLost`. A step that exceeds its 10 s is `TimedOut`, including the hello, the
  verdict, the handshake and the authentication.
- **Verdict.** `ok: true` needs a non-empty `user` and a `fingerprint` equal to the `SHA256:`
  fingerprint of the key the phone sent, else `Protocol` (the host installed something else). `ok: false`
  maps `expired`, `gone`, `key` and `failed` to their errors, and `request`, no reason or any other
  reason to `Refused`. A line over 512 bytes or JSON that does not parse is `Protocol`.
- **Cancellation.** Dropping the future (cancelling the coroutine) hands the channel and the connection
  to a task on the network runtime that closes the channel through the connection's close path,
  disconnects and stops the relay, so the command's standard input ends at once.
- **FFI.** `pair_enroll` rebuilds the core offer from the `PairOffer` record: an address or a
  `host_key.openssh` that does not parse, or a malformed `pairing_id`, is `InvalidOffer`. `K` is a
  `PairCode` object, never a string, on the way in.
- **Tests** do not use a paused clock: the pairing connection's channel opens run on the process-wide
  network runtime, so an auto-advancing clock on the caller's runtime would fire the 10 s step while
  that runtime does its work. The timeout tests use a 400 to 700 ms step instead
  (`PairTiming { step }`; the FFI always uses 10 s).

## Host CLI

Flags kept: `--name`, `--user` (must be the account the process runs as, as in version 1),
`--ssh-port`, `--address` (repeatable; prepended), `--check`, `--ascii`, `--invert`, `--no-color`,
`-h`, `-V`. New: `--manual` (`--no-listen` stays as an alias). Removed: `--bind` and `--pair-port`,
refused with "pairing uses the SSH port now; these options are gone".

**The code prompt** is asked only when standard input is a terminal; otherwise the CLI refuses to start
(scripts and pipes cannot pair), except `--manual` and `--check`. It reads one line, checks the check
character (re-prompting on a typo, a wrong length or a character codes never use; after 10 mistakes the
run ends with nothing changed), and an empty line, the end of input or Ctrl-C ends the run with nothing
changed. It is asked **after** every check that can refuse (so nobody types a code for a run that cannot
start) and **before** anything is written. `K` is not echoed, written to a file or logged: while it is
typed the terminal's echo is off (`termios`: `ECHO` and `ECHONL` cleared on descriptor 0, like a
password prompt), and `or2-pair` draws the answer itself after the line is read (the code masked: see "Output"). The terminal's
settings are put back on every way out: after the line, on an error or a panic (a drop guard), and on
SIGINT, SIGTERM, SIGHUP, SIGQUIT and SIGTSTP during the prompt (handlers installed only for the prompt,
not for signals that were ignored, restore the settings with `tcsetattr`, then re-raise the signal with
its default action; the previous handlers come back with the guard). SIGTSTP (Ctrl-Z) only stops the
process, and the prompt is still waiting when it goes on: a SIGCONT handler, installed for the guard's
life (even when SIGCONT was ignored), switches echo off again (the terminal's settings at that moment
without `ECHO` and `ECHONL`) and handles SIGTSTP again, so the rest of the code is not echoed either.
(Fix check of the v2 fixes: after Ctrl-Z and `fg` the rest of the code was echoed.) The guard's
teardown is one protected sequence: it blocks the six handled signals in its thread
(`pthread_sigmask`), tells the SIGCONT handler to leave echo alone, puts the settings back, only then
tells the ending handlers there is nothing left to restore, puts the previous handlers back, and
restores the signal mask, so a signal that arrived meanwhile is delivered to the previous handlers
(the default action, normally) after echo is back; one that another thread takes meanwhile still
restores the settings itself. (Fix check of the v2 fixes, round 2: the guard told its handlers there
was no terminal before restoring it, and an ending signal in between left echo off.) Something that is not a
terminal is left alone. (Review of the v2 integration: echo was left on, so `K` reached scrollback and
session recorders.) It is read from descriptor 0 a byte at a time into a zeroizing buffer, and
`PairCode` is zeroized on drop.

**Addresses.** Every non-virtual unicast address is listed, now including global IPv6 (not link-local),
plus the mDNS name: overlay (`zt*`, `tailscale*`, `ZeroTier*`, `utun*` in 100.64/10, `feth*`), then LAN,
then public IPv4, then public IPv6, then `<name>.local`; `--address` entries first. Container and VM
bridges are left out as before. Nothing is bound, so a misclassified interface only changes the order.
Unique-local IPv6 (`fc00::/7`) counts as LAN; loopback, link-local, site-local, multicast, wildcard and
IPv4-mapped addresses are skipped; an IPv6 address on an overlay-named interface is an overlay address.

**Checks** add: the sshd version from the banner; a best-effort read of `/etc/ssh/sshd_config` and the
files it `Include`s when readable (below); the login shell (it must start this program: see "The
bootstrap key"); the executable path's characters; leftover bootstrap entries (reported by `--check`,
removed by a run). Each line is `ok` (✔), `info` (●), `warn` (▲) or `fail` (■). A **`fail`** is something that makes
automatic pairing impossible here (sshd not answering or not OpenSSH, `authorized_keys` unwritable or
refused by StrictModes, `~/.ssh` not writable (the replacement is a new file there), a login shell that
cannot run commands or does not start this program, a program path sshd's shell would mangle, and what
`sshd_config` certainly does for this account, below): the run prints the checks and ends with
"automatic pairing is not possible here: the failed checks above say why" and "fix them, or run or2-pair
--manual and add the phone's key by hand", before asking for the code and before changing anything. With `--manual` (or where no keys
are installed) the same findings are `warn`. `--check` prints them either way and exits 0. An sshd
older than OpenSSH 9.1 is a `warn` that says the key is written without an expiry.

**Fixes for this host** (`hints.rs`). A missing or failing prerequisite prints the exact command for this
host; `or2-pair` only prints it, never runs it, and never runs `sudo` (commands are shown without `sudo`
when it runs as root, and Homebrew's never have it). What the host has is read once, from files only,
under a root directory that tests replace with a fake tree (`HostFacts::detect`): the package manager
(macOS: `brew` in `/opt/homebrew/bin` or `/usr/local/bin`; Linux, the first of `apt-get`, `dnf`, `yum`,
`zypper`, `pacman`, `apk` on `PATH` or in the usual `bin`/`sbin` directories), the service manager
(systemd when `/run/systemd/system` is a directory, OpenRC when `/run/openrc` is), the SSH server's unit
(`ssh.service`, Debian's, wins over `sshd.service` in `/usr/lib/systemd/system`, `/lib/systemd/system`,
`/etc/systemd/system`), whether an `sshd` program is in the usual directories, whether the package database
lists the OpenSSH server (dpkg's `var/lib/dpkg/info/openssh-server.list`, pacman's `var/lib/pacman/local/openssh-<version>`,
apk's `P:openssh-server` in `lib/apk/db/installed`; the RPM database is not read), NixOS (`/etc/NIXOS` or
`ID=nixos` in `os-release`) or Guix System (`ID=guix`), and the active firewall (ufw when
`/etc/ufw/ufw.conf` says `ENABLED=yes`, else firewalld, else nftables when its service is enabled:
systemd's `multi-user.target.wants` link or an OpenRC `default`/`boot` runlevel entry). **The one exception to
"files only"** (v0.1.1): the macOS application firewall has no file to read, so on macOS `HostFacts::detect` runs
its read-only queries `socketfilterfw --getglobalstate`, `--getblockall` and `--getappblocked <mosh-server>`
(never `sudo`, never a `--set…`), through a `Commands` seam that tests replace with captured outputs; see
"or2-pair on macOS: the firewall". Nothing is run on any other system.
- sshd not answering: macOS, Remote Login (System Settings > General > Sharing > Remote Login) or
  `sudo systemsetup -setremotelogin on`, noting it needs Full Disk Access for the terminal app; Linux with
  systemd, `sudo systemctl enable --now <unit>` with the unit found (else the package manager's: `ssh` for apt,
  `sshd` for the others; else `sshd`, naming `ssh` for Debian and Ubuntu); OpenRC, `sudo rc-update add sshd &&
  sudo rc-service sshd start`; any other or unknown init (runit, s6, a container, WSL without systemd), "start
  sshd with this host's service manager", never a guessed `systemctl` (external review of the installer lane).
  When no `sshd` is found the install command of the OpenSSH server (`openssh-server`; `openssh` for pacman and
  apk) comes first: "is not installed" only when the package database agrees, else "does not seem to be
  installed (no sshd in the usual directories); if it is not, install it with …". NixOS gets
  `services.openssh.enable = true;` and `nixos-rebuild switch`, Guix System `(service openssh-service-type)` and
  `guix system reconfigure`, never a package manager. Every sshd hint, on every system, ends "; if sshd listens
  on another port, pass --ssh-port".
- tmux or mosh-server not found (`info`): `install it:` and the package manager's command (`brew
  install`, `apt install`, `dnf install`, `yum install`, `pacman -S`, `zypper install`, `apk add`; the
  package `tmux`, or `mosh` for mosh-server); without a package manager, Homebrew on macOS
  (`https://brew.sh`) or "with your package manager" on Linux. herdr: "see herdr's install docs
  (https://github.com/herdrdev/herdr)", whatever the package manager.
- mosh's UDP ports 60000-61000 (`info`, only when mosh-server is found): ufw `ufw allow 60000:61000/udp`;
  firewalld `firewall-cmd --permanent --add-port=60000-61000/udp && firewall-cmd --reload`; nftables an
  `nft add rule inet filter input udp dport 60000-61000 accept` example to adapt to the ruleset and keep in
  `/etc/nftables.conf`; none found: a firewall in the way would be another one (on this host, a router's or a
  cloud provider's), to open them there. macOS: what `socketfilterfw` says of mosh-server (v0.1.1, below);
  when it cannot be asked or read, allow mosh-server in System Settings > Network > Firewall.

**The `sshd_config` reading.** `Include` patterns are relative to `/etc/ssh` (a `*`/`?` in the file
name, at most four levels deep, each file once); an `Include` inside a `Match` block belongs to that
block, and a `Match` inside an included file ends with that file. The first value of a keyword wins, as
in sshd, and the first `Match` block that applies and sets a keyword overrides the global value; `Port`
(outside `Match`) is the default for `--ssh-port`. A `Match` block **applies** to this account when its
criteria are `all`, or only `User` pattern lists that match the login (sshd's patterns: `*`, `?`,
comma-separated, a matching `!pattern` excludes); it **does not apply** when a `User` criterion does not
match (all criteria must hold); otherwise (`Group`, `Address`, `Host`, `LocalPort`, …) it **cannot be
evaluated**. A finding is **certain** when the effective value says so and no block that cannot be
evaluated sets that keyword before the deciding one and every `Include` could be read; it is
**possible** when such a block or an unreadable `Include` could change the answer. A block that cannot be
evaluated after the one that decided a keyword changes nothing (external review: a later `Match Group` block
made a decided `AuthorizedKeysFile` uncertain again). Keywords and the
enumerated values compared (`yes`/`no` of `PubkeyAuthentication`, `none` of `AuthorizedKeysCommand`
and `ForceCommand`, `all` and `User` in `Match`) are read ignoring case, as sshd reads them (`sshd -T`
reports `PubkeyAuthentication No` as `no` and `ForceCommand None` as `none`; fix check of the v2 fixes:
those two were taken for enabled and for a command).
- **`fail`** (certain, for this account): `PubkeyAuthentication no`; an `AuthorizedKeysFile` that does
  not include `.ssh/authorized_keys` (`%h`, `%u`, `%%` and `~/` expanded), unless an
  `AuthorizedKeysCommand` is also set (it might read that file itself: then `warn`, saying exactly that
  rather than blaming a `Match` block); a `ForceCommand`
  (not `none`), global or in a block that applies.
- **`warn`**, suggesting `--manual`: any of those that is only possible (the message says it is in a
  `Match` block or an `Include` that cannot be evaluated); an `AuthorizedKeysCommand` when the
  `AuthorizedKeysFile` may not include `.ssh/authorized_keys` (certainly or possibly); an
  `AuthenticationMethods` that needs more than a key.
- **No finding**: an `AuthorizedKeysCommand` next to an `AuthorizedKeysFile` that includes
  `.ssh/authorized_keys` (the default does). sshd consults the command in addition to the files, not
  instead of them, so the temporary key is found in the file. (Owner report: systemd's standard snippet
  `/usr/lib/systemd/sshd_config.d/20-systemd-userdb.conf`, `AuthorizedKeysCommand /usr/bin/userdbctl
  ssh-authorized-keys %u`, included on Arch, Fedora and others, made every such host warn "pairing would
  likely fail".)
- A block that does not apply (`Match User` for someone else) is ignored. (Review of the v2 integration:
  `Match` blocks were all ignored and these findings were all warnings, so a person typed a code for a
  run that could not work.)

**Distribution.** One release stream for the project: a pushed tag `vX.Y.Z` (the workspace version and the app's
`versionName`) makes the GitHub release of that tag (`.github/workflows/release.yml`) with five assets:
`or2-pair-<target>` for `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (static, linked with
`rust-lld`), `x86_64-apple-darwin` and `aarch64-apple-darwin` (built on a macOS runner), each the bare
executable, and `SHA256SUMS` (`sha256sum` format, by name); the notes are `docs/releases/vX.Y.Z.md`. The app's
APK is added afterwards by hand (it is signed locally). All binaries are built by `cargo xtask dist`, which strips
symbols, maps the build's paths to `/or2` and `/cargo`, and runs the binary that matches the builder (`--version`
must print `or2-pair <version>`; `--expect-version` makes the tag, the workspace version and the `versionName`
agree); the workflow checks each binary's `file` output (static ELF, or Mach-O, of its architecture). A manual
run (`workflow_dispatch`) never publishes.
`scripts/install-or2-pair.sh` (POSIX `sh`; the whole script is one `main` function called on its last line, so a
download cut short runs nothing) maps `uname -s`/`uname -m` to a target (`x86_64`/`amd64`, `aarch64`/`arm64`;
Linux and Darwin; an Intel shell under Rosetta gets the Apple silicon binary) and refuses anything else before
downloading. It calls no GitHub API: the default is `<base>/latest/download/<asset>` and
`<base>/latest/download/SHA256SUMS` (GitHub's latest release, never a draft or a prerelease), `--version vX.Y.Z`
(or `X.Y.Z`) `<base>/download/vX.Y.Z/…`, where `<base>` is `OR2_PAIR_RELEASES_BASE` (a mirror or a test fixture,
`https://` or `file://` only) or `https://github.com/code-akram/or2/releases`. It downloads with `curl` (HTTPS
only, also after redirects; a redirect away from HTTPS is reported as such) or `wget`; a missing `SHA256SUMS`
under `--version` says that version does not exist. It refuses unless `SHA256SUMS` lists the asset and its
SHA-256 (`sha256sum` or `shasum -a 256`) matches, installing nothing. The destination is `--dir`, else
`$OR2_PAIR_INSTALL_DIR`, else `~/.local/bin` (created); it is refused when `or2-pair` there is a directory, when
anyone may write it, and, as root, when it is not root's or group or others may write it (root also gets a
one-line notice that pairing from its shell pairs the root account). The binary is written to a new file that
`mktemp` creates in the destination (no existing name, link or not, is ever opened), made executable and run
(`--version` must print `or2-pair …`, and `or2-pair X.Y.Z` for `--version vX.Y.Z`), and only then renamed over
`or2-pair`; a binary that does not run here replaces nothing and the staging file is removed on every exit. It
then warns when the directory's path has characters `or2-pair` refuses, prints the `PATH` line to add when it is
not on `PATH`, and tells the person to run `or2-pair`. It never runs `sudo` and writes nothing else. The
checksum proves integrity, not authenticity: the binary and `SHA256SUMS` come from the same release (signing
is a future item).
The installer draws the same rail as `or2-pair` (see "Output" below; `printf` only, colour only when `[ -t 1 ]`
(`[ -t 2 ]` for an error), `NO_COLOR` unset or empty and `TERM` not `dumb`, the Unicode glyphs only when
`${LC_ALL:-${LC_CTYPE:-${LANG}}}` names UTF-8, else the ASCII rail), once its arguments are understood:

```text
┌  install or2-pair
│
▲  running as root: or2-pair is installed for root, and pairing from root's shell pairs the root account
│  run this as the user the phone should log in as                  (as root only)
●  Downloading the latest release for x86_64-unknown-linux-musl     (or: Downloading v0.1.1 for …)
●  Downloaded or2-pair-x86_64-unknown-linux-musl
│  from https://github.com/code-akram/or2/releases/latest/download
✔  Checksum verified
│  SHA-256 <the asset's hash>
✔  Installed or2-pair 0.1.1 at /home/dev/.local/bin/or2-pair
▲  /home/dev/.local/bin is not on your PATH                         (only then)
│  add it in your shell's startup file, for example:
│  export PATH="/home/dev/.local/bin:$PATH"
│
●  Next: open or2 on your phone (Add host > Easy pair) and run:
│  /home/dev/.local/bin/or2-pair                                    (or2-pair when on PATH)
│
└  Done
```

A refusal goes to standard error as `│`, `■  <what>` and its fix on the next lines, `│`, `└  Failed`; a usage
error (before the rail) is the usage and one plain `install-or2-pair: <message>` line. A directory whose path has
characters `or2-pair` refuses is a `▲` with the fix (`--dir` to a plain path).

**Kept from version 1 unchanged:** the account (`getpwuid_r(geteuid())`, `$HOME`/`$USER` ignored,
`--user` only repeating it); the phone's `authorized_keys` line, rebuilt from the validated key, with the
label reduced to ASCII letters, digits, `.`, `_`, `-`, at most 32 characters; duplicate detection by
parsed key; the backup naming; the checked handles (`O_NOFOLLOW`, `O_NONBLOCK` before `fstat`, owner,
regular file, one hard link, `O_CREAT|O_EXCL` 0600, 8 MiB limit), now also used for the replacement's
new file and its second check, the state directory and the lock file (the `flock` moved from
`authorized_keys` to `~/.ssh/or2-pair/lock`); StrictModes refused, not warned; non-Unix targets install
nothing (they print a `--manual` code and the manual instructions).

**`or2-pair-testhost`** (only with `test-support`, never built by `cargo install`) reads `K` from
standard input without a terminal so tests can feed it, and honours `OR2_PAIR_TEST_HOME`,
`OR2_PAIR_TEST_USER` and `OR2_PAIR_TEST_AUTHORIZED_KEYS`, which tests point a disposable sshd's
`AuthorizedKeysFile` at (the state directory `or2-pair/` then lives beside that file). Its forced command
line names the testhost binary, which also runs `enroll <id>`; the disposable sshd therefore passes the same
variables to its sessions (`SetEnv`). Two more, added in implementation because the host key and the window
cannot be injected otherwise: `OR2_PAIR_TEST_ETC_SSH` (the directory holding `ssh_host_ed25519_key.pub` and
`sshd_config`, instead of `/etc/ssh`) and `OR2_PAIR_TEST_WINDOW_SECS` (the pairing window, instead of 300).
Its human output is the normal output on standard output; the line that starts `or2-pair:2?` is the
pairing code, and `Waiting for the phone` marks the moment the temporary key is in place. The installed
`or2-pair` reads none of these variables; a `test-support` build of it reads them too.

**Output** (v0.1.1, `rail.rs`; the look of `@clack/prompts`). Everything `or2-pair` prints for a person is one
continuous **rail** in a dim left gutter, text two columns after the rail's glyph:

- `┌  or2-pair <version> · <what>` opens it (`pair a phone with this host`, `check this host for pairing`,
  `pairing code only` for `--manual`); `└  <word>` closes it with one plain word: `Paired`, `Cancelled` (an empty
  answer, the end of input, too many typos, Ctrl-C, SIGTERM or SIGHUP), `Timed out`, `Not paired` (a phone's result
  without its key in the file), `Failed` (an error, or a temporary key that could not be removed), `Done`
  (`--check`, `--manual`). A blank rail line (`│`) separates the steps.
- Each message swaps the rail glyph for a symbol: `●` info (blue), `✔` ok (green), `▲` a warning or a calm notice
  (yellow), `■` an error or a failed check (red), `◆` a question being asked (cyan), `◇` an answered one (green).
  A message's further lines (each `\n` in its text) continue the rail (`│  `): a check's **fix** is on its own
  lines under the check, a command on a line of its own between backticks (cyan with colour).
- Lines **wrap** to the terminal's width (the `TIOCGWINSZ` of the stream, else `COLUMNS`, else 80) with the rail
  continued, at spaces only: a word longer than the line (a path) and a command on a line of its own are never
  split, so they stay copyable. Off a terminal nothing is wrapped. Under 16 columns of text nothing is wrapped
  either.
- **This host** is labelled rows (labels dimmed, one per line: name, user, ssh port, host key, addresses, one
  address per line); values wrap under their first line.
- **The QR** is drawn on the rail (`│  ` before each row, nothing else changed: its 2-module quiet zone, `--ascii`
  and `--invert` as before) between blank rail lines; the rail glyph is three columns from the code, outside its
  quiet zone, and the tests decode the drawing with the rail in it (`rqrr`). **The pairing code** is printed
  **bare** on a line of its own, off the rail and never wrapped, so that selecting the line copies exactly it (the
  phone's parser takes nothing before `or2-pair:`), and a line starting `or2-pair:2?` is still the code for the
  tests.
- **The code question:** `◆  Code shown on your phone`, then the answer's line `│  ` where the cursor waits. The
  code is not echoed; once it is read the answer's line shows it masked (each character `•`, separators kept:
  `••••-••••-••••`). On a terminal that takes cursor movement (standard output a terminal, `TERM` not `dumb`,
  Unix) the answer's line first shows a dim hint (`hidden as you type; Enter when done`) with the cursor at its
  start, and the answered question is redrawn in place (`\r`, cursor up one line, erase line) as `◇` (accepted),
  `▲` (a typo: the reason follows under it, then a new `◆`) or `■` (no code: `No code was typed. Nothing was
  changed.`). The redraw is skipped (only the answer's line is drawn) when the process was stopped and continued
  during the question (Ctrl-Z and `fg`: the shell wrote meanwhile; the prompt's SIGCONT handler counts it) or the
  question does not fit one line. Off a terminal the answer simply ends the line the cursor is on. An ending
  signal during the question (Ctrl-C; SIGTERM, SIGHUP, SIGQUIT) still ends the rail: the handler that puts the
  terminal back also writes the end prepared before the question (`write(2)` of bytes already in memory: the
  answer's line `Cancelled. Nothing was changed.`, `│`, `└  Cancelled`), then the signal ends the process as
  before.
- **The wait** is one static line, `●  Waiting for the phone (until 12:35). Ctrl-C removes the temporary key.`: no
  spinner. A spinner redraws its line in place, and the keys a person types while waiting (Enter, while scanning)
  are echoed by the terminal and move the cursor, so the redraw would leave stale copies behind; turning echo off
  for the wait would need a second termios guard beside the signal counting. Not worth the risk for a calm wait.
- **Errors** (`RunError`) go to standard error after the rail on standard output: `│`, `■  <error>` (its fix on
  the next lines), `│`, `└  Failed`. An error before the run starts (the account cannot be found) opens the rail
  first. Usage errors (exit 2, before any rail) stay one plain `or2-pair: <message>` line, and `--help` and
  `--version` are plain (`--version` prints exactly `or2-pair X.Y.Z`; the installer reads it). A cleanup that
  fails in a panic or early return (the `Live` drop guard) reports on standard error with the same rail lines.
  `or2-pair enroll` speaks JSON to the phone and is not drawn.
- **Colour** only when the stream is a terminal, `NO_COLOR` is unset or empty, `TERM` is not `dumb` and
  `--no-color` is not given; only the 16-colour palette (SGR 2 dim for the rail and quiet text, 31 red, 32 green,
  33 yellow, 34 blue, 36 cyan, 0 reset), so it fits any theme. Standard error decides for itself.
- **Glyphs**: Unicode only when the locale is UTF-8 (the first of `LC_ALL`, `LC_CTYPE`, `LANG` that is set and
  not empty names `UTF-8` or `utf8`, in any case), and not with `--ascii`; otherwise the ASCII rail: `+` open,
  `|` rail, `` ` `` close, `*` info, `+` ok, `!` warning, `x` error, `>` question (asked and answered), `*` mask,
  `-` instead of `·` in the title. The QR keeps its own choice (half blocks unless `--ascii`): a terminal that
  draws UTF-8 under a C locale (a common SSH session) still gets the narrow code.

Output, for a host reached over its public address (no colour; `[QR]` stands for the rows of the code):

```text
┌  or2-pair 0.1.1 · pair a phone with this host
│
✔  sshd is answering on port 22 (OpenSSH_9.8)
✔  /home/dev/.ssh/authorized_keys can be written and sshd will honour it
✔  login shell /bin/bash runs /home/dev/.local/bin/or2-pair
✔  sshd will run /home/dev/.local/bin/or2-pair for the pairing
✔  tmux, herdr, mosh-server found
●  mosh needs UDP ports 60000-61000 open on this host
│  ufw is on; open them with:
│  `sudo ufw allow 60000:61000/udp`
│
●  Open or2 on your phone: Add host > Easy pair
│
◇  Code shown on your phone
│  ••••-••••-••••
│
●  This host
│  name       workstation
│  user       dev
│  ssh port   22
│  host key   ssh-ed25519 SHA256:…  (/etc/ssh/ssh_host_ed25519_key.pub)
│  addresses  10.147.17.5 (overlay)
│             203.0.113.9 (public)
│             2001:db8::9 (public)
│             workstation.local (mDNS name, same network only)
│
●  A temporary pairing key was added for dev until 12:35
│  Scan this with the same phone:
│
│  [QR]
│
│  Or paste this code into the app (Easy pair > Paste pairing code):
or2-pair:2?…
│
●  Waiting for the phone (until 12:35). Ctrl-C removes the temporary key.
│
✔  "OnePlus" can now log in as dev (SHA256:7xKc…)
│  Its key replaced the temporary key. To undo, delete the line ending or2-OnePlus-2026-10-02 in
│  /home/dev/.ssh/authorized_keys.
│  The previous file is saved as /home/dev/.ssh/authorized_keys.or2-backup-20261002-123012.
│
└  Paired
```

`--check` ends with `✔  No warnings. Nothing was changed.` (or `▲  2 warnings. Nothing was changed.`, warnings
and failed checks together) and `└  Done`.

## FFI (API 13)

- `pair_new_code() -> Arc<PairCode>`; `PairCode` is an opaque object with `display() -> String`
  (`7KQ4-M2XD-9PTM`, for the screen) and a redacted `Debug`.
- `parse_pair_payload(text) -> PairOffer` (`PairParseError` as in API 12; `UnsupportedVersion` now
  carries `version: u32`).
- `PairOffer { name, username, port, addresses, host_key: PublicKeyInfo, pairing_id: Option<String> }`;
  `pairing_id` is `None` for a `--manual` code. `PairExchange` and `PairSecret` are gone.
- `async pair_enroll(offer, code: Arc<PairCode>, public_key_line, device_label) -> Result<PairResult, PairError>`,
  `PairResult { username, fingerprint }`. Cancelling the coroutine closes the connection.
- `PairError`: `NoPairingId`, `InvalidOffer`, `InvalidKey`, `InvalidDevice`, `Unreachable`, `TimedOut`,
  `HostKeyMismatch`, `BootstrapRefused`, `NotOr2Pair`, `Protocol`, `ConnectionLost`, `Expired`, `Gone`,
  `KeyNotAccepted`, `HostFailed`, `Refused` (`request`, or a reason this phone does not know). Version
  1's errors that no longer apply are removed.
- `API_VERSION` = 13.

## Android

- **Add host** sheet unchanged (Easy pair, Set up manually). (Later one chooser everywhere: see
  [First run](#first-run-one-add-host-chooser).)
- **Easy pair screen**: the code `K` at the top ("Type this code into or2-pair on the host", the code in
  monospace as `7KQ4-M2XD-9PTM`, the one large element at about 24 sp; everything else at the compact
  scale of `docs/ui.md`), a one-line hint with the command (`or2-pair`, copyable), then the camera and
  **Paste pairing code** as in version 1 (permission handling unchanged).
- **Review** after the scan: name and user (user read-only when `pairing_id` is present), addresses, the
  host key's fingerprint, the key to authorize (an existing key or **New key**, saved first so a retry
  reuses it). The button is **Pair**; then **Pairing with <name>…** (1 to 3 s).
- **After `ok`**: the host, its addresses (the offer's port on each), user, key and the trusted `hk` are
  saved in one Room transaction (`AppDao.saveHostWithTrust`). On a fresh install the last step of adding
  the host comes next, **before** the host connects: or2's battery step ("Keep sessions alive in the
  background?", Allow / Not now, then possibly the system's own request), once ever and skipped when or2 is
  already exempt ([Permissions: none on connect](#permissions-none-on-connect)). Then the host's page opens and
  the usual connect runs, which goes straight to the biometric unlock: no notification permission and no
  battery dialog during a connect. A host-key prompt can only follow the unlock (the connection needs the
  decrypted key), and its buttons are **Trust and connect** / **Reject**. If saving
  fails after the host installed the key, the retry is **bound to that key** (its stored key id and
  fingerprint): key selection and **New key** are disabled on the review, and the retry repeats only
  the atomic host and trust save, never `pair_enroll` (the host's run is spent).
- **Messages** (one line each, wrapped on screen like every message):
  - `Unreachable`: "Couldn't reach <name> on port <p>. Pairing uses the same SSH port as connecting:
    the phone must reach it (same network, ZeroTier or Tailscale, or a public address)."
  - `HostKeyMismatch`: "The host presented a different key than the code. Nothing was sent."
  - `BootstrapRefused`: "The host didn't accept the pairing key. The code typed into or2-pair may
    differ, or2-pair may have stopped, or sshd may not read ~/.ssh/authorized_keys. Run or2-pair again."
    (sshd refused the derived key; the phone cannot tell these three causes apart.)
  - `NotOr2Pair`: "Something other than or2-pair answered on the host. Pair manually."
  - `Expired`: "or2-pair has stopped or timed out on the host. Run it again."
  - `Gone`: "Another device already used this pairing."
  - `KeyNotAccepted`, `HostFailed`: "The host couldn't add the key." with the reason where known.
- After a pairing that reached the host the screen draws a new `K`.
- A `--manual` code saves the host and trust at once and shows the key line (Copy, Share), unchanged.
- `PairFlow` stays logic over `PairBackend` and `PairStore` in a `ViewModel`.

**As implemented** (`io.github.code_akram.or2.pair`). Decisions where the text above left room:

- **Where `K` lives.** `PairFlow` holds one `PairCode` (an FFI object; Kotlin sees only its `display()`
  text, in `PairState.Scanning.code` as a `ShownCode` whose `toString` is redacted) and frees the old one
  when it draws another. A new one is drawn when the screen opens (`start`), when the screen is left
  (`cancel`), when a pairing has installed the key, and after a failure that reached the host. Going back
  from the review to the scanner keeps it: nothing was sent, and the host may still be waiting for it. The
  `ViewModel` keeps it across a rotation or the permission dialog.
- **"Reached the host"** is every failure except `Unreachable`, `HostKeyMismatch` (the connection ends
  before anything is sent) and the phone's own input errors (`NoPairingId`, `InvalidOffer`, `InvalidKey`,
  `InvalidDevice`). `TimedOut` counts as reached (the handshake may have succeeded), so the choice errs
  towards a new code.
- **Where a failure shows.** A failure that did not reach the host returns to the review with the message
  above the button, and **Pair** retries with the same `K` (the person typed it already). One that reached
  the host returns to the **Easy pair screen** with a new `K` and the message under the camera card: a
  retry from the old review could not succeed, because the host's run needs the new `K` typed again, so
  or2-pair is run again and the new QR scanned.
- **Saving.** After `ok` the host is saved with `AppDao.saveHostWithTrust` (the user is the offer's
  `user`, read-only when the code has a pairing id; the result's `fingerprint` was checked against the
  phone's key by the core). If saving fails, the review shows "The host accepted the key, but this phone
  could not save the host. Try again." and records the key as `PairReview.accepted` (`AcceptedKey`: the
  key's id and fingerprint; a key made by **New key** is already stored, so this is its id). From then
  on the review's key choice is locked (`PairFlow.edit` ignores a choice; the key rows and **New key** are
  disabled, and the key's note reads "The host added this key already; only saving the host is left."),
  no key is generated, and **Pair** repeats only `saveHostWithTrust` with that key, never `pair_enroll`.
  If the stored keys no longer hold a key with that id and fingerprint (deleted meanwhile), nothing is
  enrolled or saved: the flow returns to the Easy pair screen with a new `K` and "The key the host
  accepted is no longer on this phone. Run or2-pair again."
- **Messages not named above.** `TimedOut`: "<name> did not answer in time. Run or2-pair again and
  retry."; `ConnectionLost`: "The connection to the host ended early. Run or2-pair again and retry.";
  `Protocol`: "The host did not understand the request. Update or2-pair on the host and try again.";
  `Refused`: "The host refused the request. Run or2-pair again and retry."; `KeyNotAccepted` and
  `HostFailed` start with the contract's "The host couldn't add the key." and add the reason ("It does
  not accept this kind of key: use an Ed25519 key." / "Read what or2-pair printed on the host."). The two
  version messages end with a full stop. Version 1 means `version < 2`.
- **Tests.** `PairFlowTest` runs the flow over a fake backend whose codes and parser are the native ones.
  `PairEndToEndTest` runs the built `or2-pair-testhost` behind a disposable sshd (the fixture passes the
  testhost's `OR2_PAIR_TEST_*` variables to the forced command with `SetEnv`) and finishes with a real
  `connect_host`; it skips without sshd, and fails instead with `OR2_REQUIRE_SSHD`.

## Tests

- **CLI**: payload round trip and `validate`/parser agreement; `K` parsing (check character, lenient
  input, typos, `Z` refused, no substitution or swap exception), generation uniform over the 31 symbols,
  and the host's and the phone's readings compared on the same inputs (`tests/code_agreement.rs`);
  the derivation vector; options by sshd version (no `expiry-time` below 9.1), the UTC expiry; path
  character refusal; login shell refusal and the shell run (failing exit, time limit, wrong output, a
  background job holding the output stopped at the limit); `sshd_config` `fail` and `warn` findings
  with `Match` evaluation and unreadable includes, values ignoring case; `authorized_keys` append, remove (a
  new file, mode kept, no temporary left), the combined replace, `gone`, sweep (dead and live state,
  leftover temporary files), backup once per run, a hard link or a swapped file after the check refused,
  a failed replacement leaving the old file whole, the label and ACL copy, an ACL that cannot be set
  refused in words with the old file kept, a directory sync that fails after the rename a warning with
  the change kept (test-only fault injection: `safefs::fault`); the state directory checks, the
  complete publish (nothing visible before it), the liveness lock, the one lock, a lock of the account
  with an unusable or group/other-writable mode repaired (`0000` to `0644`, `0666`, `0620`, `0060`), a
  directory, a FIFO, another account's file and a hard-linked file refused in words about the lock; a
  rewritten `.done` whose warning is control characters, escapes or multi-byte text still read back
  (and one cut by characters is not); `enroll` (missing,
  expired, foreign, unheld state; bounded request; `.done`; a `.done` that cannot be written installs
  nothing; a replacement that fails takes the `.done` back; a sync that fails after the rename keeps the
  `.done`, with the warning, and answers `ok`; the lock held); the foreground's endings
  under the lock (a cleanup that meets an `enroll` in its commit reports the pairing; an `enroll` that
  reaches the lock after the cleanup refuses; a `.done` without the key is not a pairing, and says
  whether it removed the temporary key; nothing removed is not reported as removed; a pairing whose
  sync failed is reported with the warning; the wall clock past the deadline; a held lock and a second
  signal); the
  prompt (terminal required, re-prompt on typo, empty line; on a pseudo-terminal: echo off while typing
  and back after, nothing echoed, echo back after Ctrl-C at the prompt, the echo guard on a panic);
  what needs a process of its own (`tests/process.rs`, the other half in a child: the prompt on a
  pseudo-terminal in its own process group, a prefix typed, SIGTSTP, SIGCONT, the rest typed, nothing
  echoed and echo back after; the prompt ending while SIGINT is raised in its teardown window, or
  SIGTERM sent to the process, through a test-only teardown hook (`test-support`), with echo back
  after; Ctrl-C (SIGINT) during a login-shell check whose shell and its background job are slow, which
  leaves none of the shell's process group running; the lock under umask 0777 created 0600 and taken
  twice);
  cleanup on SIGINT, SIGTERM and SIGHUP of the built binary; SIGKILL of the built testhost (`enroll`
  refuses, the next run sweeps); `--manual`; the removed flags; the QR read back with `rqrr`.
- **End to end against a disposable sshd** (gated like the existing sshd tests, required in the full
  gate): the or2-core client pairs through real sshd, the forced command and the built testhost in a
  temporary home, then logs in with the paired key. Also: `BootstrapRefused` for a different `K` and after
  the run ended; `HostKeyMismatch`; `Gone` for a second phone; rc-file noise from a `.bashrc` that
  echoes; a `ForceCommand` the checks can read refuses before the prompt, and `NotOr2Pair` from one they
  cannot; a host under `TZ=<-05>5` with sshd under `TZ=UTC` pairs (the `Z` expiry), and sshd five hours
  ahead of UTC refuses a past `Z` expiry and accepts a future one; `sshd -T` of a disposable configuration
  reads `PubkeyAuthentication No` and `ForceCommand None` as the checks do. The host crate's suite also runs
  the product's own client (`or2_core::pair`, a dev-dependency) against the built testhost behind the disposable sshd: its parser
  reads the QR line the host printed (and agrees with the host crate's reference reader), `pair_enroll`
  pairs, and a different `K`, a different host key and an ended run give `BootstrapRefused`,
  `HostKeyMismatch` and `BootstrapRefused`.
- **`or2_core::pair`**: `PairCode` (the 31-symbol alphabet and `Z` refused as invalid, the check
  character with no exception and the five shared check vectors, rejection sampling and uniformity
  bounds, any white space ignored and the length checked first, as on the host), the parser table,
  the derivation vector, the client against a scripted exchange and every error mapping.
- **Kotlin**: `PairFlowTest` (fakes; among them that changing the key after a save failure starts no
  second enrolment, `afterASaveFailureTheKeyCannotBeChangedAndOnlyTheSaveIsRepeated`), `PairMessagesTest`,
  `PairEndToEndTest` (`or2-pair-testhost` behind a disposable sshd with `K` from the flow written to its
  stdin, then a real `connect_host` with the paired key and pinned host key, on an sshd with ED25519,
  ECDSA and RSA host keys, through the capability probe that follows `Connected` with no host-key
  prompt at any point), and `PairUiDeviceTest`
  with the new screen (screenshots of the Easy pair, review and pairing screens; the locked key choice
  of a save retry).

# First run: one add-host chooser

The owner compared the first run with Moshi's: Home's empty state offered "First step: Add an SSH key" and
"Then: Add a host" (the manual path), while **+** opened a different chooser (Easy pair, Set up manually). Two
designs for one job. Now there is one chooser, and no key step before it.

## Android

- **`AddHostChooser`** (`io.github.code_akram.or2.pair`, `AddHost.kt`): the two `ActionCard`s in `AddHostOptions`
  order, `FASTEST` / **Easy pair with QR** ("Run one command on your Mac or Linux box and scan the QR. or2 installs
  the SSH key for you.", "Recommended · ~1 min") then `SSH-FLUENT` / **Set up manually** ("Already comfortable
  with SSH? Enter the hostname, user and key yourself.", "~3 min · needs hostname + key"). It is drawn in three
  places with the same cards, copy and order: Home's empty state (under the `EmptyState`), the inbox's empty state
  (no host shows agents) and the **+** sheet (`AddHostSheet`). Card tags are `<prefix>-easy` and
  `<prefix>-manual`, the column `<prefix>-chooser`; the prefix is `add-host` in the sheet, `home-add-host` on Home
  and `inbox-add-host` in the inbox, so a chooser inline and one in the sheet never share a tag. The Easy pair card
  starts the flow and pushes `EasyPair`; the manual card pushes `HostForm(0)`; both close the sheet.
- **No key step on Home.** The empty Home no longer has "Add an SSH key" (`home-add-key`) or "Add a host"
  (`home-add-host-card`) cards. The keys icon on Home and the inbox still opens **SSH keys** for import and
  management. Home's attention card "Add an SSH key" (`home-add-key`) remains only for hosts that exist while no
  key is stored (a deleted key): a repair, not a first step.
- **New key in the host form.** The key choice is `KeyPicker` (`io.github.code_akram.or2.keys`), the radio group
  the Easy pair review uses too: stored keys (`host-key:<id>`), then **New key** (`host-key-new`), preselected for
  a new host when the phone has no key. With **New key** chosen, **Save** first calls `AppActions.createKey` (the
  former `generatePairKey`: `MainActivity.createKey`, Ed25519 generated in Rust, encrypted under a new vault key
  after the "Save SSH key" biometric prompt, stored), named like Easy pair's (`newKeyLabel`: `Key for <name>`,
  `newKeyComment`: `or2@<phone model>`). The stored key is then selected (a second **Save** never makes another),
  the host is saved with it, and the form shows `PairInstallKeyScreen` with `trusted = false` (the key's public
  line, Copy, Share; "<name> is saved. Add this line to ~/.ssh/authorized_keys on the host, then connect from Home
  and trust its host key once."). **Done** closes the form. A failure (biometric cancelled, vault refused) is one
  `danger` line (`host-form-error`, `newKeyErrorMessage`, the text Easy pair shows too) and nothing is saved. The
  prompt is tied to the screen: leaving or recreating it cancels the key, and `createKey` deletes the vault entry
  of a key that was not stored.
- **`HostFormScreen`** takes `createKey` and `deviceLabel`; `save` only stores, and the form closes itself (or
  shows the key line first). `openKeys` and the "Add a key" pill (`host-add-key`) are gone.

## Tests

- JVM: `AddHostOptionsTest` (order, copy, tags, no key step), `NavigationTest` (the chooser's two pushes from Home
  and from the inbox, Back, saved state), `KeyOperationsTest` (new-key names and the failure line).
- Device (compiled in the gate; run on the phone): `HomeUiDeviceTest.theEmptyStateAndTheAddHostSheetShowTheSameChooser`
  (the inline cards and the sheet's carry the same texts in the same vertical order as `AddHostOptions`), the empty
  Home's chooser wiring without a key step, `InboxUiDeviceTest` (the chooser in the inbox's empty state),
  `HostFormUiDeviceTest` (New key preselected without keys; Save makes the key with Easy pair's names, saves the
  host with it and shows its line; a failed key saves nothing) and `PairUiDeviceTest` (the sheet, unchanged).
- UI gallery: `home-empty` shows the chooser; `host-form-new-key` is the form with no stored key.

# Permissions: none on connect

The owner's first run on a fresh install: the first connect showed four dialogs in a row in the middle of
connecting (Android's notification permission, or2's "Keep sessions connected", Android's battery-optimisation
request, then the biometric unlock). Moshi does not ambush the user like that. Now **no permission dialog comes
during a connect**: every connect, the first one included, goes straight to the biometric unlock
(`MainActivity.connect` is the grouped unlock and nothing else). The two one-time questions moved to where they
make sense.

## Android

- **Battery exemption: the last step of adding a host.** `BatteryPrompt` (`app/OneTimePrompts.kt`) decides with
  `keepAliveStep(asked, exempt, requesting)`: `ASK` when it was never asked and
  `PowerManager.isIgnoringBatteryOptimizations` is false, `WAIT` while the system's request it opened is up,
  `DONE` otherwise. `shouldOffer()` is "`ASK` now". Adding a host ends on the step when it is:
  - **Easy pair**: once the paired host is saved, `NavStack.afterPaired(id, keepAlive)` opens
    `Destination.KeepAlive(id)` (`home|keepalive:<id>`) instead of the host page, and nothing connects yet; when the
    step is done it opens `HostPage(id)` and connects (the usual unlock). Without the step the host page opens and
    connects at once, as before. A code made with `--manual` ends on its key line; **Done** goes to
    `KeepAlive(0)` (`NavStack.afterKeyToInstall`), then Home.
  - **Manual form**: `HostFormScreen` ends a save through its new `saved` callback (after the key-line screen with
    **New key**); for a new host `NavStack.afterHostFormSaved(keepAlive)` replaces the form with `KeepAlive(0)`,
    which returns to the screen the form was opened from (Home or the inbox). An edit never shows the step.
  - **The step** (`pair/KeepAliveScreen.kt`, compact, centred like the pairing progress): the `CardTitle`
    "Keep sessions alive in the background?", one muted `Secondary` line "Android may stop the connection while or2
    is in the background.", **Allow** (`PrimaryButton`) and **Not now** (`TextAction`); Back is **Not now**. Tags
    `keepalive`, `keepalive-title`, `keepalive-why`, `keepalive-allow`, `keepalive-not-now`. `answer(allow)`
    records it at once (`battery_asked`), whatever the answer. **Allow** launches
    `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` through an activity-result launcher; the step shows its buttons
    off (`WAIT`) until `requestClosed()` (the result, or a device without the screen), so the biometric prompt never
    comes up over Android's dialog. **Not now**, or a refusal there, leaves Home's battery card (unchanged: "Background
    connections may drop", Allow, dismiss).
  - **Done**: an effect in `Or2App` watches `step` on a `KeepAlive` destination and, once `DONE` (and the stored
    hosts are read and nothing is busy), navigates with `afterKeepAlive()` and connects the paired host. The same
    effect covers a restore: a saved `keepalive:` destination whose answer is already stored (process death after
    the answer, or with the system dialog up: the wait is memory-only) goes on at once, and so does a step whose app
    became exempt meanwhile (`refresh()` on every `onStart`).
- **Notifications: offered in context, never on connect.** The foreground service starts and runs without
  `POST_NOTIFICATIONS` (on Android 13+ its notification is simply not shown). `NotificationPermission` holds the
  permission's state; `offer(NotificationUse.CONNECTION)` is Home's one-line card **"Show connection
  notification"** (Allow, close glyph; tags `home-notification-card`, `notification-card-allow`,
  `notification-card-dismiss`), shown only while a host is connected and the permission is not granted, until it is
  dismissed (`notification_offer_connection_dismissed`). It stays after a denial. **Allow**
  (`AppActions.allowNotifications`, `MainActivity.allowNotifications`) launches Android's dialog
  (`NotificationGrant.REQUEST`, recording `notifications_requested`) or, once Android no longer shows it (requested
  before and `shouldShowRequestPermissionRationale` false: denied for good), opens
  `Settings.ACTION_APP_NOTIFICATION_SETTINGS` (`NotificationGrant.SETTINGS`). Every offer re-reads the permission
  after the dialog and on every `onStart`.
- **The hook for later uses.** Agent alerts (M4) add a `NotificationUse` (its own dismissal key), show its offer where
  the alerts are switched on, and call the same `allowNotifications`; nothing asks on connect.
- **Migration.** The flags stay in `OneTimePrompts` over the app's `PrefStore`. `battery_asked`,
  `battery_declined` and `battery_card_dismissed` keep their keys, so a user who answered the connect-time
  explanation is not asked again and keeps (or does not get) the card. `notifications_asked` (the connect-time
  request) hides the connection offer and counts as a request for the Settings fallback. The saved instance state
  keys `pending_connect` and `pending_battery` are no longer written or read.

## Tests

- JVM: `OneTimePromptsTest` (the step's decision table; exempt and already-asked apps are never offered it; "Not
  now" and "Allow" are recorded at once, Allow waits for the system dialog, a refusal leaves the card; a new process
  after Allow does not wait; the old keys still count; the notification offer's visibility, dismissal and legacy
  flag; request versus Settings), `AddHostEndTest` (as `Or2App` drives it: Easy pair ends on the step and connects
  only after it; an exempt or already-asked app pairs straight into the connect; the step is asked for the first
  host only; the manual save ends on the step and returns to Home or the inbox without connecting; an edit never
  does; a `--manual` code's key line; a step restored after its answer), `NavigationTest` (`keepalive:` saved
  state, `afterPaired`, `afterKeyToInstall`, `afterHostFormSaved`, `afterKeepAlive`).
- Device (compiled in the gate; run on the phone): `AddHostEndDeviceTest` (the whole `Or2App` over fakes: Easy
  pair ends on the step and the host connects only after **Allow**; an exempt app pairs straight into the connect;
  the manual form's save ends on the step and **Not now** returns Home with the card; a connect from Home shows no
  dialog), `PairUiDeviceTest` (the step's copy, Allow, Not now and Back; nothing answers while Android's dialog is
  up; screenshot `keepalive`), `HostFormUiDeviceTest` (a save ends through `saved`, with **New key** only after the
  key line), `HomeUiDeviceTest` (the notification card next to the battery card),
  `ConnectionServiceDeviceTest.theServiceStartsAndKeepsRunningWithoutTheNotificationPermission` (skipped where the
  permission is granted).
- UI gallery: `keepalive`, `keepalive-waiting` (Android's dialog up), `home-notices` (both one-line cards).

# v0.1.1: instant opens and the Next items (API 14)

Owner requests of 2026-10-02: opening a terminal must be instant on every host, including one whose
firewall drops mosh's UDP (a Mac paired that day took seconds: AUTO waited for the capability probe,
then spent the whole 5 s mosh budget before falling back to SSH); and four roadmap Next items ship in
the same patch: agent notifications, wheel-aware scrolling, tap links with OSC 52, and gestures with
hardware-keyboard shortcuts. One FFI bump, **`API_VERSION` = 14**, covers every lane; each lane adds
only its own exports below.

## FFI (API 14)

| Export | Lane |
|---|---|
| `HostConnection.mosh_server() async -> Result<Option<String>, HostError>`: the path of `mosh-server`, resolved by the program probe alone (never by the herdr listing) | Instant |
| `SessionListener.on_server_pid(pid: u32)`: mosh only, at most once, before `Connected` and before the session sends its server anything; Rust waits for the return (the one callback that may block briefly: the app writes the pid durably there). Core: `SessionObserver::server_pid_known`, called by `SessionDriver::set_server_pid` | Instant (review fix) |
| `TerminalFrame.modes: TerminalModes { mouse_tracking: bool, alternate_screen: bool }` | Scroll |
| `ViewportScroll::Wheel { rows: i32, column: u16, row: u16 }` (negative rows = up; the touch's cell) | Scroll |
| `HostConnection.scroll_target(target: TerminalTarget, pane_id: Option<String>, scroll: TargetScroll) async -> Result<(), HostError>`, `TargetScroll { Up { lines: u32 }, Down { lines: u32 }, Bottom }` | Scroll |
| `ResolvedRow.links: Vec<CellLink { start_column: u16, end_column: u16, uri: String }>` (OSC 8 hyperlinks; empty when none) | Links |
| `SessionListener.on_clipboard_write(text: String)` (OSC 52 and OSC 1337 copy; reads are never answered) | Links |
| `HostConnection.navigate(target: TerminalTarget, pane_id: Option<String>, nav: TargetNav, client_id: Option<String>) async -> Result<(), HostError>`, `TargetNav { NextWindow, PreviousWindow, Pane { direction: NavDirection }, NextSession, PreviousSession }`, `NavDirection { Left, Right, Up, Down }` (`client_id`: the moving terminal's shown `Session.client_id()`; review fix, see "Fix: navigation by terminal identity") | Gestures |
| `Session.client_id() -> Option<String>`: a tmux terminal's own opaque client id (32 lowercase hex digits, fixed for the session's life; `None` for shell and herdr) | Gestures |

Both transports (the SSH pump and the mosh driver's engine) behave the same for every frame and
callback field above. A target with nothing to do (a `Shell` target for `scroll_target` or
`navigate`) returns `Ok(())` and does nothing.

## Instant opens (lane Instant)

**Rule: no open waits on UDP or on herdr's session listing.** Measured by the `or2.timing` markers
(debug builds): an inbox tap or a host-screen row on a connected LAN host reaches `frame` without any
wait for mosh, whatever the host's firewall does.

- **Program probe first.** `probe_within` publishes the program probe's result (`mosh_server`, `tmux`,
  `herdr` paths, locale) as soon as `PROBE_SCRIPT` returns; the herdr listing completes separately
  and fills `capabilities().herdr_sessions` as today. Every Rust wait for program paths (the tmux and
  herdr SSH opens) waits for the program probe only. `mosh_server()` is the new export Kotlin's
  transport choice awaits instead of `capabilities()`.
- **Per-connection UDP verdict** (`ActiveHost.udpVerdict`: `UNKNOWN`, `OK`, `BLOCKED`), reset on every
  new connection and on a host edit. Nothing about UDP is remembered across connections any more: the
  24 h memory (`MOSH_PAUSE_MS`, `moshFailedUntil`, `MOSH_PAUSED_NOTE`) is no longer read or written
  (the Room column stays, unused; no migration), because a blocked host now costs one invisible
  background attempt and a user who fixes the firewall must get mosh on the next connection.
- **AUTO, tmux and herdr targets:** verdict `OK` opens over mosh directly. `UNKNOWN` opens over **SSH at
  once** and starts a mosh terminal for the same target in the background (budget: the explicit-mosh
  15 s); when it reaches `Connected` the `ActiveTerminal` swaps to it (the existing
  `fallBackToSsh` swap run the other way: same id, thumbnail and navigation, the `attempt` counter
  drops the old session's frames) and the SSH session is disconnected; the verdict becomes `OK`. A
  background start that fails `TimedOut` or `NotInstalled { mosh-server }` sets `BLOCKED` and the
  terminal stays on SSH, unseen. `BLOCKED` opens over SSH with no attempt, until it is `UDP_RECHECK_MS` (5 min) old: then the next AUTO tmux or herdr open (never a shell) tries mosh once more behind its SSH terminal, unseen; `OK` swaps as usual, a failure blocks for another 5 min (added after the owner's Mac dropped mosh until its firewall prompt was answered, and the connection then kept SSH for its whole life). At most one background
  attempt per terminal; one in flight per host at a time (others wait for its verdict, never for UDP).
  Closing the terminal, or the user disconnecting the host, cancels the attempt and stops its server
  (the existing abandon/`terminate` path, its pid recorded like any mosh server's).
- **AUTO, `Shell` target** (two shells cannot be swapped): `OK` opens mosh; `BLOCKED` or no
  `mosh-server` opens SSH; `UNKNOWN` opens mosh with a budget of `max(700 ms, 6 × the program probe's
  round trip)` and falls back to SSH as today, setting the verdict.
- **No probe wait on a tap.** `awaitTransportChoice` awaits `mosh_server()` (one exec round trip), and
  only when the host connected in this tap; an inbox tap on a live host never waits.
- **Explicit `SSH` / `MOSH`** preferences are unchanged.
- **The note.** No note under terminals any more. A `BLOCKED` verdict shows one muted line on the host
  screen: `Mosh can't reach this host over UDP, so terminals use SSH.` (Changed after the owner's Mac:
  its firewall allowed mosh-server and UDP was still dropped, so the line names no fix.) The terminal header's badge follows the live transport (it flips SSH → Mosh on a
  swap). **Bug fixed with it:** the old note was drawn over the terminal's top rows; any line under
  the header must take layout space, never overlay the grid.
- **Tests:** JVM tests for the choice table, the swap (frames of the old session dropped, one attempt,
  cancel on close, `BLOCKED` stays SSH), the verdict reset; a Rust test that `mosh_server()` resolves
  while the herdr listing hangs; the device suite on the `.devicetest` app.

### Implemented (branch `v011/instant`)

**Rust.** `probe.rs` is split: `probe_programs` runs `PROBE_SCRIPT` alone; `probe_within(host, limit,
programs)` joins a program-probe future with `HERDR_SCRIPT` (the public `probe`/`probe_entries` pass
`probe_programs`, so their behaviour is unchanged). `SshHost` caches the two apart: `programs()` is a
`OnceCell` of the program probe, published the moment its script returns; `capabilities()` runs
`probe_within` with `programs()` as its program half, so the first full probe and any concurrent
`programs()` waiter share one `PROBE_SCRIPT` exec (still one exec per connection, two scripts at
once). `programs()` is what every program-path wait now uses: the tmux and herdr terminal plans (SSH
and mosh), the mosh start's `mosh-server` check, `list_tmux_sessions` and `focus_herdr_pane`.
`watch_herdr` still waits for the whole probe: it is not an open, and it needs the directory the
listing seeds. `HostCommand::MoshServer` / `HostHandle::mosh_server()` answer from `programs()`; the
FFI export `HostConnection.mosh_server()` wraps it, and `contract_probe_host` answers it with its
fixed path. Tests: `probe::tests::the_program_probe_is_published_when_its_script_returns_not_with_the_listing`
(paused time: a waiter on the program cache gets its answer at t=0 while the hung listing runs to its
bound), and in `ssh/connection_tests.rs`
`mosh_server_and_tmux_resolve_from_the_program_probe_while_the_herdr_listing_hangs` (a real SSH
fixture whose herdr listing never answers: `mosh_server()` and `list_tmux_sessions()` resolve in under
a second while `capabilities()` is still held, one program probe serves all three) and
`mosh_server_is_none_without_it_and_runs_only_the_program_probe` (`None`, cached, `Closed` after the
host closed). The fixture's `HerdrList::Hang` is new.

**Kotlin.**
- `TransportChoice.kt`: `UdpVerdict`, `MoshServerAnswer(path, roundTripMs)`, `OpenPlan` and
  `planOpen(pref, target, verdict, moshServer)` (the choice table) replace `chooseTransport`;
  `shellMoshBudgetMs`, `UDP_BLOCKED_LINE`. `MOSH_PAUSE_MS`, `MOSH_PAUSED_NOTE` and
  `moshFallbackNote` are gone.
- `HostConnections`: `HostPort.moshServer()`; on `Connected` the holder asks `mosh_server()` beside
  the probe and keeps the answer with its round trip (`ActiveHost.moshServer`, `moshServerSettled`;
  a failed query leaves it unknown). `ActiveHost.udpVerdict` (a `StateFlow`) starts `UNKNOWN` per
  connection and is reset by `hostEdited` (any edit that keeps the connection). `openTerminal` opens
  at once from `planOpen`. A tmux or herdr terminal opened over SSH under `UNKNOWN` requests a
  background mosh session (`ActiveTerminal.background`, numbered `attempt + 1`); while the host's
  verdict is `UNKNOWN` one terminal holds the probe (`ActiveHost.probingTerminal`) and the others queue
  (`awaitingVerdict`). The background session's callbacks go to `backgroundState`: `Connected` swaps it
  in (`attempt` moves on, handle and transport replaced, link health cleared, the server's pid
  recorded in the ledger, a frame tick for the view, the SSH session disconnected and released, the
  verdict `OK`, the queue started); `Closed` with `TimedOut` or `NotInstalled { mosh-server }` sets
  `BLOCKED` (the terminal stays on SSH, the queue is dropped); any other close is inconclusive and
  hands the probe to the next queued terminal. `cancelBackground` runs from `disconnectTerminal`,
  `retireTerminal` (dismiss), every host-wide close (`markHostTerminalsClosing`: the user's Disconnect,
  dismiss, release, a destination edit) and the foreground session's own `Closed`; it disconnects the
  attempt (Rust's abandon path stops its server) and releases it. Any AUTO-or-not mosh terminal that
  reaches `Connected` sets `OK`; the shell's SSH fallback sets `BLOCKED`. The constructor's
  `moshFailures` and `clock` are gone (`monotonicMs` measures the round trip).
- `awaitTransportChoice(current, connectedInThisTap = false)`: waits (at most 3 s) for
  `moshServerSettled` only under AUTO and only when `connectedInThisTap`. Only the Resume path that
  had to connect the host first passes `true` (`Or2App`'s pending resume, through
  `TerminalActivations.reopen`/`launchReopen`); the host screen's `open` no longer awaits anything.
- UI: `TerminalCard` has no `note`; the `Last heard N s ago` line moved into the header row (before the
  badge, the title ellipsizes), so nothing overlays the grid and the grid never resizes for it.
  `HostScreen(udpBlocked)` shows `UDP_BLOCKED_LINE` (`host-udp-blocked`) under the status card while
  connected; `Or2App` passes `BLOCKED && mosh-server not known to be missing`.
- Timing: the connect span also marks `mosh-server` (the answer) and `udp-ok` / `udp-blocked` (the
  verdict); the tap path still ends at `frame` (the SSH session's `terminal-connected`; the swap does
  not mark a second one).

**Decisions and deviations.**
- The shell's budget is capped at 15 s (`max(700 ms, 6 × RTT)`, at most what an explicit Mosh gets):
  a pathological round trip must not make AUTO wait longer than an explicit choice would.
- `UNKNOWN` with no `mosh_server()` answer yet: a shell opens SSH (no `mosh-server` known, no wait); a
  tmux or herdr terminal still starts its background attempt (a missing `mosh-server` then ends it
  `NotInstalled`, `BLOCKED`). A probe that answered "not installed" opens SSH with no attempt.
- Under `OK`, AUTO terminals keep `AUTO_MOSH_BUDGET_MS` (5 s) and the SSH fallback, in case the link
  changed since the verdict; a fallback then sets `BLOCKED`.
- The host-screen line is hidden when the program probe says `mosh-server` is missing (that is not a
  UDP problem), even though such a background failure also sets `BLOCKED` as specified.
- The data layer is untouched: the Room column, `MoshFailureStore` and the DAO stay (no migration),
  and `saveHost` still zeroes the unused column on a transport or address edit; nothing reads it or
  sets it any more. Removing that dead write would change data-layer and migration device tests for
  no behaviour.
- A background herdr-pane start focuses its pane again when it starts (the mosh plan owes the same
  focus as any herdr-pane open); it races nothing the user did unless they switched panes inside
  herdr in the second it takes.

**Tests (Kotlin).** `TransportChoiceTest` (the whole table, the shell budget's floor and ceiling,
explicit preferences under every verdict, the blocked line); `HostConnectionsTransportTest` (rewritten:
the shell budget from the measured round trip, SSH before the answer, the transport choice's wait only
on a tap that connected the host and only for `mosh_server()`, the swap with the old session's frames,
health and close dropped and the badge flow flipping, a background mosh that beats the SSH session,
`BLOCKED` for `TimedOut` and `NotInstalled { mosh-server }` staying on SSH unseen, inconclusive
failures, one attempt per terminal and one in flight per host with the queue started on `OK` and
dropped on `BLOCKED`, cancellation on close, dismiss, host disconnect and the SSH session's own close,
the verdict reset on a host edit and a new connection, the old 24 h memory ignored);
`TerminalActivationsTest` (a host-screen open and an inbox tap never wait; only a Resume that
connected awaits `mosh_server()`); `TimingTest` (the `mosh-server` and `udp-ok` marks, the tap path
through a swap); `HostConnectionsProbeTest` (the real-FFI swap against `contract_probe_host`: tmux
opens SSH and moves to the probe's mosh session, health sequence and roam after it; the swapped-in
server's pid is recorded); `HostContractTest` (`moshServer()` over the FFI, `NotConnected`/`Closed`).
Device tests (compiled, not run here): `TransportChromeDeviceTest` (the link line sits in the header,
never over the terminal, and never resizes it; no note), `HostScreenUiDeviceTest` (the blocked line
only while connected).

### v0.1.1 review fixes (branch `v011/fix-lifecycle`)

Two findings of the external v0.1.1 review.

**The mosh-server ledger had a process-death hole (P2).** The pid was written only when the main
dispatcher handled `Connected`, through `SharedPreferences.apply()`. A process killed after Rust accepted
the first authenticated datagram (from then on `mosh-server` has no idle timeout) but before that main
turn ran, or right after `record` returned with the `apply()` still pending, left a server no later
process knew to stop.
- **Rust.** `SessionObserver::server_pid_known(pid)` (default: nothing), called by
  `SessionDriver::set_server_pid` for a nonzero pid while the observer is held (not after `Closed`).
  `mosh_session::drive` sets the pid before `run_session` sends the first datagram, so the call returns
  before the server can see its client and before `Connected`. FFI: `SessionListener.on_server_pid(pid)`
  (API 14; table above), the one listener callback allowed to block briefly. The probe's mosh terminals
  call it with 4242 before `Connected`. SSH terminals never call it.
- **Kotlin.** `HostConnections`' listener records the pid in `onServerPid`, on Rust's callback thread
  (never on the main dispatcher's `Connected` path), and adds it to `ownServers` (host id, pid: this
  process's live servers, guarded by itself). `MoshServerLedger.record` writes durably:
  `PrefStore.putStringDurably` (new; `SharedPrefsStore` uses `commit()`, `MemoryPrefStore` just writes).
  Clears and purges stay `apply()` (a lost clear costs one repeated, harmless stop). Each session's own
  `Closed` (on main, whatever terminal it still speaks for: a background attempt, a fallback's first
  try, a swapped-out session) removes its pid from `ownServers` and clears its record on `Disconnected`
  or `RemoteExited`; `Failed` keeps it, as before. `reapOrphans` spares `ownServers` (no longer the open
  terminals' `moshServerPid`), and checks again before each stop. `ActiveTerminal.moshServerPid` is now
  only what the terminal shows (read from `Session.server_pid()` on `Connected` and on the swap).
- **Decisions.** Every server whose pid is known is recorded, not only one that connected: a background
  attempt is recorded while it is still connecting (and a reconnect meanwhile spares it), a cancelled
  one is forgotten on its `Disconnected`, and a start that fails before connecting (AUTO's fallback, a
  `BLOCKED` background attempt) keeps its record like any `Failed` session, so the next connection
  sends one more idempotent stop. The blocking write runs on a Rust runtime thread: a `commit()` of one
  small file, once per mosh start, before the UDP handshake.
- **Tests.** Rust: `session::tests::the_observer_hears_a_server_pid_when_it_is_set_before_connected_and_never_after_the_close`,
  FFI `session::tests::a_mosh_server_pid_reaches_the_listener_before_connected`, and the live
  `host_mosh.rs` orphan test (the pid is heard before `Connected`). JVM: `MoshServerLedgerTest.aRecordIsOnTheDiskWhenRecordReturns`
  (a `DiskPrefStore` fake: `putString` reaches its disk only on a later flush, like `apply()`);
  `HostConnectionsMoshServerTest` `aProcessThatDiesBeforeMainHandlesConnectedStillLeavesThePidOnDiskToStop`
  (pid and `Connected` delivered, the main dispatcher never run, the next process reads only the disk and
  stops the server), `aBackgroundAttemptIsRecordedBeforeItConnectsSparedByAReconnectAndForgottenWhenCancelled`,
  `aMoshStartThatFailsBeforeConnectingKeepsItsRecordLikeAnyFailure`; `HostContractTest` (the probe's
  `on_server_pid` through the real FFI). Device (compiled, not run): `PrefsDeviceTest` (the
  `shared_prefs` file holds a durable write and a ledger record when the call returns). The fakes report
  `on_server_pid` before `Connected` (`FakePort.serverStarted`), as Rust does.

**A transient first `mosh_server()` failure degraded AUTO shells for the whole connection (P3).**
`askMoshServer` settled the answer as unknown for good, and a later successful capability probe never
filled it in, so a shell kept opening over SSH until a reconnect.
- **Fix.** A failed query is not the answer. `askMoshServer` asks again after each of
  `MOSH_SERVER_RETRY_DELAYS_MS` (250 ms, 1 s, 2 s, 4 s) while the answer is unknown and the connection is
  still this one and up (one round at a time, `ActiveHost.askingMoshServer`). After a failure the
  capability probe answers it (it ran the same program probe): at once when its result is already in,
  else when `probe` gets one (at connect or on `refresh`). Its call's time is not one round trip (herdr's
  listing is in it, or nothing at all when cached), so the shell's budget uses
  `max(that time, UNMEASURED_PROBE_ROUND_TRIP_MS = 500 ms)`, a 3 s budget at least. While no query has
  failed, the capability probe does not pre-empt the query's measured round trip. `refresh` also starts a
  new round while the answer is unknown and none is running. `moshServerSettled` now completes with the
  answer or when a round's retries are spent (it describes that wait, not the answer), so a Resume that
  connected the host (at most 3 s) can still catch a retry that succeeds.
- **Tests.** `HostConnectionsTransportTest`: `aFailedMoshServerQueryIsAskedAgainAndAShellThenPlansMosh`,
  `aMoshServerQueryThatKeepsFailingIsAnsweredByARefreshedCapabilityProbe` (bounded at five queries, SSH
  meanwhile, then mosh with the 3 s budget after a refresh), `aMoshServerAnswerFromTheCapabilityProbeWhileTheQueryFailsAtConnect`;
  the wait test now spends the bounded retries.

**Fix: no input is lost in the swap (Codex v0.1.1 review, P1; branch `v011/fix-terminal-input`).**
The swap publishes the mosh handle and disconnects SSH in one main-dispatcher turn, but the view on
screen is recreated for the new handle only at the next recomposition; a key or IME commit taken by
the old view in between went to the disconnected SSH session and was silently dropped. Input now
never goes to a view's handle: `ActiveTerminal.input` is a stable `SessionRoute` (terminal package)
that reads the published handle at each call, and `TerminalScreen(input =)` binds every view to it
(`TerminalView.bind(session, route)`, `TerminalSession.call`). Every input path of a view (hardware
keys, IME commits, paste, the composer's submit, the toolbar and pad, wheel and arrow-key scrolls, and
input held behind a target's `Bottom`) goes through `TerminalSession.call`, so it reaches the session
the terminal shows when it is sent, exactly once. Only the view's own frame plumbing stays on its bound
handle (`callOwn`: `request_full_frame`, `resize`; `take_frame`), and a destroyed bound handle ends the
view's frames, never its input. Ordering: once the handle is published nothing can target SSH; input
sent to SSH before that is ahead of its `Disconnect` in the session's single command queue, so it is
written before SSH closes. No FFI change. Tests: `HostConnectionsTerminalInputTest` (a key, a text, a
submit sent through the view still bound to SSH after the swap reach mosh once each and SSH never,
also after the SSH object is destroyed; a key held behind a `Bottom` that returns after the swap goes
to mosh once), `TerminalSessionTest` (input follows the route, frames stay with the bound handle).
`FakeSession` now records input and refuses it after `disconnect`, as Rust does.

## or2-pair on macOS: the firewall (lane Instant, host side)

`HostFacts::detect` on macOS runs one read-only command (an exception to "files only", recorded in
"Fixes for this host"): `/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate`, and when it
is on, `--getblockall` and `--getappblocked <mosh-server's real path>` (symlinks resolved, Homebrew's
`/opt/homebrew/bin/mosh-server` → `Cellar/...`). No `sudo`, nothing changed. When the firewall would
block mosh-server, the checks print a warning (pairing still proceeds: SSH works) with the exact fix:

```sh
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add "<real path>"
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "<real path>"
```

and say that `brew upgrade mosh` replaces the binary, so the rule must be added again (`or2-pair
--check` shows it). "Block all incoming connections" gets its own sentence (it overrides any rule). A
missing `socketfilterfw` or unreadable output is silence, not an error. Unit tests on captured outputs;
no test runs the real command.

**Implemented** (`core/or2-pair/src/hints.rs`, branch `v011/pair-macos`):

- `HostFacts::detect` takes a `&dyn Commands` (`run(program, args) -> Option<String>`: standard output
  then standard error, whatever the exit status, `None` when it cannot be started or does not finish).
  The real one, `SystemCommands`, runs the program directly (no shell, no `sudo`, standard input
  closed) with a 3 s limit (killed at the limit; answer `None`) and keeps at most 64 KiB of each stream.
  On every system but macOS nothing is run. `socketfilterfw` is looked for under the fake root in tests.
- mosh-server is found as the checks find it (`PATH` and the usual directories first, then
  `/opt/homebrew/bin`, `/usr/local/bin`) and resolved with `canonicalize` (Homebrew's relative link
  `bin/mosh-server` → `Cellar/mosh/<version>/bin/mosh-server`); that path is what `--getappblocked`,
  the printed commands and the messages name. `--getblockall` and `--getappblocked` are asked only
  when the global state is on; the rule only when mosh-server was found.
- **Parsing** (no Mac ran this lane: the wordings are what could be gathered, so the reading is
  deliberately loose and anything ambiguous is "unknown"). Global state: `(State = N)` decides when
  present (0 off, 1 on, 2 on with block all; any other number unknown), else the words
  (`enabled`/`on` versus `disabled`/`off`; both or neither is unknown; `blocking … all` is block all).
  Block all: needs the word `block`, then the same on/off words (`Firewall has block all state set to
  enabled.`, older `Block all ENABLED!`). App rule: mosh-server's path is cut out of the text first (a
  directory named `blocked` says nothing); `not part of the firewall` is "not listed"; else
  `permitted`/`allowed`/`unblocked` versus `blocked`/`denied`, both or neither unknown. Lines about
  **stealth mode** (and logging) are ignored: stealth mode drops pings and probes of closed ports, not
  traffic to an allowed program, so it is never a finding and is not queried.
- **Verdict.** A program that is **not in the list** is treated as blocked: macOS would ask in a dialog
  on the Mac's screen, and nobody answers it for a mosh-server started over SSH. Checks printed (only when
  mosh-server is found, in place of the generic macOS line):
  - firewall off: `ok    the macOS firewall is off (it does not block mosh's UDP)`;
  - on, block all off, mosh-server allowed: `ok    the macOS firewall is on and allows mosh-server (<path>)`;
  - block all on: a `warn` of its own ("blocks all incoming connections, which overrides any rule"; it
    also says sharing services such as Remote Login are blocked from other machines) with the
    System Settings path (Network > Firewall > Options) and `sudo …/socketfilterfw --setblockall off`;
  - mosh-server blocked or not listed: a `warn` ending "so mosh cannot reach this host and terminals use
    SSH; allow it:" followed by the two commands of this contract, each on its own line indented under
    the text, then the upgrade note. Both warnings appear when both apply.
  - anything else (tool missing, first answer unreadable, block all or the rule unreadable with no
    warning to give): the generic macOS line as before.
  Commands drop `sudo` when `or2-pair` runs as root. The path is double-quoted as in this contract, or
  single-quoted (with `'\''`) when it holds `"`, `$`, `` ` ``, `\` or `!`.
- **Deviation:** the `brew upgrade mosh` sentence is printed only when the real path is in a Homebrew
  `Cellar/` ("`brew upgrade mosh` installs a new mosh-server at another path, so add the rule again after
  an upgrade (or2-pair --check shows it)"); another install (MacPorts, a source build) gets "upgrading
  mosh replaces this binary, so add the rule again …", because naming `brew` there would be wrong.
- **Tests** (`hints.rs`): `socketfilterfw_answers_are_parsed_in_every_wording` (off, on, state 2,
  stealth lines, older wordings, garbage, errors, unknown states, a path containing `blocked`);
  `the_mac_firewall_is_asked_about_mosh_servers_real_path` (a fake Homebrew tree with the relative
  Cellar link: exactly the three queries in order, the Cellar path asked; off asks nothing more; block
  all from state 2 when `--getblockall` is unreadable; unreadable answers are unknown; missing tool or a
  garbage first answer is `None`; no mosh-server asks no rule; Linux runs nothing);
  `the_mac_firewall_prints_the_exact_fix_when_it_would_block_mosh` (blocked and not-listed texts in full,
  root without `sudo`, the non-Homebrew note, quoting, block all alone and with a rule, allowed and off
  are `ok`, unknowns print nothing); `system_commands_capture_both_outputs_and_give_up_in_time` (both
  streams, a non-zero exit, a missing program, the time limit). `checks.rs`:
  `a_mac_firewall_that_blocks_mosh_warns_with_the_fix_and_pairing_goes_on` (a `warn`, no `fail`, no
  generic line; with nothing known the generic line as before). Not run on macOS; the real command is
  never run by a test.

## Agent notifications (lane Notify)

Kotlin only, no FFI change. The roadmap rule: **exactly one notification per Blocked or Done edge**
(`HerdrAgent.state_change_seq` advancing into `Blocked` or `Done`, or into `Idle` after `Working`: see
"The rule" below), **none while that pane is on
screen** (its terminal is the visible one and the app is resumed), **tap opens the pane** (through
`launchOpenAgent`, the same path as an inbox tap).

- A new channel `agents` ("Agents", `IMPORTANCE_HIGH`), created with the existing `connections` one.
  One notification per pane (id from host id + session + pane id), replaced on the next edge and
  cancelled when the pane is opened, goes back to `Working`, or disappears.
- Title: the agent's label as the inbox shows it; text: `Needs input` (Blocked) or `Done`; sub-text: the
  host name. Nothing from the pane's output.
- **On by default** for every host shown in the inbox (`showInInbox`), once the notification permission
  is granted; a switch in Settings (`Agent notifications`, default on) turns all of them off. No
  per-host setting (no Room migration). The Home permission card's text says it covers connection
  status and agent alerts (`NotificationUse.AGENT_ALERTS`). Owner preference: zero configuration.
- Edges seen while the app was not watching (the first snapshot after a connect) never notify: only an
  advance observed by a live watch does.
- `MainActivity` handles the notification's intent (`onCreate` and `onNewIntent`): host id, session,
  pane id; a host that is no longer connected is connected first, as a Resume does.
- Tests: JVM tests for the edge rule (one per seq, no notification on screen, none on the first
  snapshot, cancel rules); the device suite checks the channel and a posted notification.

**Implemented (branch `v011/notify`).**

- **Seam.** `HostConnections.herdrObserver` (`HerdrObserver`, a `fun interface`) is told every state of
  every herdr watch, on main, in delivery order, right after the watch's own state flow has it, and only
  while the watch is running (`handle != null`: a watch stopped with its connection says nothing more).
  It is fed from the native listener, not by collecting the `StateFlow`, so a quick Blocked → Working
  pair cannot be conflated away. Nothing else in the holder changed.
- **The rule** (`notify/AgentAlerts.kt`, plain JVM): a baseline per watch object (each connection makes
  new ones, so a reconnect's first view is a baseline), cleared by any non-`Live` state (herdr stopped
  or failed, then came back: that first view is a baseline too). A later view posts when a pane's
  `state_change_seq` is greater than the one last seen and its status is `Blocked` or `Done`; a pane
  first seen in a later view (no earlier seq) is baselined, never notified; a seq that went backwards
  (herdr restarted under a live watch) is no advance. `Unknown` never notifies, and `Done` →
  `Idle` keeps the notification (only `Working`, opening and disappearing cancel it, as written).
  **Idle after Working is a Done edge** (fix, 2026-10-03): herdr reports a finished turn as `Idle`
  instead of `Done` when the pane counts as seen, and a `pane.focus` (or2 opening the pane) makes it
  seen, so the usual flow (open the agent from the phone, send, lock the phone) ended in `Idle` and
  never notified. Observed on the owner's host (herdr 0.9.3): a focused Claude Code pane went
  `working` → `idle` with the seq advancing by one, twice, and no notification arrived. So an
  advance into `Idle` posts `Done` when this watch saw the pane `Working` since it last settled
  (`Blocked`, `Done` or `Idle`; `Unknown` settles nothing, a repeat view changes nothing). An `Idle`
  never preceded by `Working` on this watch (a baseline, an `Unknown` flap, `Done` or `Blocked` then
  `Idle`) still never notifies. Tests: `aTurnHerdrReportsAsIdleNotifiesDone`,
  `anIdleNotReachedFromWorkingNeverNotifies`,
  `aTurnWorkingAtTheBaselineThatEndsIdleNotifiesUnlessOnScreen`.
- **On screen** is reported by `Or2App`: the visible `Destination.Terminal`'s host and target while the
  lifecycle is `RESUMED`, else null. A herdr terminal shows herdr's focused pane (shared state), so the
  pane on screen is the session view's `focused_pane_id`, falling back to the target's pane id before
  herdr reported one; an edge for that pane is recorded and never posted later. Becoming on screen
  (any way in: inbox, thumbnail, switcher, the notification) and the focus moving to a notified pane
  inside the shown terminal both cancel its notification.
- **Notification.** Tag `agent:<host>:<d | s<len>:<session>>:<pane>` with the fixed id 2 (the
  service's is 1, untagged): the length prefix keeps a session name and a herdr pane id (`w1:p2`) from
  reading as another pair. `CATEGORY_STATUS`, auto-cancel, `setWhen` now. The channel is created by
  `ConnectionService.createChannel` beside `connections`, and again (idempotent) before each post.
  Nothing is posted without `POST_NOTIFICATIONS`.
- **Tap.** `PendingIntent.getActivity` with `FLAG_IMMUTABLE | FLAG_UPDATE_CURRENT`, request code the
  tag's hash and a data URI of the tag (so each pane's pending intent is distinct whatever the hash
  does), `SINGLE_TOP | CLEAR_TOP`, action `io.github.code_akram.or2.action.OPEN_AGENT`. **Addition:**
  `MainActivity` is exported (the launcher), so the intent also carries a random token made once and
  kept in the app's prefs (`agent_open_token`); an intent without it is ignored, so another app cannot
  drive or2 to a pane. `onCreate` (whatever its saved state; see "Review fixes") and
  `onNewIntent` cancel the notification and hand the pane to `AgentOpenRequests`. `Or2App` takes it
  once the stored hosts are read and the launch's own recovery has run: connected → `launchOpenAgent`
  at once; connecting or waiting for a host-key decision → once connected; not connected → the usual
  grouped unlock and connect, then `launchOpenAgent` (`resumeStep`, as a Resume; the pending pane is
  saved state). A tap that started the app suppresses the cold-launch auto-resume, and a tap that
  brought it back suppresses the return's reattach (the user asked for that pane).
- **Settings** (new): `Destination.Settings` (`settings`), pushed from a fourth trailing icon on Home
  (`Or2Icons.Settings`, "Settings", `nav-settings`). A `TopBar` "Settings", section `NOTIFICATIONS`, one
  grouped row `Agent notifications` with the toggle (`settings-agent-alerts`) and a muted sentence under
  the card. The switch is `AgentAlertSettings` (prefs key `agent_alerts_off`, so the default is on).
  Turning it off cancels every agent notification and posts none (edges seen meanwhile are still
  recorded, so turning it on never posts late); turning it on without the permission asks for it
  (`allowNotifications`, the in-context path). The screen is `settings/SettingsScreen.kt`, built for
  more rows (lane Links' `Copy from the host` belongs in it).
- **Permission card.** Home's card reads `Show connection and agent notifications` and is the
  `NotificationUse.AGENT_ALERTS` offer (its own dismissal key, no legacy key): a user who dismissed the
  connection-only card, or answered the old connect-time request, is offered it once more, since agent
  alerts are a new use. `CONNECTION` stays for its legacy flag.
- **Not done:** a host whose watches stop without a state (the inbox flag turned off on a live
  connection, a session no longer listed) keeps the notifications already up until they are opened;
  a deliberate disconnect or a deleted host does not cancel them either (a tap on a deleted host's
  notification says `That host no longer exists.`).
- **Tests.** `AgentAlertsTest` (16): first snapshot, one per seq (re-deliveries, Blocked → Done,
  Idle/Unknown, backwards), the notification's text, cancel on Working and on disappearing (not on
  Idle), nothing on screen (and not late), on screen as the focused pane of that host and session,
  showing or opening cancels, baseline after unavailable and on a new watch, a pane first seen later,
  sessions and hosts kept apart, the switch, the setting's default, the tag's uniqueness and saved
  state, the token, the connect-first decision, and the holder feeding it from live watches only.
  `OneTimePromptsTest` (the `AGENT_ALERTS` offer), `NavigationTest` (`settings`). Device:
  `AgentNotificationsDeviceTest` (the service creates the channel; a Blocked edge posts one
  notification with its title, text, host, tap and auto-cancel, which opening cancels: skipped
  without the permission), and `HomeUiDeviceTest`'s card text.
- **Review fixes (Codex v0.1.1 review, branch `v011/fix-notify`).**
  - *A new process reconciles what an old one left up.* Notifications outlive the process, so the set
    of panes with one up is no longer process-local: `AgentAlertSink.shown()` (new) reads it back from
    `NotificationManager.getActiveNotifications()` (our id 2 on the `agents` channel, the tag parsed
    by `AgentPaneKey.fromTag`, which only accepts a tag that round-trips; empty when the call fails),
    and `AgentAlerts` starts from it. Its first views therefore cancel a pane that went back to
    `Working` or disappeared while no process watched, though they still post nothing (baseline). A
    pane seen `Working` by a watch for the first time (its baseline, or a new `state_change_seq`)
    cancels unconditionally, known or not; a repeat of the same view (same seq, so still `Working`)
    cancels only what this process posted since, so a steady `Working` pane costs no call per view.
    The off switch cancels this process's set and whatever `shown()` lists. Decision: rebuild from the
    system rather than persist the set (the system is the truth; the user may also swipe one away).
  - *A tap is never discarded by a restore.* `onCreate` always inspects its intent: Android may create
    the activity, with the killed one's saved state, for a new tap (no live activity gets
    `onNewIntent`), and a non-null bundle no longer means "old intent". Each tap's intent carries its
    own id (extra `io.github.code_akram.or2.extra.TAP`, a fresh UUID per post, replacing the pane's
    pending intent's extras), and `AgentTaps` (plain JVM) keeps the ids taken as the activity's saved
    state (`agent_taps`): a recreation handing back a taken tap (rotation, restore) does nothing, a new
    one opens its pane. It keeps the first id and the newest, 16 in all (the first may be the intent
    that created the activity, which a restore hands back). A tap relaunched from Recents
    (`FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY`, the task's old intent) is not a new tap; an intent without
    a tap id is not a tap.
  - Tests (`AgentAlertsTest`, each failed before the fix): `aNewProcessReconcilesTheNotificationsTheOldOneLeftUp`
    (a second `AgentAlerts` over a sink that keeps the first's notifications: its baseline cancels the
    pane back at `Working` and the gone one, keeps the Blocked one, and its off switch takes the rest),
    `aPaneSeenWorkingIsCancelledEvenWhenWhatIsUpCannotBeRead` (unconditional `Working` cancel; the
    switch asks the system), `aTapOpensWhateverTheSavedStateAndARecreationNeverRepeatsOne` (a non-null
    saved state with a fresh tap opens it; a replay of a taken one does not; Recents; the bound), and
    the tag round trip in `eachPaneHasItsOwnTagAndItSurvivesAsSavedState`. The test sink now behaves
    like the system (what is up survives the instance; a cancel removes only what is up). Device:
    `AgentNotificationsDeviceTest` also checks the tap id and that `shown()` lists the posted pane.

## Wheel-aware scrolling (lane Scroll)

A vertical swipe scrolls what the user is looking at, never shell history:

1. **Mouse tracking on** (`TerminalModes.mouse_tracking`: tmux with `mouse on`, herdr, vim with mouse,
   any TUI that asked): the swipe sends wheel events at the touch's cell (`ViewportScroll::Wheel`),
   encoded by libghostty-vt's mouse encoder with the terminal's own mouse format.
2. Else, **a tmux target** on the alternate screen: `scroll_target` runs, over exec, `tmux copy-mode -e
   -t <session>` then `tmux send-keys -t <session> -X -N <lines> scroll-up` (or `scroll-down`);
   `Bottom` sends `-X cancel`. No consent prompt and no `set mouse on`.
3. Else, **a herdr target**: `scroll_target` uses herdr's `pane.scroll` (`offset_from_bottom`, the
   pane's current offset kept by Rust per pane; `Bottom` is 0).
4. Else (a plain shell, or a full-screen program on the alternate screen without mouse): today's
   behaviour (primary screen: the scrollback viewport; alternate screen: arrow keys).

Swipes are coalesced: at most one `scroll_target` call in flight per terminal, the deltas summed.
**Scroll-to-bottom button:** a small round button at the bottom right (compact, per the UI system)
shown while the primary-screen viewport is above the bottom (`Scrollback.offset > 0`) or after a
route 2 or 3 scroll up that has not returned to the bottom; tapping it returns (viewport bottom, or
`TargetScroll::Bottom`). Any key, paste or composer submit while a route 2 or 3 scroll is away first
sends `Bottom`, so typing never lands in tmux copy mode. Tests: Rust tests for the wheel encoding (SGR
and X10 formats, the touch's cell) and the tmux/herdr commands against `LocalHost`/the herdr fixture;
JVM tests for the routing and the button's visibility.

**Implemented (branch `v011/scroll`).**

- **Modes.** `TerminalEngine::frame` attaches `TerminalModes` from libghostty's `is_mouse_tracking()`
  (DECSET 9, 1000, 1002, 1003) and `active_screen()`; `Frame::with_modes`, and a merged delta takes
  the newest modes, so a mode change that dirties no row still reaches Kotlin (`TerminalGrid.modes`).
  The mosh engine is the same `TerminalEngine`: mosh's server relays the mouse modes (1000-1006,
  1015) in its diffs, so mouse tracking reports the same over mosh; it **never relays the alternate
  screen** (its own emulator keeps it; `mosh-server` emits no 1047/1049), so `alternate_screen` is
  false on a mosh terminal.
- **Wheel.** `ViewportScroll::Wheel` encodes `rows` presses of button 4 (up) or 5 (down) with
  libghostty's mouse `Encoder` (`set_options_from_terminal`: the terminal's tracking mode and
  format; SGR, UTF-8, urxvt, X10), at the centre of the touched cell (clamped to the grid), at most
  one viewport of events per call. One event per row, as Termux does. Without mouse tracking, or in
  a mode that reports no wheel (X10 tracking, DECSET 9), it is a `Delta` of `rows` (viewport or
  arrow keys), so a swipe always does something. The touched cell is where the finger went down
  (the pane the user touched, even if the finger crosses into another).
- **tmux (`tmux::scroll_command`, `tmux::scroll`).** One exec, tmux commands joined by `;`: `Up` is
  `tmux -u copy-mode -e -t =<name>: ; send-keys -t =<name>: -X -N <lines> scroll-up` (a pane already
  in copy mode keeps its position); `Down` is only `send-keys … -X -N <lines> scroll-down`
  (entering copy mode to scroll down would flash it); `Bottom` is `send-keys … -X cancel`. `=<name>:`
  targets the exact session's active pane (a bare `=<name>` is not a pane target, and a bare name
  could match a prefix). tmux's "not in a mode" (a `Down`/`Bottom` after `-e` already left copy mode)
  is success.
- **herdr (`herdr::scroll_pane_in`, `herdr::ScrollOffsets`).** The pane is `pane_id`, else the one
  herdr answers `pane.current` with (the session's focused pane, whose answer also carries its real
  offset). Kotlin passes the focused pane from the session's herdr watch when it has a live view
  (one round trip fewer), `None` otherwise; the target's own `pane_id` is not used (the user may
  have moved focus since the terminal opened). `pane.scroll` answers with the pane's info, and its
  `scroll.offset_from_bottom` (herdr clamps an offset past the top of the history) becomes the kept
  offset, so a swipe down after an overshoot starts from the real top. Offsets are kept per
  connection by (session, pane); `PaneNotFound` forgets the pane's. Verified against herdr 0.9.3.
- **`scroll_target`** validates names (`InvalidName`), returns `Ok` without a round trip for a `Shell`
  target or zero lines, and runs on the host's driver like `focus_herdr_pane` (`NotInstalled`,
  `PaneNotFound`, `CommandFailed`).
- **Kotlin.** `scrollRoute` (`TargetScroller.kt`) picks the route; `TerminalView.scrollRows` applies it
  to swipes, flings and the history key's page up. `TargetScroller` keeps at most one `scroll_target`
  in flight per terminal and sums the swipes meanwhile, tracks the lines scrolled up (a swipe down at
  the bottom sends nothing), and holds input: any key, text, paste or composer submit while the target
  is away (or a call is in flight) sends `Bottom` and goes out, in order and exactly once, only after
  a `Bottom` has **succeeded**; a failed one keeps it held (superseded rule: it used to go out also
  when the call failed; owner decision after the review, see "Fix: held input waits for a `Bottom`
  that succeeds" below). `HostConnections.scrollTarget` uses the host's current connection (a mosh
  terminal outlives the one it opened on). The button (`ScrollToBottomButton`, a 28 dp disc in a 40 dp
  box at the terminal's bottom right, docs/ui.md) shows while `scrollToBottomVisible`; tapping it, or
  holding the history key, returns the target to `Bottom` and/or the viewport to its bottom.

**Deviations.**

- **A tmux target takes route 2 whichever screen is active** (not only on the alternate screen):
  over SSH tmux is on the alternate screen for the terminal's whole life anyway, and over mosh the
  alternate screen is never reported (above), so the condition would send tmux over mosh to the
  local scrollback, which holds mosh's redraws rather than tmux's history.
- **"Above the bottom" is `offset + rows < total_rows`**, not `offset > 0`: `Scrollback.offset` is the
  first visible row counted from the top of the scrollback, so the bottom is `total_rows - rows`.

**Tests.** Rust: `terminal_tests.rs` (modes in frames, including a row-less delta; SGR wheel at the
touched cell, clamping and the one-viewport cap; any-event tracking; X10 format bytes; no tracking or
DECSET 9 falls back to a delta/arrows), `frame.rs` (merged deltas report the newest modes),
`mosh/ghostty.rs` (modes and wheel through mosh diffs), `herdr/scroll.rs` against the fake herdr
(kept offsets, herdr's clamp, the focused pane via `pane.current`, `PaneNotFound`), `tests/local.rs`
(the tmux commands on a private tmux server: exact-session targeting, copy mode, `-e`, zero lines, a
missing session), `tests/host.rs` (`scroll_target` over SSH: tmux, `Shell`, zero lines, invalid
names, a missing session), `tests/herdr_live.rs` (an isolated herdr 0.9.3 session), FFI conversions.
JVM: `TargetScrollerTest` (routes, button visibility, coalescing, down at the bottom, input held
behind `Bottom`, a failed call), `TerminalGridTest` (modes per frame), `HostConnectionsTest`
(`scrollTarget` through the connection with the watch's focused pane).

**Fix: the scroll state belongs to the terminal (Codex v0.1.1 review, P1; branch
`v011/fix-terminal-input`).** The `TargetScroller` lived in one `TerminalView`, in the composition's
coroutine scope: hiding a scrolled terminal (minimise, another terminal selected) or the swap's new
view forgot that tmux was in copy mode (or the herdr pane scrolled), the button vanished, and the
next key went straight into copy mode. Now:
- One `TargetScroller` per tmux or herdr terminal (`ActiveTerminal.targetScroller`, null for a shell),
  created by `HostConnections.openTerminal` in the holder's scope with `scrollTarget(terminal, _)`; it
  survives the swap, a new view and the terminal being hidden. `TerminalScreen(targetScroller =)`
  hands it to each view (`TerminalView.useTargetScroll(target, scroller)`); the view reads
  `awayState` (a `StateFlow`) at creation and on change, so a view of a terminal already scrolled away
  shows the button at once.
- Hiding: `SessionScreen` calls `HostConnections.hideTerminal(terminal)` as it leaves composition (not
  on the swap, which keeps the screen): `TargetScroller.leave()` sends `Bottom` when the target is
  away, best effort, in the holder's scope (a view going away no longer cancels it). The swap and a
  recreated view send nothing: the user is still reading the history.
- **Unconfirmed.** A `Bottom` that fails or is cancelled (unless another `Bottom` is already queued
  behind it) leaves the scroller `unconfirmed`, which counts as `away`: the button shows, and the next
  input sends `Bottom` first. A swipe down while unconfirmed is sent and does not clear it. *Superseded
  by the fix below:* input held behind a failed `Bottom` no longer goes out; it waits for one that
  succeeds.
- Tests: `TargetScrollerTest` (a failed `Bottom` keeps the target away until one succeeds, a queued
  `Bottom` decides over a failed one, `leave` only while away, a cancelled `Bottom`),
  `HostConnectionsTerminalInputTest` (state kept across the swap with the button for the new view and
  `Bottom` before the first key; a recreated view of a scrolled herdr terminal; hiding sends `Bottom`;
  a failed hiding `Bottom` shows the button again and the next key waits for a successful one; a
  `Bottom` in flight when hidden is not cancelled; a shell has no scroller). Device test (compiled, not
  run here): `TerminalChromeDeviceTest.aTargetScrolledAwayBeforeTheViewExistedShowsTheButtonAlsoAfterTheSwapsNewView`.

**Fix: held input waits for a `Bottom` that succeeds (Codex v0.1.1 fix check, P1; branch
`v011/fix-2`).** The fix above still released input held behind a failed `Bottom`, and a failed `Down`
could leave the local count at zero and release it too, so a key could still land in tmux's copy mode
(or a scrolled herdr pane). **Owner decision after the review:** input typed while a tmux or herdr
target is, or may be, scrolled away waits until a `Bottom` has **succeeded**, then goes out exactly
once, in order. In `TargetScroller`:
- **Any failed call is unconfirmed.** A `Bottom` that fails or is cancelled, and an `Up` or `Down` that
  fails (it may or may not have moved the target), set `unconfirmed` (unless a `Bottom` is already
  queued behind it, which decides); local positions are never trusted after a failure. While
  unconfirmed the target is `away`: the button shows, and the next input needs a `Bottom` that
  succeeds. Only a successful `Bottom` clears it; `bottom()` hides the button at once only when the
  target was not unconfirmed (a first `Bottom` after a plain swipe up), so the button stays visible
  while away or unconfirmed, also while a retry is out.
- **Retries.** Input held behind a failed `Bottom` stays queued and `Bottom` is tried again after
  `RETRY_DELAYS_MS` = 250 ms, 1 s, 2 s, then every 4 s, while the terminal's host connection is live
  (`HostConnections.hostLive`: the host's current `ActiveHost` is `Connected` with a port, the
  connection `scrollTarget` uses). When it is not live, the input stays held and the next try goes as
  soon as it is live again (the delays then start over). Retrying stops, and the held input is
  dropped, only when the terminal closes: `TargetScroller.close()` from the terminal's `Closed`
  (`sessionState`) and from `retireTerminal` (dismiss); a closed scroller holds nothing and sends
  nothing. Without held input a failed `Bottom` (hiding a terminal) is not retried: the button shows
  and the next input or tap tries again.
- **The button** (`jumpToBottom` → `bottom()`) tries a `Bottom` at once, replacing a waiting retry.
- **Cap.** At most `MAX_HELD_BYTES` = 64 KiB are held (UTF-8 bytes of text, pastes and composer
  submits plus one; a key counts `KEY_BYTES` = 8). An input that would pass the cap is dropped, the
  newest (so what was typed first still goes out whole and in order); `TargetScroller.input` returns
  false and the composer's `sendLine` then keeps its message.
- Tests (each failed against the previous scroller, with only its API stubbed): `TargetScrollerTest`
  `aKeyStaysHeldThroughAFailedBottomAndGoesOutOnceAfterOneSucceeds` (replaces
  `aFailedCallStillReleasesTheTyping`: held through two failed `Bottom`s, out exactly once after the
  third succeeds), `aFailedBottomIsRetriedWithABoundedBackoffWhileInputIsHeld` (sends at 0, 250 ms,
  1.25 s, 3.25 s, then every 4 s; none after `close`), `aFailedDownKeepsTheTypingHeldUntilABottomSucceeds`,
  `aFailedUpLeavesThePositionUnconfirmed`, `heldInputWaitsOutAConnectionDropAndGoesOutWhenItIsBack`
  (no try while not live, however long), `heldInputIsCappedAndTheNewestPastTheCapIsDropped`,
  `closingDropsTheHeldInputAndStopsRetrying`, `theButtonStaysWhileUnconfirmedAndTappingItTriesAtOnce`;
  `HostConnectionsTerminalInputTest`
  `inputHeldBehindAFailedBottomWaitsOutAHostDropAndReachesTheSurvivingMoshTerminalOnReconnect` (no
  try while the host is closed, then the `Bottom` and the key over the new connection, once) and
  `closingTheTerminalDropsItsHeldInputAndStopsRetrying`.

## Tap links and OSC 52 (lane Links)

- **Tap a link to open it.** A single tap on a URL opens it (`Intent.ACTION_VIEW`, through the system
  chooser when no default exists); a tap elsewhere keeps today's behaviour (clear the selection, show
  the keyboard). Links are OSC 8 hyperlinks (`ResolvedRow.links`, from libghostty-vt's
  `hyperlink_uri`) and plain-text URLs detected in Kotlin over the visible rows, **joining wrapped
  rows** (`ResolvedRow.wrapped`) so a URL broken across lines opens whole. Detected schemes: `http`,
  `https` only (no `file:`, `intent:` or `javascript:`). A tapped link flashes an underline for the tap
  (feedback), no confirmation dialog.
- **OSC 52 copy.** A clipboard write from the host (`on_clipboard_write`) sets the Android clipboard
  (`ClipData` labelled `or2`), **on by default** with a Settings switch (`Copy from the host`, default
  on); a read request is never answered (the host never learns the phone's clipboard). Writes are
  limited to 1 MiB and rate-limited to one per 500 ms per terminal (later ones in the window replace
  the pending one). Android 13+ shows its own copy confirmation; or2 adds none. Owner preference over
  the roadmap's per-host opt-in: zero configuration, with the global switch as the off-ramp.
- Tests: Rust tests that an OSC 8 link reaches `ResolvedRow.links` on both transports and that OSC 52
  reaches the observer (base64 decoded, an invalid payload dropped); JVM tests for URL detection
  (wrapped, trailing punctuation, brackets, schemes refused) and the rate limit.

### Links implementation (branch `v011/links`)

- **FFI shape.** The FFI row record is `TerminalRow` (Kotlin's `ResolvedRow` is the app's resolved copy of
  it), so the export is `TerminalRow.links: Vec<CellLink>`, carried into `ResolvedRow.links`. `CellLink`'s
  columns are **inclusive** (`start_column..=end_column`, Kotlin `IntRange`), ascending and not overlapping,
  and a wide character's spacer tail belongs to its head's run; the core validates this
  (`FrameError::LinkRange`). The field is `#[uniffi(default)]`, so Kotlin code that builds a `TerminalRow`
  (fixtures, fakes) needs no change.
- **Rust.** `TerminalEngine::frame` reads links only on changed rows whose libghostty row says it has a
  hyperlink (`Row::has_hyperlink`), then per cell (`Cell::has_hyperlink`) through `Terminal::grid_ref`
  (viewport point) and `GridRef::hyperlink_uri`, growing one reused buffer on `OutOfSpace`; adjacent cells
  with the same URI merge into one run, two different adjacent links stay two runs. The engine registers
  `on_clipboard_write` in `from_terminal`, so a new engine and one restored from a snapshot (mosh's older
  states) both report; libghostty decodes the base64 (an invalid payload never reaches the callback) and
  never forwards a read request (`?`). The engine keeps the newest write (`take_clipboard_write`); it
  passes the first `text/plain` (else `text/*`) representation that is UTF-8 and at most
  `MAX_CLIPBOARD_BYTES` (1 MiB), and drops a clear request (no representation), binary data, non-UTF-8 text
  and anything larger. Every destination (`c`, `p`, `s`, none) counts: the phone has one clipboard.
  `SessionObserver::clipboard_write` (default no-op) is called through `SessionDriver::publish_clipboard`,
  only while `Connected`. The SSH pump takes the write after each output batch; the mosh driver after
  each authenticated datagram (a later datagram may replace the live screen with a restored one).
  `ListenerObserver` forwards it to `SessionListener.on_clipboard_write`; a listener error is ignored.
- **Kotlin, links.** `terminal/TerminalLinks.kt` (`TerminalLinks.at(rows, cell)`): an OSC 8 run under the
  cell first, then a text URL in the cell's logical line (rows joined while the previous one is `wrapped`,
  each character mapped back to its cell). The regex is `\bhttps?://[^\s<>"'`]+` (case-insensitive), then
  trailing `.,;:!?*` are trimmed and a closing `)`, `]` or `}` with no opener inside the URL
  (`(see https://example.org/a)` loses the `)`, `https://example.org/a_(b)` keeps it). **Deviation:** the
  `http`/`https` allowlist applies to OSC 8 targets too (`TerminalLinks.allowed`): `ls --hyperlink` emits
  `file://host/path`, which names a file on the host, and `intent:`/`javascript:` must never come from a
  remote program; a refused OSC 8 target falls back to the text under it. `TerminalView.onSingleTapUp`:
  with no selection, a tap on a link underlines its cells in `accent` (1.5 dp, 300 ms) and starts
  `ACTION_VIEW` with `CATEGORY_BROWSABLE` (Android's own resolver asks when there is no default; no
  handler at all shows a short toast); otherwise today's behaviour (clear the selection, show the keyboard).
  A tap while a selection is shown only clears it, so a link is never followed by accident.
- **Kotlin, OSC 52.** `HostConnections.clipboardWrite` (set by `Or2Application`) receives
  `(terminalId, text)` on the main dispatcher, only from the terminal's current session (a replaced mosh
  attempt is ignored). `terminal/HostClipboard.kt` checks the switch at each write (a pending write is
  dropped if it was turned off meanwhile), drops empty text and text over 1 MiB of UTF-8, and per terminal
  writes the first at once and holds one pending write (the newest) until 500 ms after the last write.
  The write is `ClipData.newPlainText("or2", text)`.
- **Settings.** There was no Settings screen: this lane adds `Destination.Settings` (`settings` in saved
  state), pushed from a new Home top-bar icon (`Or2Icons.Settings`, three sliders, between keys and about),
  with `app/SettingsScreen.kt` (a TERMINAL group with the `Copy from the host` switch row) over
  `app/AppSettings.kt` (stored inverted as `host_copy_off`, so absent means on). The Notify lane's
  `Agent notifications` switch belongs in the same screen; whichever lane merges second adds its row there.
- Tests: Rust `terminal::tests::osc8_hyperlinks_reach_the_row_as_column_runs` (runs, a wide tail, adjacent
  links, a 600-byte URI), `osc52_clipboard_writes_are_decoded_and_bad_ones_dropped` (BEL/ST, newest wins,
  read request unanswered, invalid base64, non-UTF-8, clear, the 1 MiB cap both sides),
  `a_restored_engine_still_reports_clipboard_writes`; `frame::tests::links_must_lie_inside_the_row_in_order`;
  `session::tests::clipboard_writes_reach_the_observer_only_while_connected`; SSH:
  `ssh::pump::tests::output_carries_osc8_links_into_frames_and_osc52_to_the_observer`; mosh:
  `mosh::driver::tests::osc8_links_reach_the_frames_and_osc52_the_observer_as_over_ssh` (fake server);
  FFI: `frame::tests` (links mapped) and `session::tests::clipboard_writes_reach_the_listener`. JVM:
  `TerminalLinksTest` (text URLs, wrapped rows, trailing punctuation, brackets, refused schemes, OSC 8 and
  its allowlist, wide characters), `HostClipboardTest` (window, pending replacement, per terminal, switch,
  cap, UTF-8 length), `AppSettingsTest`, `TerminalGridTest` (links kept and replaced with their row),
  `NavigationTest` (settings) and `HostConnectionsTransportTest` (writes routed by terminal, a replaced
  session ignored). Not covered on the device: the tap itself and the Android clipboard write.

## Gestures and hardware-keyboard shortcuts (lane Gestures)

- **Horizontal swipe** (one finger, horizontal dominant, past a threshold, not while selecting):
  left = next tmux window / herdr tab (`TargetNav::NextWindow`), right = previous. **Two-finger
  horizontal swipe:** the pane in that direction (`Pane { Left | Right }`); **two-finger vertical
  swipe:** next/previous tmux session or herdr workspace (`NextSession`/`PreviousSession`). Pinch
  keeps priority over two-finger swipes (a scale change past its slop makes it a pinch). A `Shell`
  target ignores these swipes. Implementation: tmux through exec (`next-window`, `previous-window`,
  `select-pane -L/-R/-U/-D`, `switch-client -n/-p` with the client's tty found by `list-clients`);
  herdr through its API (`tab.focus` on the neighbouring tab from `tab.list`, `pane.focus_direction`,
  `workspace.focus`).
- **Hardware keyboard** (an attached keyboard, not the IME): `Ctrl+Shift+1..9` switch to the n-th open
  terminal (Home's order), `Ctrl+Shift+W` close the terminal, `Ctrl+Shift+V` paste, `Ctrl+Shift+C` copy the
  selection, `Ctrl+Shift+Enter` open the composer, `Ctrl+Shift+/` show a compact shortcuts sheet. Every
  other key goes to the terminal as today. Matched by key code, not by the character (layouts differ).
- Tests: JVM tests for the gesture classifier (thresholds, pinch priority, selection) and the shortcut
  table; Rust tests for the tmux commands and the herdr calls against fixtures.

### Gestures: implemented (branch `v011/gestures`)

**Rust.** `or2_core::host::{TargetNav, NavDirection}` and `HostHandle::navigate(target, pane_id,
nav)`, a host query like `focus_herdr_pane` (`HostCommand::Navigate`, bounded by `QUERY_TIMEOUT`).
The handle validates first (`InvalidName` for a bad tmux/herdr name or `pane_id`), then a `Shell`
target is `Ok(())` at once, before the connection state is even looked at; nothing is sent. The
FFI wraps it as `HostConnection.navigate` with `uniffi::Enum` copies of both enums; the contract
probe host answers every move `Ok`, except one from a `pane_id` its view lacks (`PaneNotFound`).

- **tmux** (`or2_core::tmux::navigate`, over exec with the probed tmux path): `next-window -t
  =<s>`, `previous-window -t =<s>`, `select-pane -L|-R|-U|-D -t =<s>:`, and `switch-client -c
  <client> -n|-p`. Targets are `=name`, tmux's exact match, so `main` never reaches `main2`.
  *Which client* is the terminal's: `list-clients -F
  '#{client_activity}:#{client_session}:#{client_name}'`, the most recently active client showing
  the target session (superseded: the client the terminal's attach recorded under its client id,
  and the memory below is per terminal; see "Fix: navigation by terminal identity"). *Decision:* the client's **name** rather than `#{client_tty}`: they are the
  same for a terminal client, and the name also works for a client without a tty. *Decision:* after
  a session move the terminal's client shows another session, which the target's name no longer
  finds, so the host connection remembers the switched client per target name
  (`tmux::NavClients`, on `SshHost`); later window and pane moves act on the session that client
  shows now (one `list-clients` more), session moves keep switching it, and a client that is no
  longer listed (the terminal reattached) is forgotten. Before any session move a window or pane
  move is one exec. A session move with no client attached to the target is `CommandFailed`
  (superseded: a session move whose client is not identified is nothing to do, see "Fix: exact tmux
  client identity").
  "Nowhere to go" (`no next window`, `no previous window`, `can't find next|previous session`) is
  `Ok`; tmux itself wraps windows and sessions around.
- **herdr** (`or2_core::herdr::navigate_in`, the socket from the connection's `Directory` with the
  same one-time rediscovery as a focus): a window move reads `workspace.list` (the focused
  workspace and its `active_tab_id`), then `tab.list { workspace_id }`, and `tab.focus`es the next
  or previous tab by `number`, wrapping; a pane move is one `pane.focus_direction { direction,
  pane_id }` (herdr answers `no_neighbor` as a success); a session move reads `workspace.list` and
  `workspace.focus`es the neighbour by `number`, wrapping. One tab or workspace, or none focused (an
  empty session), is `Ok` with nothing sent. **Finding:** herdr 0.9.3 answers **one request per
  connection** (a second request on the same stream gets a broken pipe), so each request is its own
  `wire::call`; the live test caught it. Results are parsed with the generated
  `success_response::ResponseResult` (unknown fields and agent statuses tolerated).
- `pane_id`: herdr's pane to move from, `None` for the focused one; tmux ignores it.

**Kotlin.**

- `SwipeClassifier` (plain JVM): one finger decides once it has moved the touch slop (Euclidean, so
  no later than the platform's scroll): horizontal at least twice the vertical makes a horizontal
  swipe, anything else leaves the touch to the terminal; it fires once at 56 dp. A second finger at
  any time before a swipe fired (also after the first began to scroll) makes it a two-finger swipe
  from the fingers' centre, firing at 56 dp along a dominant (2×) axis. **Pinch priority:** the span
  changing past the pinch slop (2 × touch slop, `ScaleGestureDetector`'s own) ends classification,
  and `onScaleBegin` cancels it; since the scale detector sees every event first, a pinch is never
  also a swipe. A third finger or a lifted one ends classification. No swipe starts during a
  selection, and one is cancelled when a long press begins one. Once a touch is a swipe the view
  sends its `GestureDetector` a cancel (no scroll, tap or long press) and consumes the rest of the
  touch. In `TerminalView.onTouchEvent` this is one call, `swipeTouch`, after the pinch handling
  and before the gesture detector; the one-finger vertical path (`onScroll`, `scrollPixels`, fling)
  is untouched.
- Mapping (`swipeNav`): left `NextWindow`, right `PreviousWindow`; two fingers left/right
  `Pane { Left | Right }` (*decision:* the direction the fingers move, read literally); two fingers
  up `NextSession`, down `PreviousSession`. `SessionScreen` runs it through
  `HostConnections.navigate(terminal, nav)` with a haptic tick (`GestureThresholdActivate`). For a
  `Shell` target it passes no `onSwipe`, and the view then never classifies: a shell's touches
  (including a two-finger scroll) behave exactly as before. `navigate` uses the host's live connection, runs one move at a time per
  terminal (a `Mutex`: herdr's next tab is read, then focused), passes **`pane_id = null`** (herdr's
  focused pane is what a herdr client shows; the pane the terminal opened on may no longer be
  focused) and swallows failures (a gesture has no error UI; it returns `false`).
- Shortcuts (`terminalShortcut`, by key code, exactly Ctrl+Shift, never with Alt or Meta, never for
  `FLAG_SOFT_KEYBOARD` events): checked at the top of `TerminalView.handleKey`, which
  `dispatchKeyEventPreIme` and `onKeyDown/Up` call; a shortcut's press acts once (repeats ignored)
  and its release is consumed too. `Ctrl+Shift+1..9` selects `open[n-1]` (Home's order: the
  holder's terminal list); `W` dismisses the terminal (disconnecting it if it runs) and returns
  Home; `V` is the toolbar's paste (multi-line confirmation included); `C` copies the selection
  (nothing without one); `Enter` (and numpad Enter) toggles the composer; `/` opens
  `ShortcutsSheet` (an `Or2Sheet` with two compact grouped cards, keys in `MonoSmall`, actions in
  muted `Secondary`, listing the keyboard shortcuts and the gestures). While the composer has the
  keys the same table applies through `onPreviewKeyEvent`, except `V` and `C`, which stay the text
  field's; closing the composer with `Ctrl+Shift+Enter` gives the keys back to the terminal.
- `HostPort.navigate` (API 14) on `NativeHostPort` and the test fakes.

**Tests.** Rust: `tmux` unit tests (command rendering with exact targets, client parsing, the
terminal client's choice, "nowhere to go", the session-move memory and its forgetting, against a
scripted exec host); `herdr::navigate` unit tests (a scripted herdr, one request per connection:
neighbour by number, wrapping, pane directions with and without `pane_id`, `no_neighbor`, nothing to
do, `PaneNotFound`, a dead cached socket rediscovered once, unreadable answers); `host` unit test
(validation, `Shell` short-circuit, the command and its reply); FFI mapping test;
`tests/host_nav.rs` (sshd + tmux end to end, see build.md); `herdr_live.rs` navigation test against
a real isolated herdr. JVM: `SwipeClassifierTest` (thresholds, slop, dominance, two fingers, pinch
priority, lifted/third finger, cancel, mapping), `TerminalShortcutsTest` (the table, modifiers),
`HostConnectionsNavigateTest` (shell ignored, null pane, failures, disconnected host, one move at a
time), `HostContractTest.navigationCrossesTheFfi...` (the real FFI with the probe host). No device
test was added (the lane runs without the phone).

**Open.** A herdr pane move or tab move changes herdr's focus behind the connection's `FocusGate`,
which may still remember an acknowledged focus for up to 2 s; a terminal opened on that same pane
within that window could be answered from memory although the focus moved. Rare (a swipe and an
open of the previous pane within 2 s), not handled. (Two or2 terminals on the same tmux target
used to share the session-move memory; fixed below.)

### Fix: navigation by terminal identity (branch `v011/fix-nav-identity`)

Review finding (Codex, P2): two terminals for the same tmux target shared one remembered client, so
a gesture on one could move the other. Rust remembered the switched client by target name,
`terminal_client` preferred it whoever asked, and Kotlin dropped the terminal's identity at the FFI.
The SSH/mosh overlap of the background swap had the same ambiguity with one `ActiveTerminal`.

**Identity.** Every tmux terminal session gets a **client id** when it is opened
(`HostHandle::open_terminal_within`: `tmux::new_client_id()`, 32 random lowercase hex digits, unique
across connections, processes and devices sharing a tmux server), kept in the session's shared
state: `SessionHandle::client_id()` / `SessionDriver::client_id()`, FFI `Session.client_id()`
(`None` for shell and herdr). An SSH session and the mosh session that replaces it are two sessions
with two ids.

**The attach records its client** (only on a tmux that takes `set-option -F`, 2.6 and later; see
"Fix: exact tmux client identity"). `tmux::attach_command(tmux, name, Some(id))` is `tmux -u
new-session -A -s <name> ; set-option -s -F @or2-client-<id> '#{client_name}'`: the second command
of the same command list runs as the client that just attached, so the tmux server option
`@or2-client-<id>` holds exactly that terminal's client name. The same argv goes to `mosh-server`
for a mosh terminal (the live mosh test checks the record). *Decision:* identify the client at the
source rather than guess among the target's clients (the most recently active one is not the one
under the user's fingers: a gesture is not tmux input, so it leaves no activity). A server user
option was chosen because it is readable from any `list-clients` format and costs no extra exec.

**Navigation.** `HostHandle::navigate(target, pane_id, nav, client_id)`; FFI
`HostConnection.navigate(..., client_id)`; a malformed id is `InvalidName` (it becomes part of an
option name). `tmux::navigate` lists clients with `#{client_activity}:#{@or2-client-<id>}:
#{client_session}:#{client_name}`; the client whose name equals the recorded one is the terminal's
(`TmuxClient.recorded`). The client of a session move is, in order: the recorded one; else (a tmux
that could not run `set-option -F`) the one this terminal's earlier switch remembered, while it is
listed; else the most recently active client on the target that no other terminal has claimed
(superseded: only the recorded one, see "Fix: exact tmux client identity").
`NavClients` is keyed by the client id (`NavKey::Client`), so two terminals on one target are two
entries; a caller without an id keeps the old per-target key (`NavKey::Target`). Window and pane
moves still cost one exec until this terminal's first session move, then one `list-clients` more
(acting on the session its recorded client shows; a remembered tty that is no longer listed is
re-resolved from the record, or forgotten).

**Release.** When a tmux terminal's session ends (either transport, any reason), a drop guard
(`SshHost::tmux_release`) removes its `NavClients` entry and, unless the connection is closing,
runs `tmux -u set-option -s -q -u @or2-client-<id>` in the background (best effort, 5 s bound,
never delaying the session's `Closed`). A connection that closes, or a host that is unreachable,
leaves the option behind: a few bytes on the tmux server, never consulted again (no other terminal
has that id).

**The swap.** Kotlin's `HostConnections.navigate(terminal, nav)` passes
`terminal.handle.value?.clientId()`: the session the terminal shows. During the SSH/mosh overlap
that is the SSH session until the swap publishes the mosh one, then the mosh one: always the client
on screen, with no transfer state. *Not changed:* the mosh client attaches to the terminal's
target, so a terminal whose SSH client had been switched to another session shows the target again
after the swap (as before this fix).

**Decision: the host screen keeps opening a new terminal for a tmux target that is already open.**
With each terminal's moves now its own, two terminals on one session are consistent, and reusing
would be wrong after a session move: the open terminal may show another session than the one the
user tapped. The inbox and reattach keep reusing (their target identifies one herdr pane or the last
terminal). Unchanged code, so no new test. *(Superseded by the owner, 2026-10-02: the session picker
now reuses an open tmux or herdr session's terminal; see "One terminal per session" under "Home: the
host card's two actions".)*

**Tests.** Rust: `tmux` unit tests (the attach and release commands, client ids and their
validation, the format with the recorded field, the choice order, two terminals on one target
alternating session and window moves, re-resolution after a reattach, release, the fallback never
taking another terminal's client); `host` unit tests (`each_tmux_terminal_gets_its_own_client_id`,
`navigate` validating and carrying the id); `tests/host_nav.rs`
`two_terminals_on_one_tmux_session_each_move_only_their_own_client` (two real SSH tmux clients on
one session, the second made the more recently active, alternating session moves and window moves
each move only their own client; closing one releases its record); `tests/host_mosh.rs` (a mosh
tmux terminal records its client and releases it on close). JVM: `HostConnectionsNavigateTest`
(two terminals on one target pass their own ids; across the swap the SSH id until the mosh session
replaces it, then the mosh id), `HostContractTest.navigationCrossesTheFfi...` (`Session.client_id`
through the real FFI, a malformed id refused). The live two-terminal test fails with the identity
withheld (`None`: "A after its next session: or2-a, expected or2-b"), and the JVM tests fail with
`navigate` passing `null`.

### Fix: exact tmux client identity, or no session move (branch `v011/fix-2`)

Codex v0.1.1 fix check, P2: the old-tmux fallback guessed. Without a record, a terminal's first session
move switched the most recently active unclaimed client on the target, which need not be its own; and
the `set-option -F` step was appended to every attach, so a tmux that cannot parse it would reject the
whole command list, attach included.

**The step only where tmux takes it.** The program probe now also runs `tmux -V` (only when tmux is
found, standard input closed, errors silenced) inside `PROBE_SCRIPT`, printing
`or2:tmux-version:<answer>`: no extra exec or round trip, cached with the program probe for the
connection's life. `probe::parse` sets the core-only `HostCapabilities.tmux_records_clients`
(`tmux::records_clients`; not exported over the FFI, `API_VERSION` stays 14). `set-option -F` came in
**tmux 2.6** (tmux CHANGES, "2.5 to 2.6": "add -F flag to expand them in option values");
`#{client_name}` (2.4), user options (1.8) and server options (1.2) are older, so 2.6 is the bar
(`RECORDS_CLIENTS_SINCE`). Accepted: `tmux X.Y` with any suffix (`3.3a`, `3.0-rc5`) at 2.6 or later,
`tmux next-X.Y` the same, `tmux master`, and `tmux openbsd-X.Y` from OpenBSD 6.3 (its base tmux is
newer than 2.6). Anything else, an older version, or no answer: `false`.
*Decision:* a version gate rather than probing the option itself. A probe of `set-option -F` needs a
tmux server (starting one, or touching the user's, as a side effect of connecting), and an old tmux
client rejects an unknown flag before it ever reaches a server while tmux 3.x defers parsing to the
server, so a server-less parse check cannot tell the two apart. `tmux -V` is answered by the client
alone on every version, costs nothing extra in the probe, and a wrong "no" only disables session
moves, never breaks an attach. A tmux that passes the gate but still fails the step at run time
(after the attach, as the attached client) only shows tmux's message on its status line; nothing is
recorded and the identity is unknown, handled as below.
- `terminal_session::plan` (SSH and mosh alike) passes the client id to `tmux::attach_command` only
  when `tmux_records_clients`; otherwise the attach is the plain `tmux -u new-session -A -s <name>`.
  The session still has its client id (`Session.client_id()` is unchanged), and Kotlin still passes it.
- The release (`release_tmux_client`) runs `set-option -u` only for a tmux that records; the
  connection's `NavClients` entry is always dropped.

**No guess: an unknown client is nothing to do.** `tmux::navigate` returns `NavOutcome { Ran,
ClientUnknown }`. A session move (`switch-client`) needs the terminal's own client, the one listed
with its recorded name; with no client id (none given, or one dropped because the host's tmux cannot
record), or nothing recorded under it (no server, the record gone), it is `ClientUnknown`: nothing is
switched and no error. `HostHandle::navigate` (so `HostConnection.navigate`) maps it to `Ok(())`, the
FFI's existing "nothing to do" (as for a `Shell` target or a move with nowhere to go), so Kotlin needs
no change: no move, no error (a gesture has no error UI anyway). The "most recently active unclaimed
client" guess, `NavKey::Target` and `NavClients::others` are gone; `NavClients` is keyed by client id
only, and a remembered client whose record is no longer listed is forgotten (window and pane moves
then act on the target). Window and pane moves act on a session, not a client, and are unchanged:
on the target, or on the session the terminal's recorded client was switched to.

**Tests.** Rust: `probe::tests::tmux_records_clients_only_from_a_version_that_can`; `tmux` unit tests
`only_a_tmux_that_takes_set_option_f_records_clients` (versions), `clients_parse_and_only_the_recorded_one_is_the_terminals`,
`a_session_move_switches_the_recorded_client_and_later_moves_follow_it`,
`moves_with_nowhere_to_go_succeed_and_real_failures_are_reported`,
`without_a_record_a_session_move_switches_nothing` (no id: nothing runs; an id with nothing recorded:
one `list-clients`, no `switch-client`, though the target has clients; window and pane moves still run),
and the two-terminal test keyed by id; `tests/host_nav.rs`
`an_old_tmux_attaches_plainly_and_its_session_moves_do_nothing` (a `tmux` wrapper first on the sshd
sessions' `PATH` answers `-V` with `tmux 2.5` and refuses any command list with `set-option -F`, like an
old tmux client: the attach is the plain argv and attaches, session moves return `Ok` and switch
nothing, window and pane moves act on the target, nothing ever runs `set-option` or `switch-client`;
against the previous code the attach never attached), the first live test now passes the terminal's
id (and checks that a session move without one switches nothing), and the live two-client test still
passes.

## Home: the host card's two actions (owner feedback)

Owner feedback of 2026-10-02: a tap on a Home host card did two things at once, pushing the host screen
and opening the session picker over it (by itself, once per connection: `pickerOffered`). Both views
stay; each now has its own button. Android only: no FFI, schema or Rust change.

- **Card body:** opens the host screen, nothing else. A host that is not connected still starts its
  connection on the tap (`tapConnects`: it has a key, nothing is connecting or connected, no unlock is
  running), as before. The host screen's picker opens only from its own **Open a session**; it never
  opens by itself, on arrival or when the host connects. `HostScreen` loses `pickerOffered` and
  `setPickerOffered`, and `Or2App` its `offered` set.
- **Session button** (`host-session:<id>`, described "Open a session on <host>"): replaces the chevron
  and opens the session picker over Home without pushing the host screen. It starts the connection on
  the same rule as the body (the same unlock, biometric included) and the sheet shows at once. Until the
  host is connected the sheet shows a `PickerGate` (`host/PickerGate.kt`, `pickerGate(host, state,
  unlocking, busy)`) instead of its lists: `Connecting` (the card's own progress line, spinner unless a
  host-key decision waits, whose dialog shows over the sheet as on any screen but the host's own) or
  `Stopped` (the reason, a failure in `danger`, the per-address detail, and one `GateAction`: **Retry**
  for a failed or asleep host, **Unlock and connect** for one that is simply not connected, **Select a
  key**, which opens the host form, for one without a key; off while another unlock runs). Once
  connected the same sheet shows the lists. Long press keeps the options sheet.
- **One picker:** `SessionPickerSheet` is shared, with an optional `gate`. Its data and actions come from
  one place, `app/HostPicker.kt`: `pickerSource(active, connections, openTerminal)` (state, capabilities,
  the tmux listing and its refresh, the UDP line, and `open(target)` through `Or2App.openTerminal`,
  that is `TerminalActivations.launchOpen`) feeds both the host screen and `HomePickerSheet`. A choice
  closes the sheet and opens the terminal on the same path from either place; Recent resumes through
  the same `resumeTerminal`. The sheet over Home is `Or2App` state (`homePicker`, saved) that belongs
  to Home: leaving Home closes it, and dismissing it changes nothing else (a connect already started
  carries on, shown on the card).
- **One terminal per session** (supersedes the M3 decision above for the picker): every picker choice
  goes through `TerminalActivations.open`, whose rule lives in `TerminalActivations.reusable(hostId,
  target)`. `Tmux(name)` reuses the open terminal on that host and session; `Herdr(session, null)` the
  open herdr terminal on that session, whatever pane it was opened on (a pane-less one first), and a
  pane terminal reused this way is focused again first (`reuse`, as from the switcher); `Herdr(session,
  pane)` goes the inbox tap's way (`openAgent`: focus, then reuse or open); `Shell` always opens a new
  terminal. A terminal that is closed, retired or being closed is never reused
  (`HostConnections.openTerminals(hostId, matches)`, which `findOpenTerminal` now uses too). A session
  moved to another tmux session in an open terminal is still that terminal's target: the picker brings
  it back as it is.
- **Close session** (the terminal's sessions sheet): one action in every state. An open terminal is
  disconnected (`disconnectTerminal`, so Rust stops its mosh server and the ledger is cleared on its
  `Closed`, which still reaches the listener after the dismissal) and dismissed (`dismissTerminal`)
  together, a closed one only dismissed, then the app returns to Home (the calmer landing than the
  next terminal). The closed strip's **Close** remains for a session that closed by itself.

**Tests.** JVM: `PickerGateTest` (connected: no gate; the progress lines; failure, asleep, not
connected and keyless with their actions; busy disables), `HomeModelTest.aTapOrTheSessionButton...`
(`tapConnects`), `TerminalActivationsTest.thePickerReusesAnOpenTmuxOrHerdrSessionAndAlwaysOpensANewShell`
and `aClosedOrClosingTerminalIsNotReusedByThePicker`, `HostConnectionsMoshServerTest.closingAnOpenTerminalInOneTapStillStopsAndForgetsItsServer`.
Device (compiled in the gate; run on the phone): `HostScreenUiDeviceTest` (the picker never opens by
itself; every picker test opens it with **Open a session**; the gate's progress, reason, detail and
action), `HomeUiDeviceTest.theCardBodyOpensTheHostAndItsSessionButtonOpensThePickerConnectedOrNot`
(48 dp target, description), `HomeSessionPickerDeviceTest` (the whole `Or2App` over fakes: body to the
host screen with no picker; button to the picker over Home, a choice opening the terminal; a host that
is not connected connects and shows progress, then the lists; a failure with **Retry**; dismissing
leaves Home), `EntryUiDeviceTest.closeSessionInThePanesSheetEndsAnOpenTerminalInOneTapAndReturnsHome`.

# v0.1.2: blue arrow pad, route-1 scroll-to-bottom, tap to click (API 15)

Owner feedback on the phone, v0.1.1 (branch `v012/pad-scroll-click`):

1. *"The arrow pad keys need to be blue, similar to its icon: the background and the arrow keys are
   the same colour."* Sizes, shapes, positions and the panel-less cluster stay.
2. *"When I scroll up in the terminal to read the agent output and then want to come back to the
   composer, the down arrow popup does nothing on click."* That was a herdr target. herdr tracks the
   mouse, so the swipe took route 1 (wheel events) and herdr scrolled itself and drew its own "Jump to
   bottom" banner; taps were not forwarded as clicks, so the banner did nothing, and or2's own
   scroll-to-bottom button never showed, because route 1 tracked no position.

## FFI (API 15)

| Export | Change |
|---|---|
| `Session.mouse_click(column: u16, row: u16) -> Result<(), SessionError>` | New. A left-button press and release at the viewport cell (clamped to the grid), encoded by libghostty-vt's mouse encoder in the terminal's own tracking mode and format (SGR, UTF-8, urxvt, X10; X10 tracking, DECSET 9, gets the press alone). Nothing is sent when the program does not track the mouse. `NotConnected`/`Closed` like `scroll`. Core: `SessionHandle::mouse_click`, `Command::MouseClick { column, row }`, `TerminalEngine::mouse_click`. |

`API_VERSION` = 15. Both transports handle the command the same: the SSH pump writes the bytes to the
channel, the mosh driver to its user stream (`Session::send_input`); a click waiting behind a submit's
Enter keeps its place like any input (`SubmitSequencer` orders it). The contract probe echoes it on row 3
(`click <column> <row>`).

## Arrow pad colours

- Two tokens (`Or2Colors`, docs/ui.md): **`padKey`** `#38425F`, `accent` at 24 % composited over the
  terminal's default background `#1E1E2E`, stored opaque so terminal text never shows through a key;
  **`padKeyEdge`**, `accent` at 55 %.
- Each pad key: `padKey` fill, `accent` glyph, `padKeyEdge` hairline (was `surface` with a `divider`
  hairline and a `text` glyph). **Enter**, the primary key: `accent` fill, `background` glyph (was
  `surfaceTrack`), as the composer's send button.
- The extras pill: `background` with the `padKeyEdge` hairline (was `divider`); its keys' labels, `Alt`'s
  included, are `accent` (were `text`); a latched `Alt` keeps its `accentMuted` fill.
- Contrast (`ThemeTest.arrowPadKeysAreBlueAndStandOutFromTheTerminal`): `padKey` against the terminal
  1.65:1 (`surface` was 1.1:1) and visibly blue (its blue channel 0.15 above red); the hairline 3.3:1 on
  the terminal; `accent` on `padKey` 4.7:1; `background` on `accent` above 7:1; `accent` labels on
  `background` above 7:1; `accent` on `accentMuted` above 4.5:1.

## Route 1 on a tmux or herdr target marks it away

- `TargetScroller.wheeled(rows)`: `TerminalView.scrollRows` calls it after a route-1 swipe (wheel events
  through `ViewportScroll::Wheel`) when the terminal has a target scroller (tmux and herdr terminals only;
  a shell has none, so a shell with vim or any mouse-tracking program keeps route 1 exactly as before, no
  button). A swipe **up** sets `unconfirmed` (away, how far unknown): the button shows and nothing is sent
  (the wheel events already went). A swipe **down** changes nothing, even if it may have reached the
  bottom: only a `Bottom` that succeeds clears it.
- From there the existing rules apply unchanged: the button sends `TargetScroll::Bottom` through
  `scroll_target` (herdr: `pane.scroll` to offset 0 on the focused pane; tmux: `send-keys -X cancel`,
  where tmux's "not in a mode" is success); any key, text, paste or composer submit first sends `Bottom`
  and goes out once it has succeeded; hiding the terminal sends `Bottom` (`leave`). The button stays
  visible while that `Bottom` is out and disappears when it succeeds.
- A wheel swipe up while a `Bottom` is in flight: that `Bottom`'s success no longer clears `unconfirmed`
  (`wheelsUp` counts wheel swipes up; a `Bottom` confirms only when none came after it was sent).
- Input typed while a `Bottom` is already out (and nothing else is queued) waits for that `Bottom` instead
  of queueing a second one: an unconfirmed target stays `away` until its `Bottom` returns, which used to
  send one more `Bottom` per input typed meanwhile.
- **herdr finding** (herdr 0.9.3, live test
  `herdr_live::bottom_returns_a_pane_herdr_scrolled_by_wheel_events_to_live`): an isolated herdr session
  with a real herdr client attached (inside a private tmux server, so the test can type into it), `seq 1
  500` in the pane, SGR wheel-up events typed into the client (`\e[<64;60;10M`, three lines per event)
  scroll the pane: `pane.get` reports the same `scroll.offset_from_bottom` as `pane.scroll` sets (wheel
  scrolling and the API share one per-pane offset, not a per-client view). `scroll_pane_in(…, None,
  Bottom)` (the app's call: the focused pane, no kept offset) then returns it to `offset_from_bottom`
  0 and the client shows the live screen again. Nothing else is needed.
- *Decision:* clicks (below) are pointer input like the wheel: they act on what is shown, so they are
  never held behind a `Bottom` (a tap on herdr's own "Jump to bottom" banner works as herdr means it).
  A target returned to its bottom that way still shows or2's button until a `Bottom` succeeds (the
  next key or a tap on it): or2 cannot see herdr's banner.

## Taps become mouse clicks

`TerminalView.onSingleTapUp` follows `tapAction(selecting, link, mouseTracking)` (`terminal/TerminalTap.kt`),
in this order:

1. A selection is shown: the tap clears it (a link under it is never followed, no click is sent); without
   mouse tracking the keyboard also opens, as before.
2. A link under the tap opens (as in v0.1.1, also while the program tracks the mouse).
3. `TerminalModes.mouse_tracking` is on: `Session.mouse_click` at the tapped cell; the keyboard does not
   open (the toolbar's keyboard key does).
4. Otherwise the keyboard opens, as before.

A long press still selects; swipes, flings and pinches are unchanged.

## Tests

- Rust: `terminal::tests::a_click_is_a_left_press_and_release_at_the_tapped_cell_in_sgr` (press `M`,
  release `m`, 1-based cell, clamping, normal/button/any-event tracking),
  `a_click_uses_the_x10_format_when_no_extended_format_is_on` (release as button 3; DECSET 9 sends the
  press alone), `a_click_without_mouse_tracking_sends_nothing`;
  `ssh::pump::tests::a_click_is_written_only_while_the_program_tracks_the_mouse` (SGR and X10 bytes on the
  channel, nothing without tracking); `mosh::driver::tests::a_click_reaches_the_server_only_while_the_program_tracks_the_mouse`
  (fake server, paused clock); `mosh::ghostty::tests::mouse_modes_from_the_server_reach_frames_and_wheel_scrolls`
  (a click through mosh-relayed modes, nothing once they are off); FFI
  `session::tests::a_mouse_click_reaches_the_driver_as_its_cell`; the herdr live test above.
- JVM: `TerminalTapTest` (precedence), `TargetScrollerTest` (a wheel swipe up marks away and sends
  nothing; down keeps it; the button's `Bottom` and only its success clears it; a failed one keeps the
  button; input held behind `Bottom`; a wheel during an in-flight `Bottom`; hiding sends `Bottom`; a
  closed scroller ignores the wheel), `ThemeTest.arrowPadKeysAreBlueAndStandOutFromTheTerminal`,
  `SessionContractTest` (the click crosses the real FFI to the probe), `NativeContractTest` (API 15).
- Device (compiled in the gate, not run here): `TerminalChromeDeviceTest.thePadKeysAreBlueAndNotTheTerminalBackground`
  (pixels of the keys are `padKey`, not the terminal background or `surface`; Enter is `accent`),
  `aWheelSwipeOnAHerdrTargetShowsTheButtonAndTheButtonSendsBottom`, `aTapIsAClickWhileTheProgramTracksTheMouse`;
  `NativeDeviceTest` (API 15).
- Gallery: `terminal-arrowpad-text` (the pad over a terminal full of text) and `terminal-herdr-wheel` (a
  herdr target after a route-1 swipe up, the button showing); `am start -n
  io.github.code_akram.or2/.gallery.UiGalleryActivity --es screen terminal-arrowpad-text` (or
  `terminal-herdr-wheel`).

**Open.** A wheel swipe goes to the pane under the finger, `Bottom` to the focused pane (herdr) or the
session's active pane (tmux): with several panes, scrolling one that is not focused leaves it scrolled
after `Bottom`. tmux with `mouse on` running a mouse-tracking program in the pane passes the wheel to
that program; the `Bottom` sent afterwards is then a harmless `-X cancel` ("not in a mode").

# v0.1.2: reply from a notification, image paste (API 16)

Owner decision of 2026-10-02: after the v0.1.2 polish, the next two features are replying to an agent from its
notification and pasting images to an agent. One FFI bump, **`API_VERSION` = 16**, covers both lanes.

## FFI (API 16)

| Export | Lane |
|---|---|
| `HostConnection.reply_to_pane(session: Option<String>, pane_id: String, agent: AgentIdentity, text: String) async -> Result<ReplyRoute, HostError>`: send `text` to that agent in a herdr pane and submit it, with no terminal open (`agent` added by the review fix, see "Fix: a reply reaches only its agent, never a shell") | Reply |
| `ReplyRoute { Prompted, Typed }`: which herdr path carried it (below) | Reply |
| `AgentIdentity { terminal_id: String, agent: Option<String>, name: Option<String>, session: Option<AgentSession> }`, `AgentSession { kind: String, value: String }`, `HerdrAgent.terminal_id: String` and `HerdrAgent.reply_identity: Option<AgentIdentity>`: the agent instance a notification is about (review fixes; see "Fix: a reply names its agent instance") | Reply |
| `HostConnection.upload_image(bytes: Vec<u8>, extension: String) async -> Result<String, HostError>`: write the bytes over SFTP to the host's image directory and return the remote absolute path | Paste |

`HostError` gains what each path needs (e.g. `SftpUnavailable` when the server has no SFTP subsystem, and
`TooLarge`). Both calls run on the host's existing SSH connection (no new connection), bounded by the query
timeout.

## Reply from a notification (lane Reply)

- **The notification** (v0.1.1's agent notification) gains a **Reply** action with an Android `RemoteInput`
  ("Reply to <agent>"), next to the tap that opens the pane. No other actions: approve or deny keys differ
  per agent, so they are not guessed.
- **The path in Rust** (`reply_to_pane`):
  - When the agent is **not blocked**, it goes through herdr's `agent.prompt` (target: the pane id), which
    submits like the agent's own input: `Prompted`.
  - herdr refuses `agent.prompt` while the agent is **blocked** (`agent_blocked`), and blocked is the usual
    case for a notification. Then the text is typed into the pane (`pane.send_text`), then, after the same
    short pause the composer's `submit_text` uses, Enter (`pane.send_keys`, `Enter`): `Typed`. This is the
    composer's submit, through herdr instead of a terminal. *(Superseded: the pane is checked, then the text and
    Enter go in one `pane.send_input`; see "Fix: a reply reaches only its agent, never a shell".)*
  - A text with several lines is sent as typed (no confirmation is possible from a notification). The text
    is limited to 4 KiB.
- **The path in Kotlin:**
  - A `BroadcastReceiver` (explicit, not exported) receives the RemoteInput and finds the host's live
    connection. A reply to a host that is not connected says so in the notification (`Not sent: <host> is
    not connected`), with no reconnect from the background.
  - It calls `reply_to_pane`. On success the notification is updated to `Sent` with the reply quoted
    (`Android's MessagingStyle` reply history) and stops alerting. The next Blocked/Done edge posts a fresh
    one, as before.
  - On failure it shows the reason and keeps the Reply action.
  - The PendingIntent is immutable except for the RemoteInput's fill-in (`FLAG_MUTABLE` is required for
    RemoteInput: restrict it with an explicit component and no other extras trusted from the fill-in).
- **Privacy:** the reply text is never logged and never kept after it is sent.
- **Tests:** Rust against the herdr fake (`Prompted` for a working or idle agent, the `agent_blocked`
  refusal leading to `Typed` with text, then the pause, then Enter, an over-long text refused, a vanished
  pane `PaneNotFound`) and the live herdr suite (a reply reaches an isolated pane's input). JVM: the receiver
  (not connected; success updates the notification; failure keeps Reply; nothing logged). Device (compile):
  the action exists with a RemoteInput.

**Implemented (lane Reply).**

- **Rust** *(the typed path, the PendingIntent and the receiver below are superseded by "Fix: a reply reaches
  only its agent, never a shell")*. `herdr::reply_in` (`core/or2-core/src/herdr/reply.rs`) sends `agent.prompt`
  (target: the pane id, no wait) and returns `Prompted`. On `agent_blocked` **or `agent_not_ready`** it sends
  `pane.send_text`, sleeps `submit::SUBMIT_ENTER_DELAY` (100 ms, the composer's), then `pane.send_keys
  ["Enter"]`: `Typed`. On
  `agent_not_found` it is `PaneNotFound` and nothing is typed. Every request uses the connection's `Directory`
  and the scroll module's one-request helper (now `call`/`call_raw`), so a stale cached socket is rediscovered
  once. `HostHandle::reply_to_pane` validates first: names as for terminal targets, an empty text is
  `InvalidName` (Enter alone could answer a dialog), and more than `MAX_REPLY_BYTES` (4096 bytes of UTF-8) is the
  new `HostError::TooLarge`. Then `HostCommand::ReplyToPane` goes to the connection, which waits only for the
  program probe (as the other herdr calls do), and the call is bounded by `QUERY_TIMEOUT`. FFI:
  `HostConnection.reply_to_pane`, `ReplyRoute { Prompted, Typed }`, `HostError::TooLarge`. The contract probe
  answers `Typed` for its blocked agent `w1:p1`, `Prompted` for `w1:p2` and `w2:p1`, and `PaneNotFound` for any
  other pane.
- **Deviation: `agent_not_ready` is typed too.** herdr 0.9.3 refuses `agent.prompt` with `agent_not_ready` ("not
  an active named agent") for every agent it did not start itself with `herdr agent start`. That includes reported
  agents and Claude Code panes detected through herdr's hooks. Without the fallback, a reply to a Done or idle
  agent of that kind would fail. `agent_not_found` (no agent in the pane, or no pane) is the one refusal that
  never types: a reply must not run as a command in a shell.
- **Kotlin.** Every agent notification carries a **Reply** action:
  - It has a `RemoteInput` (`Reply to <agent>`), `SEMANTIC_ACTION_REPLY` and no generated replies.
  - Its PendingIntent is a mutable broadcast with an explicit component: `notify.AgentReplyReceiver`, declared
    `exported="false"`.
  - The pane is named only in the intent's data (`or2-agent-reply:<tag>`), which a fill-in cannot change. The
    title, status and host in the extras are used only for display. A fill-in's extras never override ours.
  - The receiver (`goAsync`) hands the alert and the RemoteInput text to `AgentReplies` on the main dispatcher.
  - `AgentReplies` calls `HostConnections.replyToPane(hostId, session, paneId, text)`. That uses the host's
    current connection only when it is live and `Connected`, else `HostException.NotConnected`, and it never
    connects.
  - The outcome replaces the notification through `AgentAlerts.replied`, only while it is still up. A pane that
    went back to `Working` (often because of the reply) or was opened stays cancelled. Every re-post is
    `setOnlyAlertOnce` and keeps the Reply action.
  - On success: `Sent`, with a `MessagingStyle` history of the agent's line (`Needs input`/`Done`) and the reply
    from "You".
  - On failure: `Not sent: <host> is not connected` (also for `Closed`), `the agent is gone` (`PaneNotFound`),
    `the reply is too long`, `<program> is not installed on <host>`, the `CommandFailed` reason (80 characters at
    most), or `herdr refused it`. A blank reply is `Not sent: the reply is empty` and is not sent. A 45 s Kotlin
    bound (above Rust's 30 s query timeout) gives `Not sent: the host did not answer`.
  - The text is not logged anywhere, and `AgentAlert.toString()` leaves the reply out. After sending it is kept
    only in the notification's quote.
- **Tests.**
  - Rust (herdr fake), `herdr::reply`: a working or idle agent is `Prompted`, one or several lines, with nothing
    typed. `agent_blocked` and `agent_not_ready` each lead to `Typed`: the text, then Enter at least the pause
    later (paused clock). `agent_not_found`, and a pane gone before the typing, are `PaneNotFound` with no Enter.
    Other refusals are `Failed`.
  - Rust, `host`: validation (names, empty, 4 KiB allowed, one byte more `TooLarge`, `NotConnected`) and the
    command's reply.
  - Rust, `ssh::connection` (in-process SSH server playing herdr): `Prompted`, then `Typed` as prompt, text and
    `["Enter"]` in that order, then `PaneNotFound` with nothing typed, then `Closed` after a disconnect. A host
    without herdr is `NotInstalled`.
  - Live herdr suite, `a_reply_reaches_an_agent_panes_input_and_is_submitted`: `cat` runs in an isolated pane.
    A reply to a reported agent that is blocked (then working, refused as `agent_not_ready`) is `Typed`, and `cat`
    prints it a second time, so the Enter arrived. A plain shell pane and a missing pane are `PaneNotFound`, with
    nothing typed.
  - JVM: `AgentRepliesTest` covers not connected, sent (both routes), every failure reason with Reply kept,
    blank, timeout, `launch` finishing the broadcast, the reply never printed, the intent parse, and an outcome
    replacing only a notification that is still up. `HostConnectionsReplyTest` covers no connection, connecting,
    connected, a failure, lost with no reconnect, and no text in the timing log. `HostContractTest` crosses the
    FFI to the probe (routes, `PaneNotFound`, `TooLarge`, `InvalidName`, `Closed`). `SessionMessagesTest` covers
    the `TooLarge` message.
  - Device (compile), `AgentNotificationsDeviceTest`: the alert, a failure and a sent update each have one Reply
    action with the RemoteInput and a mutable broadcast PendingIntent of the app. The intent round-trips. A sent
    update is only-alert-once and quotes two messages.
**Fix: a reply reaches only its agent, never a shell (Codex v0.1.2 review P1 #1, P2 #3, P2 #4; Fable v0.1.2
review P1 and P2; branch `v012/fix-reply`).** This supersedes the typed path above (text, the pause, then Enter
as its own request) and the PendingIntent described in "Implemented (lane Reply)".

- **What herdr 0.9.3 offers, measured against an isolated session.**
  - `agent.send_keys` takes key names only. A single character is a key (`h`, `é`), a space is `space`, and a
    longer string is `invalid_key`, so there is no newline and no bracketed paste: herdr writes the keys as one
    unbracketed burst, which an agent's TUI can take as a paste (Enter included). Like `agent.prompt`, it refuses
    every agent herdr did not start (`agent_not_ready`, "not an active named agent", also after
    `agent.rename`), and a pane without an agent (`agent_not_found`). It does reach a blocked agent herdr
    started.
  - `agent.prompt` refuses a herdr-started agent that left the pane's foreground (`agent_not_ready`, "no longer the
    pane foreground process"). The old fallback typed into the pane on exactly that answer.
  - No other agent request carries input, so no request ties text to a reported agent (Claude Code through its
    hooks, the common case).
  - herdr keeps reporting an agent for about 0.5 to 0.75 s after its process exits, while `pane.process_info`
    already shows the shell's own process group in the foreground. Reproduced against the old code: a reported,
    blocked agent exits, a reply is `Typed`, and the shell runs it (`echo ...$((6*7))` printed `42`).
  - `pane.send_input { text, keys: ["Enter"] }` is one request: herdr writes the text as a bracketed paste when
    the program enabled mode 2004, then Enter, as `agent.prompt` does. `pane.send_text` is never bracketed.
  - Each socket connection answers one request and is then closed by herdr.
- **The reply names its agent.**
  - `AgentIdentity { terminal_id, agent }` is the `HerdrAgent` that raised the notification: herdr's terminal id
    and the agent's kind. `HerdrAgent` gains `terminal_id` (API 16 changes shape, unreleased).
  - `reply_to_pane(session, pane_id, agent, text)` first asks `agent.get` for the pane. A different pane,
    terminal or kind (a kind the reply names must match; one it does not name matches any) is `PaneNotFound`
    with nothing sent, as is no agent (`agent_not_found`).
  - So a stale notification never reaches another agent under a reused pane id (herdr restarted and numbered its
    panes again) or another kind of agent in the same terminal. Two agents of the same kind, one after the other
    in the same terminal, cannot be told apart: herdr gives no per-instance id. *(Superseded: herdr's
    `agent_session` tells them apart, and an absent kind matches nothing; see "Fix: a reply names its agent
    instance".)*
  - The terminal id is validated like a pane id and the kind is at most 128 bytes without control characters,
    else `InvalidName`.
- **The typed path is one request, checked right before.**
  - `agent.prompt` is tried first, as before. On `agent_blocked` or `agent_not_ready`, three streams are opened
    together: `agent.get` and `pane.process_info` run at once, then one `pane.send_input` with the text and
    `["Enter"]`.
  - It is sent only when herdr still reports the same agent, and the pane's foreground process group is neither
    the shell's own (`foreground_process_group_id == shell_pid`) nor led by a shell (`sh`, `bash`, `zsh`, `fish`,
    ..., a leading `-` ignored). The second case is an agent started from a nested shell that exited.
  - A pane whose foreground herdr cannot tell (no `shell_pid`, no foreground group, or no leader listed) is
    refused (`CommandFailed`: "herdr cannot tell what runs in the pane"). The foreground is the pane's shell:
    `PaneNotFound`.
  - The text and its Enter can never be split. `SUBMIT_ENTER_DELAY` is no longer used here.
- **Narrowed guarantee: "a reply never runs in a shell" holds except within one round trip.** An agent that exits
  after the checks and before herdr receives the `pane.send_input` (the checks' answers travel to the phone and the
  send travels back, about one network round trip, its stream already open) still gets the reply typed into its
  shell. Before the fix that window was the whole agent-exit lag above plus three requests and the 100 ms pause.
  herdr would need one request that types only while a given agent holds the pane's foreground to close it.
  The owner kept this design (owner decision of 2026-10-02, below).
- **Cancellation (Codex P2 #4).**
  - `HostCommand::ReplyToPane` carries the caller's `deadline` (`QUERY_TIMEOUT` from the call). The worker
    watches `reply.closed()`, as the upload does.
  - A caller that stops waiting (the coroutine cancelled, or the timeout) stops the reply at once during a
    check, and before any request that sends.
  - A request that sends (`agent.prompt`, `pane.send_input`) is never cut short, and starts only while its own
    bound (the 10 s request bound plus 1 s) ends before the deadline. Otherwise the reply is `CommandFailed` ("no
    time was left to send the reply").
  - So nothing is sent after the caller timed out, and there is no partial submit. Narrowed: a reply whose one
    sending request timed out on its own (10 s, herdr not answering) may still have been applied; it is reported
    as failed.
- **One Reply capability per post (Codex P2 #3).**
  - Each post of an agent notification gets a random nonce (`ReplyNonces`, app-private prefs, so a notification
    the system still shows after the process died stays answerable). It is put in the Reply PendingIntent's data
    (`or2-agent-reply:<tag>#<nonce>`, so each post's PendingIntent is distinct), and the PendingIntent is
    `FLAG_MUTABLE | FLAG_ONE_SHOT`.
  - A reply is taken only while the pane's notification is up (the system's list) and with the pane's current
    nonce, which `AgentAlerts.admitReply` uses up on the main thread before anything is sent.
  - A replayed, older or cancelled one does nothing at all. Every cancel (Working, opened, gone, on screen,
    switched off, another agent) revokes the nonce, and a re-post (the reply's outcome, a failure kept
    retryable) issues a new one.
  - The agent's identity travels in extras the intent always sets, so a fill-in cannot replace them.
    `AgentAlert.toString()` leaves the nonce out.
- **No broadcast is held (Fable P1).**
  - `AgentReplyReceiver` no longer uses `goAsync`: it hands the reply to `AgentReplies.launch` on the
    application's scope (`Dispatchers.Main.immediate`) and returns.
  - The capability is taken and the send started before `onReceive` returns, and a host that is not connected
    fails before it returns too.
  - A reply goes only over a live connection, and `ConnectionService` runs in the foreground while any is open,
    so the process lives until the reply ends (Rust 30 s, Kotlin's bound 45 s). A held broadcast could not have
    waited that long under Android's broadcast timeout. Chosen over capping the hold below 10 s, which would cut
    replies short on a slow host.
- **Stale notifications go (Fable P2).** `AgentAlerts` remembers the agent of each notification it posted and
  cancels it when the pane's view shows another one (another terminal, as after herdr restarts, or another kind).
  A notification left by a dead process is not known that way, but its reply is still checked in Rust.
- **Tests (each written to fail before the fix).**
  - Rust, herdr fake (now serving `agent.get`, `pane.process_info`, `pane.send_input`, agents that leave the
    foreground, and held answers), `herdr::reply`:
    - an agent that left the foreground behind an `agent_not_ready` gets nothing typed (the old code typed);
    - one that exits after the prompt's refusal gets nothing typed;
    - a shell leading the foreground (`bash`, `-zsh`, `fish`) gets nothing typed, while a shell the agent runs
      under it does not count;
    - a foreground herdr cannot tell is refused;
    - another terminal or kind gets nothing, also when it changes between the refusal and the typing;
    - the typed path is the checks then one `pane.send_input` with `["Enter"]`, its stream opened with the
      checks';
    - a caller that gives up during either check stops everything that sends;
    - a reply out of time sends nothing (paused clock).
  - Rust, `host`: the agent's validation and the deadline the worker gets. `ssh::connection` (in-process
    server): the order `agent.get`, prompt, both checks, one `pane.send_input`.
  - Live herdr: `a_reply_never_runs_in_the_shell_after_the_agent_exits` makes the agent exit, waits for the
    shell to lead the foreground, and replies while herdr still reports the agent (checked after the reply; an
    attempt that missed it tries again in a new pane, up to eight): `PaneNotFound`, nothing on the screen. Its
    first form failed on the old code with `Ok(Typed)` and the shell printing `42`; with the foreground check
    removed it fails the same way. The live reply test also refuses a stale terminal and a stale kind.
  - JVM: `AgentAlertsTest` (a pane that now holds another agent loses its notification; a capability taken
    once, refused after a newer post, a cancel, or once the notification is gone, and kept across a new
    process), `AgentRepliesTest` (a replayed or stale reply sends nothing; no agent, not sent; `launch` takes
    the capability and starts the send before it returns; the intent parse), `HostConnectionsReplyTest` and
    `HostContractTest` (the agent crosses the FFI; a malformed one is `InvalidName`, another pane's
    `PaneNotFound`; `HerdrAgent.terminal_id` crosses). With the two `AgentAlerts` guards undone, the three
    JVM capability and identity tests fail.
  - Device (compile): each post's PendingIntent is distinct, and the intent round-trips the capability and the
    agent.


**Owner decision of 2026-10-02: the typed reply stays for agents herdr did not start.** The Codex fix-check
review (P1, "the final pane-bound reply still has a shell-execution race") proposed dropping the typed reply and
failing closed for every agent herdr did not start. The owner chose to keep it: those are the owner's Claude Code
panes, the reason the feature exists. The design stays as above: the checks (`agent.get`, then `agent.get` and
`pane.process_info` together) immediately before one `pane.send_input` carrying the text and Enter.

- **Residual risk, stated plainly.** An agent that exits within about one network round trip after the checks
  (their answers travel to the phone, the send travels back) gets the reply typed into its shell, which runs it.
  The same client check, then send, shape comes before `agent.prompt`, which herdr itself also binds to the
  agent's foreground.
- **What would close it.** An atomic agent-bound herdr operation: one request that types text and Enter only
  while a given agent instance (its `agent_session`) holds the pane's foreground. herdr 0.9.3 has none
  (`agent.send_keys` takes key names only, and refuses every agent herdr did not start). It is a future request to
  herdr; more client-side checks cannot close it.

**Fix: a reply names its agent instance (Codex v0.1.2 fix-check P2; branch `v012/fix-3`).** Terminal id and kind
were not an identity: a Codex that exits and another Codex started in the same persistent terminal match both, and
a null kind matched every kind. This supersedes the matching in "The reply names its agent" above.

- **What herdr 0.9.3 reports, measured against an isolated session.**
  - `agent_session` (`{agent, kind, source, value}`, `kind` `id` or `path`) appears on `agent.get`,
    `pane.get` and the snapshot's agents only when herdr's own integration for that kind reports it
    (`source: "herdr:claude"`, as Claude Code's hooks do) and herdr detects that kind's process in the pane.
    A report from any other source (or2's tests' `or2-test`) is kept as the agent's status but gives no session.
  - herdr keeps one instance's session for the life of its process: a later report with another session id is
    ignored. When the process exits, the agent goes; a new instance in the same terminal has no session until its
    integration reports one (`pane.report_agent` or `pane.report_agent_session`), then its own.
  - `name` is set by `herdr agent start` (with `interactive_ready`) or `agent.rename`, and herdr clears it when
    that agent exits, is released or is replaced.
  - `agent.prompt` accepts an agent herdr's integration reports while it is not blocked (`Prompted`), not only one
    herdr started.
- **The identity.** `AgentIdentity { terminal_id, agent, name, session }` (`session` is `AgentSession { kind,
  value }`, herdr's `agent_session`). The view's `Agent` gains `agent_session` and `interactive_ready`, and
  `HerdrAgent` gains `reply_identity` (`AgentIdentity::of`):
  - with a kind and an `agent_session`: the terminal, kind, name and session;
  - with a kind, no session, and a `name` of an agent herdr started (`interactive_ready`): the terminal, kind and
    name;
  - otherwise (no kind, or a reported agent with neither a session nor a herdr name, renamed or not): `None`.
- **The rule, in Rust, on every path.** `same_agent` runs on `agent.get` before `agent.prompt` and again
  immediately before `pane.send_input`. The pane, the terminal and the kind must match exactly (the reply's kind
  must be present). When the reply has a session, herdr must report a session now with the same kind and value; an
  absent one never matches. Without a session, herdr must report the same name. Otherwise `PaneNotFound`, with
  nothing sent.
- **Fail closed.** An identity with no kind, or with neither a session nor a name
  (`AgentIdentity::is_instance`), is refused before anything is sent: `HostHandle::reply_to_pane` answers
  `CommandFailed` with `open the pane to reply` (`herdr::OPEN_THE_PANE`; `reply_in` refuses it too). The session's
  kind and the name are validated like the kind (at most 128 bytes, no control characters) and the session's value
  is at most 4096 bytes with no control characters, else `InvalidName`.
- **Kotlin.**
  - The alert's agent is `HerdrAgent.reply_identity`. A notification whose alert has none has no Reply action, only
    the tap that opens the pane.
  - The intent carries the name and the session's kind and value in extras it always sets.
    `agentReplyFrom` names no agent unless there is a terminal, a kind, and a session or a name.
  - A reply with no agent is not sent: `Not sent: open the pane to reply` (`NOT_SENT_OPEN_PANE`), as is Rust's
    refusal (`CommandFailed`).
  - `AgentAlerts.sameAgent` mirrors Rust. A notification this process posted is cancelled when the pane's agent
    now has another session, none, another name, terminal or kind.
- **Unchanged limit.** An agent herdr started that is replaced by another started with the same name in the same
  terminal, before its integration reports a session, cannot be told apart: herdr gives nothing more.
- **Tests (each written to fail before the fix; run against the old matching with the new shape).**
  - Rust, herdr fake (now reporting `agent_session`, `name` and `interactive_ready`), `herdr::reply`:
    - `a_replacement_of_the_same_kind_in_the_same_terminal_gets_nothing`: another session, or none, with the same
      terminal and kind; on the prompt path and the typed path; also a replacement between the prompt's refusal
      and the typing; a session of kind `path` with the same value. Old code: `Ok(Prompted)`.
    - `an_identity_without_kind_or_instance_is_refused_before_anything_is_sent`: no kind, or neither a session nor
      a name, whatever the pane holds, with no request at all; and an agent herdr reports with no kind. Old code:
      `Ok(Prompted)`.
    - `an_agent_herdr_started_is_named_by_its_name`: the same name is sent; none, another name, or another
      terminal is not. Old code: `Ok(Prompted)`.
    - `an_unchanged_session_gets_the_reply` (prompted and typed), and
      `only_an_agent_herdr_identifies_gets_an_identity` (`AgentIdentity::of`).
  - Rust, `host`: the identity's validation and `open the pane to reply` before anything is sent.
    `ssh::connection`: the in-process herdr reports the session.
  - Live herdr (isolated sessions, fake agents only):
    - `a_reply_reaches_only_the_agent_instance_it_names`: the fake agent is a copy of `cat` named `claude` in a
      temporary directory, which herdr's process detection takes for Claude Code. Its session is reported with
      `pane.report_agent_session` from `herdr:claude`. A reply naming it arrives, another session's does not, and
      no kind or no instance is refused. Then it exits and another starts in the same terminal: with no session yet,
      then with its own, the first one's reply gets `PaneNotFound` and nothing on the screen; the second one's
      arrives. Old code: the other session's reply was `Ok(Prompted)`.
    - `a_reply_reaches_an_agent_panes_input_and_is_submitted` and
      `a_reply_never_runs_in_the_shell_after_the_agent_exits` name their reported agent with `agent.rename`; a
      reply naming another name is refused (old code: `Ok(Typed)`).
  - JVM: `AgentAlertsTest.anotherInstanceOfTheSameKindInTheSameTerminalLosesItsNotification`,
    `anAgentHerdrDoesNotIdentifyIsNotifiedWithoutAReply` and `AgentRepliesTest.aReplyThatNamesNoAgentInstanceIsNotSent`
    fail with the old `identity`, `sameAgent` and no-agent message. `AgentRepliesTest` (the intent parse: no kind,
    or neither a session nor a name, names no agent), `HostContractTest` (the probe's agents carry a session, a
    herdr-started name and none; another session or name is `PaneNotFound`; no instance is `CommandFailed` with
    `open the pane to reply`), `HostConnectionsReplyTest`.
  - Device (compile): an alert with no agent builds a notification with no action.


## Image paste (lane Paste)

- **Sources**, each ending in the same upload:
  1. The composer's attach button (a compact `+` or image glyph left of the text). It opens the Android
     Photo Picker (`PickVisualMedia`, images only, no storage permission).
  2. An image committed by the keyboard (IME `commitContent`, e.g. a clipboard screenshot or a GIF
     keyboard) into the terminal or the composer.
  3. Android's share sheet: or2 accepts `ACTION_SEND` of `image/*` and asks which open terminal to send it to
     (a compact picker of open terminals, the last used first; no open terminal → a short message).
- **Processing on the phone:**
  - Decode, downscale so the long edge is at most 2048 px, and re-encode: PNG stays PNG when the image has
    transparency or is a screenshot-sized PNG under 2 MiB, otherwise JPEG quality 85.
  - Re-encoding drops EXIF and GPS. Nothing of the original file's metadata is sent.
  - Refuse above 20 MiB decoded.
- **Upload** (`upload_image`, Rust, `russh-sftp`, Apache-2.0, on the host's SSH connection):
  - The directory is `~/.cache/or2/images` (created `0700`; files `0600`).
  - Names are `or2-<UTC yyyyMMdd-HHmmss>-<6 random hex>.<ext>`.
  - The write goes to a temporary name and is then renamed.
  - Each upload first removes this directory's `or2-*` files older than 7 days (best effort).
  - No shell command is involved.
- **Insert:** the returned absolute path (shell-quoted if it needs quoting) is inserted with a leading space
  and no Enter. It goes into the composer when the composer is open, else into the terminal as a bracketed
  paste. Claude Code and Codex take an image path in the prompt.
- **UI:** while it runs, the terminal's notice strip shows `Uploading image…` with a spinner and a cancel
  action; a failure shows the reason (e.g. `SFTP is not available on this host`). Compact, per the UI system.
- **Tests:** Rust against the in-process SSH server with an SFTP subsystem (the directory created `0700`,
  file `0600`, the rename, old files swept, a server without SFTP → `SftpUnavailable`, a too-large refusal)
  and the disposable sshd suite if it offers `internal-sftp`. JVM: the processing (downscale, EXIF gone, PNG
  versus JPEG, size cap), quoting, the insert target. Device (compile): the attach button and the share
  target's intent filter.

`russh-sftp` (or the SFTP client chosen) is recorded in THIRD_PARTY_NOTICES as needed, and `xtask gen-licenses`
stays green.

**Implemented (branch `v012/paste`).**

- **SFTP client.** `russh-sftp` 3.0.1 (Apache-2.0, pinned `=3.0.1` in `core/Cargo.toml` and
  `Cargo.lock`), its client only (`RawSftpSession`), with no default features. It speaks SFTP v3 over
  any `AsyncRead + AsyncWrite`, so it runs on the pinned russh 0.63 channel stream with no russh
  dependency of its own; it is maintained alongside russh (its tests use russh 0.63). It brings
  `dashmap`, `tokio-util`, `serde_bytes`, `smallvec`, `hashbrown`, `lock_api`, `parking_lot_core`,
  `crossbeam-utils` and `scopeguard` (MIT or MIT/Apache-2.0, all GPL-3.0-compatible). Nothing is
  vendored or ported, so `THIRD_PARTY_NOTICES.md` is unchanged: the generator lists the crates, with
  their licence texts, in `rust.json` (Open source licenses); `gen-licenses --check` is green.
- **Rust (`core/or2-core/src/ssh/upload.rs`).** `HostHandle::upload_image(bytes, extension)` validates
  first, before anything is sent: the extension, lower-cased, is one of `png`, `jpg`, `jpeg`, `gif`,
  `webp` (`IMAGE_EXTENSIONS`), else `InvalidName`, as is an empty image; more than 20 MiB
  (`MAX_IMAGE_BYTES`) is `TooLarge`. The host driver (`HostCommand::UploadImage`) opens one session
  channel on the host's connection (the connection's own open, so a refused or late channel is closed),
  requests the `sftp` subsystem with a reply and initialises SFTP, within the exec timeout. A
  `channel_failure`, a channel closed instead, or a subsystem that does not answer SFTP's init (a
  missing `sftp-server`) is `SftpUnavailable`. Every path is relative to where the server's SFTP
  starts, which is the login's home, so `~` needs neither `$HOME` nor a shell, and no exec runs at
  all. Then (as first implemented; the review's fixes below supersede the `chmod` best effort, the
  order, the `realpath` reply, the timeout and the cleanup):
  1. each missing part of `.cache/or2/images` is made with `mkdir` mode `0700` (another upload making
     it meanwhile is fine); an image directory something else made is `chmod`ed to `0700` (best
     effort: a server that refuses `chmod` still gets the upload); a part that is not a directory is
     `CommandFailed`;
  2. the sweep: the directory's regular files named `or2-*` with an mtime more than 7 days before the
     phone's clock are removed (best effort, at most 5 s; directories and other names stay);
  3. the bytes go to `<name>.part` (`CREATE|EXCL|WRITE`, mode `0600`, then `fsetstat 0600` in case the
     server ignored the mode), 32 KiB writes with up to 16 in flight; the handle is closed;
  4. `rename` to `or2-<UTC yyyyMMdd-HHmmss>-<6 random hex>.<ext>` (`.part` is an `or2-*` name too, so a
     left-over one is swept later), and the reply is the server's `realpath` of it (absolute, through
     symlinks).
  The whole call is bounded by the query timeout (30 s). A caller that stops waiting (the coroutine
  cancelled, or the timeout) closes the reply: the upload stops and removes its `.part` (2 s, best
  effort). Failures are `CommandFailed` with a reason that never carries a path (`creating the image:
  permission denied`), `Closed` when the connection went. The SFTP channel closes when the upload
  ends, whichever way.
- **FFI (API 16).** `HostConnection.upload_image(bytes, extension)` as in the table, and
  `HostError::SftpUnavailable` and `HostError::TooLarge`. The contract probe host answers
  `/home/probe/.cache/or2/images/or2-19700101-000000-000000.<ext>`, and `SftpUnavailable` for a `gif`.
- **Deviation: `Session.paste_text(text)` (API 16).** "Into the terminal as a bracketed paste" needs
  the terminal's bracketed-paste mode, which only Rust knows (the existing `send_text` is typed text,
  never bracketed). `paste_text` writes what a submit writes without its Enter: one bracketed paste
  while the program has DECSET 2004 on (a paste end marker inside the text removed), else typed with
  newlines as carriage returns; empty text sends nothing; it waits behind a submit's pending Enter like
  other input (`SubmitSequencer`). Both transports (`Command::Paste` in the SSH pump and the mosh
  driver). A probe terminal echoes it as `paste` and the bracketed bytes.
- **Kotlin, processing (`paste/ImagePipeline.kt`, `AndroidImageCodec.kt`).** `prepareImage` decides,
  an `ImageCodec` does the pixels: `ImageDecoder` (PNG, JPEG, WebP, HEIF, a GIF's first frame; EXIF
  orientation applied) scales while it decodes to `fitWithin` 2048 px on the long edge (never up),
  into a software bitmap; `Bitmap.compress` writes PNG, or JPEG at 85, with no metadata, so EXIF and
  GPS are gone, also for an image that needed no scaling (nothing of the source is ever passed on).
  PNG when any pixel is not opaque (an alpha channel that is fully opaque does not count), or when the
  source is a PNG under 2 MiB (a screenshot); JPEG otherwise. The 20 MiB cap is applied to the image
  as read (`readCapped` stops reading past it, before any decoding) and to the result; the decoded
  bitmap itself never exceeds 2048 x 2048 (16 MiB), because the decoder samples down while it decodes.
  Reading runs on the IO dispatcher, processing on the default one.
- **Kotlin, upload and insert (`paste/ImagePaste.kt`, `PathInsert.kt`).** Each `ActiveTerminal` has an
  `ImagePaste` in the holder's scope: one upload at a time (a second image while one runs is not
  taken), over the terminal's host's current connection (`HostConnections.uploadImage`, so a mosh
  terminal that outlived its connection uses the new one). Its state drives the terminal card's
  notice strip: `Uploading image…` with the spinner and **Cancel** (the coroutine is cancelled, Rust
  cleans up), or the reason in `attention` with **Dismiss** (`SFTP is not available on this host`,
  `The image is larger than 20 MiB`, `Not sent: the host is not connected`, `Upload failed: <reason>`).
  The connection notice (connecting, closed) wins the strip when both apply. The path arrives on a
  channel the terminal screen collects, so it is inserted once, even if the screen showed later:
  `shellQuote` leaves a path of `[A-Za-z0-9_@%+=:,./-]` alone and single-quotes anything else (`'` as
  `'\''`), and `pathInsertion` puts a space before it, no Enter after. Into the composer's text when
  the composer is open, else `TerminalView.pasteText` (through `paste_text`, behind a tmux or herdr
  `Bottom` like other input). Closing or dismissing a terminal cancels its upload.
- **Sources.** The composer's attach button: an outline image glyph in a 40 dp box at the left of the
  text (the text's own 14 dp start padding then goes), `textMuted`, shown only for a terminal that
  takes images; it opens the Photo Picker (`PickVisualMedia`, `ImageOnly`, no permission). Keyboard
  images: the composer is now a state-based `BasicTextField(TextFieldState)` (the only Compose text
  field that receives content) with `Modifier.contentReceiver`, which takes image items and leaves the
  rest; `TerminalChromeState.composerText` stays a plain string view of it. The terminal's
  `InputConnection` advertises `image/*` (`EditorInfo.contentMimeTypes`) while the view has an
  `onImage`, and `commitContent` takes an image with its read permission, given back once it was read.
  Shares: `MainActivity` has an `ACTION_SEND` filter for `image/*`; a new share (not a recreation, not a
  relaunch from Recents) is offered to the UI, which keeps it in saved state and shows the share picker
  (`SharePickerSheet`, a compact sheet, "Send image to"): the connected terminals that are not closing,
  the last shown first (`ActiveTerminal.shownAt`, counted on each display), then the never shown in
  opening order. A pick shows that terminal and uploads the image to it. No open terminal: the message
  `No open terminal to send the image to`.
- **Gallery.** `terminal-attach` (the composer with its attach button), `terminal-uploading`,
  `terminal-upload-failed` and `share-picker`.
- **Tests.** Rust, the in-process server (it now serves the `sftp` subsystem with `russh-sftp`'s
  server over a temporary directory, `ssh/sftp_test_server.rs`, or refuses it): the directories made
  `0700` and the file `0600` with its bytes, the name's shape, the write before the rename and no
  `.part` left, no exec, the channel closed; the sweep (old `or2-*` files gone, recent ones, other
  names and an old `or2-` directory kept) and an existing `0755` directory made `0700`; no SFTP is
  `SftpUnavailable` with the channel closed and the connection fine; too large, empty and unknown
  extensions refused with nothing opened, exactly 20 MiB taken; a cancelled upload removes its
  `.part` and never renames. Names and dates (`image_name`, leap day, year end), mode bits, the pump's
  `Paste` (bracketed only with the mode on, no Enter, behind a submit's Enter), the handle's
  `paste_text`. The disposable sshd (`Subsystem sftp internal-sftp -d <fixture home>`, so the real home
  is never touched): an upload lands with `0600`/`0700` and an 8-day-old `or2-` file is swept; with no
  `Subsystem` line, `SftpUnavailable`. JVM: `ImagePipelineTest` (downscale to 2048, EXIF and GPS gone
  from a JPEG and a PNG text chunk, PNG versus JPEG, the 20 MiB cap before decoding and while reading,
  unreadable images) through a test codec of real PNG and a JPEG-container stand-in (unit tests have
  neither Android graphics nor AWT); `ImagePasteTest` (quoting, the insert target, progress, one at a
  time, delivered once even when no screen collected, cancel, failure reasons);
  `HostConnectionsImagePasteTest` (the host's connection, a closed host, close cancels, the share
  order); the FFI contract across JNA (`upload_image`'s path and errors, `paste_text`'s bracketed
  echo). Device (compile; not run here): the attach button only with images and left of the text, the
  path into the composer or the terminal, Cancel in the strip, the `ACTION_SEND` filter for image types
  only, and `commitContent` (images only, permission given back).
- **Open.** An upload larger than about 30 s of the link's speed hits the query timeout (a 3 MiB JPEG
  needs about 1 Mbit/s); the processed images are usually far smaller. `russh-sftp` decodes SFTP
  handles as UTF-8 strings (lossily): OpenSSH's handles are small integers and survive, a server with
  binary handles above 0x7f could fail. The sweep compares with the phone's clock. (Both timeouts and
  handles: see the fixes below.)

**Fix (v0.1.2 review, lane Paste; branch `v012/fix-paste`).** From the Codex and Fable reviews of
`0455bd3`. `API_VERSION` stays 16 and no export changed shape: the FFI table above stands.

- **Fix: the host's `realpath` is untrusted (Codex P1 #2, Fable P3).** A server could answer an
  absolute "path" holding ETX, ESC, CR or LF: typed into a shell (no bracketed paste) the quote is
  flushed and the rest runs; ESC acts on the terminal or its program. Rust now passes on the server's
  answer only when it is safe to type (`upload::is_safe_image_path`): absolute, ending in `/` and the
  exact generated name, at most 4096 bytes, and with no control character (C0, DEL, C1: NUL, ETX, ESC,
  CR, LF, and so no bracketed-paste marker) and no U+FFFD (a name that was not UTF-8, so not the
  file's). Otherwise it makes the path from the SFTP start directory's own `realpath` and the known
  relative path (`<start>/.cache/or2/images/<name>`), checked the same way; otherwise the upload fails
  (`the host did not resolve the image path`) and the image is removed. Kotlin is the second line:
  `insertablePath` (absolute, no ISO control character, no U+FFFD, at most 4096 characters);
  `ImagePaste` fails such an answer in words (`Upload failed: the host answered an unusable path`) and
  delivers nothing, and `pathInsertion` refuses one outright.
  Tests: `a_hostile_realpath_never_reaches_the_terminal_in_either_paste_mode` (ETX with a command and
  LF, an OSC, CR, LF, `ESC [201~`, C1 CSI, NUL, DEL: the path made instead, and `submit_text_bytes` of
  it with bracketed paste on and off carries no control byte but the paste markers),
  `a_realpath_that_names_another_file_is_not_passed_on`, `an_upload_with_no_safe_path_fails_and_removes_its_image`,
  `upload::tests::only_an_absolute_control_free_path_of_the_image_is_safe`; JVM
  `aPathWithControlCharactersIsNeverInsertedInEitherTarget`.
- **Fix: only `content:` images (Codex P2 #5).** `readableImageScheme`: `imageFromUri` (every source)
  refuses any other scheme (`file:`, none) before anything is opened, and `sharedImage` does not take
  such a share. Tests: JVM `onlyContentUrisAreRead` (`file`, none: refused, nothing opened, the grant
  given back); device (compile) `sharedImage` of `file://`, a bare path and a relative name is null.
- **Fix: a provider that never answers (Codex P2 #6).** An image is read through an
  `AssetFileDescriptor` opened with a `CancellationSignal` (`AndroidImageSource`) on an IO thread of its
  own (`readImage`): the caller awaits it, so a cancel or the 60 s read deadline (`READ_TIMEOUT`, open
  to last byte; `The image took too long to read`) returns at once, then aborts the source, which is
  idempotent: it cancels a pending open and closes the descriptor (Android wakes a read blocked on a
  descriptor it closes). A keyboard's grant is given back exactly once on every path (`Once`), as soon
  as the reading is over. `ImagePaste` starts its coroutine atomically, so a taken image is always
  prepared and gives back its grant even when cancelled before it ran; a refused one is given back by
  the caller. Tests: JVM
  `aCancelStopsAProviderThatNeverOpensOrNeverReadsAndGivesTheGrantBackAtOnce`,
  `aProviderTooSlowToAnswerIsRefusedAtTheDeadlineAndAborted` (each for a blocked open and a blocked
  read), `aTakenImageIsAlwaysPreparedSoItsGrantIsGivenBackEvenWhenCancelledAtOnce`.
- **Fix: abandoned provider reads are bounded (Codex v0.1.2 fix-check P2; branch `v012/fix-3`).** A
  provider that ignores the cancel keeps its thread until it answers, and each new share or keyboard
  image used to add one more blocked `Dispatchers.IO` job. Reads now run on `ImageReaders`, not the
  shared IO pool: a pool of `MAX_LIVE_READS` (4) daemon threads of their own (`or2-image-read`), with
  one place per read. A read holds its place until its blocking call really returns, not when its
  caller gives up at the cancel or the deadline. With all four places taken by reads that have not
  returned, a new image (attach button, keyboard, share) is refused before anything is opened, in the
  strip: `Earlier images are still being read; try again later` (`STILL_READING`). No more blocking
  work starts, and the grant is given back as for any refusal. A place comes back when its provider
  finally answers or fails. So stuck providers cost or2 at most four threads; the upload and its grant
  are released at the deadline as before. Test: JVM
  `providersThatIgnoreTheAbortHoldAtMostTheCapAndLaterReadsAreRefusedWithoutStartingAny`: with a cap of
  2 and six timed-out reads whose provider ignores the abort, only two are opened, at most two workers
  are live, and the other four are refused; once the providers answer, a read goes through again. On
  the old unbounded path all six were opened and timed out (`[TOO_SLOW x6]`). **Open:** no Android
  instrumentation test with a real `ContentResolver` provider (open cancellation, a blocked descriptor
  read, the grant) runs here: the phone is not used.
- **Fix: the image directory is checked, not trusted (Codex P2 #7, Fable P3).** Each part of
  `.cache/or2/images` is checked with `lstat` before anything is made below it, and fails closed
  (`upload::directory_problem`): a directory; not a symbolic link; not writable by group or others
  (`~/.cache`: not by others; **narrowed** because a umask of `002` with a group of the user's own, as
  Debian and Ubuntu give users, leaves it `0775`, and the file is `0600` and the directories below it the
  user's `0700` ones whatever a group member renames); the
  image directory exactly `0700` after the `setstat` (a server that will not make it private fails the
  upload: `~/.cache/or2/images could not be made private`). The temporary file gets `fsetstat 0600` and
  is then checked with `fstat` to be a regular `0600` file, before any byte is written (`the image
  could not be made private`). Its owner is the account the upload runs as: every part must belong to
  it (`~/.cache` may also be root's, as a `sudo` program leaves it), checked once the file is made
  (before the sweep and before any byte) and again before the rename; the sweep removes only that
  owner's files.
  A server that reports no modes or no owners fails. **Deviation:** `~/.cache` itself may be a
  symbolic link (dotfile setups put it elsewhere): it is followed, and its target is held to the same
  rules (`~/.cache is writable by others` for a shared one). **Narrowed guarantee:** SFTP v3 names
  files by path only (no `openat`, no `O_NOFOLLOW`), so the checks hold against other accounts, which
  cannot change a private directory of the user's, but not against a process of the same account
  racing the upload between a check and the next request; the server itself is trusted with where the
  bytes go. Tests: `symbolic_links_in_the_image_directory_are_refused_and_nothing_is_written_through_them`,
  `a_cache_directory_elsewhere_is_followed_only_when_it_is_private`,
  `directories_others_may_write_to_or_another_user_owns_are_refused`,
  `a_server_that_ignores_modes_fails_the_upload_instead_of_sharing_the_image` (directory modes and file
  modes), `the_directories_are_checked_again_before_the_rename` (the directory swapped for a link while
  the bytes are written), `upload::tests::image_directory_parts_must_be_private_directories_of_the_user`.
- **Fix: nothing left behind after the rename (Codex P2 #8).** The upload records what it made (the
  temporary file's create sent, made, the rename sent, renamed); whatever ends it early (a failure, a
  check, a cancel, the timeout) removes the temporary file, the image, or both while a rename's answer
  is pending (2 s, best effort). A name that already existed (an exclusive create or a rename refused)
  is never removed. The image stays the host driver's until its path is delivered: `upload::deliver`
  removes it when the caller's reply is gone. *(The narrowing that stood here, a path sent but not taken
  left for the sweep, is closed by "Delivery is acknowledged" below.)* Tests:
  `a_failed_realpath_removes_the_renamed_image`,
  `an_upload_cancelled_while_its_path_resolves_removes_the_renamed_image` (a delayed `realpath`),
  `an_image_whose_caller_stopped_waiting_as_it_was_done_is_removed`.
- **Fix: delivery is acknowledged (Codex v0.1.2 fix-check P2; branch `v012/fix-3`).** A successful
  `reply.send` only queued the path: a caller whose timeout or cancel became ready together with the
  send dropped it unread, and the image stayed. Delivery is now two steps:
  - The host task sends `host::UploadedImage { path, taken }` (`HostCommand::UploadImage`'s reply; no
    FFI change).
  - `HostHandle::upload_image` acknowledges on `taken` in the same step that receives the path (no
    await in between), then returns it.
  - `upload::deliver` waits for that acknowledgement, at most 30 s (`ACK_TIMEOUT`). A caller always
    acknowledges or drops the path at once, so the bound only covers a caller that never runs again.
    Without the acknowledgement it removes the image (2 s, best effort).
  - **Narrowed:** once Kotlin has the path, an image no one inserts (the terminal closed in that
    instant) is left for the 7-day sweep. A server stalled longer than the 2 s cleanup keeps the file for
    the sweep too.
  - Tests: `an_image_whose_path_was_sent_but_never_taken_is_removed` (in-process SFTP server: the path
    is sent into an open reply and dropped unread; with the old rule the image was left on the host),
    `a_path_sent_as_its_caller_gives_up_is_never_acknowledged`,
    `a_path_sent_at_its_callers_deadline_is_kept_only_if_returned` (paused clock: the host's answer and
    the caller's deadline fall due together, and the image is kept exactly when the caller got its path),
    `an_uploaded_path_is_acknowledged_as_its_caller_takes_it`.
- **Fix: a path that arrives under the multi-line confirmation (Codex P2 #9).** Confirming clears only
  the text that was sent (`TerminalChromeState.composerSent`, `composerAfterSend`): an image's path
  inserted while the dialog was open stays in the composer. Test: JVM `ComposerSendTest`.
- **Fix: the upload's timeout grows with its size (Fable P3).** `host::upload_timeout`: 30 s and 1 s
  per 100 KiB begun (a link of 100 KiB/s still gets there), at most 240 s (`MAX_UPLOAD_TIMEOUT`; 20 MiB
  gets 235 s). The SFTP requests keep their own exec timeout each. Tests:
  `the_upload_timeout_grows_with_the_image_up_to_a_cap`,
  `an_upload_waits_its_size_s_timeout_not_the_query_timeout` (3 MiB: 61 s, paused clock).
- **Fix: a second image is never dropped silently (Fable P3).** While one uploads, `ImagePaste.start`
  of another (attach button, keyboard, share) takes nothing and the strip says `An image is already
  uploading` (still with **Cancel**) for 3 s, then `Uploading image…` again. Test: JVM
  `aSecondImageWhileOneUploadsIsRefusedInWords`.
- **Fix: a pending share after process death (Fable P3).** The share picker's pending image is saved
  with a token of the process that took it (`savedShare`); a recreation in another process (the old
  one died) drops it (`restoredShare`): the terminals it was for are gone. A rotation keeps it, as
  before. Test: JVM `ImageSharesTest`.
- **Fix: binary SFTP handles (Codex P3 #10): documented, not patched.** Making `russh-sftp`'s handles
  opaque bytes changes its protocol types for the client and the server (`Handle`, `Close`, `Read`,
  `Write`, `Fstat`, `Fsetstat`, `Readdir`): not a small patch. The client is therefore
  **OpenSSH-compatible** (OpenSSH and every server whose handles are UTF-8), not generally SFTP v3
  compatible. A server with handles that are not UTF-8 fails the upload cleanly: the lossy handle names
  no open file there, so the first request on it fails before any byte is written, and the temporary
  file is removed by path. Test (in-process server relaying handles as `0xff 0xfe` + its own):
  `a_server_with_handles_that_are_not_utf8_fails_the_upload_cleanly` (no file anywhere, no rename, the
  connection fine). It passes before this fix too: it pins the behaviour, it does not repair one.

**Fix (fix-check 2, P2): a path whose acknowledgement came too late is not returned.** `deliver` waits 30 s
for the caller's acknowledgement and then removes the image; a caller polled only after that still found the
queued path. `HostHandle::upload_image` now returns the path only when its acknowledgement was received by the
host (`taken.send` succeeds), else `CommandFailed` ("the upload took too long and its image was removed").
Test: `host::tests::a_path_whose_acknowledgement_the_host_stopped_waiting_for_is_not_returned` (fails with the
check disabled).

### Several images at once (v0.1.2, owner request 2026-10-03)

The owner could attach only one image at a time: the picker took one, the share target took one, and a
second image during an upload was refused. v0.1.2 takes several. Kotlin only: Rust's `upload_image` is
unchanged and is called once per image, one after another, on the host's connection (no FFI change, API
stays 16).

- **Sources.**
  - The composer's attach button opens `PickMultipleVisualMedia` (images only) with **at most 10**
    (`MAX_IMAGES`). Picking one image behaves exactly as before.
  - The share target accepts `ACTION_SEND_MULTIPLE` of `image/*` as well as `ACTION_SEND` (both in the
    manifest's intent filters). Each `EXTRA_STREAM` item goes through the same checks as a single share
    (`content:` only; anything else is skipped and counted as refused, never silently dropped). The
    open-terminal picker is asked once for the whole share.
  - A keyboard's image (`commitContent`) is one image, as before.
- **One queue per terminal** (`ImagePaste`). Every image from every source joins the terminal's queue, in
  arrival order, and the queue uploads them **one at a time** (prepare, then `upload_image`). An image
  arriving while the queue runs joins it instead of being refused. At most `MAX_IMAGES` (10) are pending
  or running at once; an image beyond that is not taken: the strip says `At most 10 images at a time`
  for `ALREADY_SHOWN` and the source gives back what it holds (a keyboard's grant), exactly as the old
  refusal did. `ALREADY_UPLOADING` and `UploadState.AlreadyUploading` go away.
- **Every queued item's `prepare` runs**, also when the queue is cancelled before reaching it, so a
  keyboard grant or a provider read is always given back (the existing rule "once taken, prepare always
  runs" holds per item). A cancelled item's `prepare` stops at once and nothing of it is uploaded.
- **Insert once, when the queue is empty.** The paths of the images that uploaded are inserted together,
  in arrival order, as one insertion: each `pathInsertion` (a space, then the shell-quoted path)
  concatenated, so `" /a.png /b.jpg"`, with no Enter, into the same target as before (composer when open,
  else one bracketed paste into the terminal). A path that is not `insertablePath` is left out and counts
  as failed. One image inserts exactly what it inserts today.
- **Failures don't stop the queue.** A failed image is skipped and the rest still upload. When the queue
  ends, the paths that uploaded are inserted, and the strip shows a warning with `Dismiss`: one image
  failed → its reason, as today; several images and some failed → `<n> of <total> images failed: <first
  reason>`.
- **Cancel** (the strip's action while uploading) stops the running upload (Rust removes its temporary
  file, as today), drops everything still queued, and **inserts nothing** from that queue run. Images that
  had already finished stay in `~/.cache/or2/images` (private, swept after 7 days); no remote delete.
- **Strip.** One image: `Uploading image…` as today. Several: `Uploading image <i> of <n>…`, where `n`
  grows if more join the running queue. Busy spinner and `Cancel` as today.
- **Tests (JVM).** The queue: three images upload in order and insert once as `" p1 p2 p3"`; an image
  joining a running queue is uploaded and inserted with the rest; the 11th pending image is refused with
  the message and its source is told (`start` returns false); a failure in the middle still uploads and
  inserts the others and shows `1 of 3 images failed: <reason>`; cancel inserts nothing and runs every
  queued `prepare` (counted); the strip's `i of n` text; one image behaves exactly as before (the existing
  tests stay green, adjusted only where they asserted `AlreadyUploading`). Shares: `ACTION_SEND_MULTIPLE`
  with three `content:` images gives three, a `file:` item among them is skipped and counted, more than 10
  keeps the first 10 and says so. Device (compile): the picker contract and the intent filter.

**Implemented (branch `v012/multi-image`).** Kotlin only; API stays 16.

- **Queue (`ImagePaste`).** A run lasts from its first image until the queue is empty. `UploadState.Uploading`
  carries `image` (the one being worked on, `done + 1`), `images` (taken in this run) and `full` (the
  `At most 10 images at a time` moment, `TOO_MANY_IMAGES`); `AlreadyUploading` and `ALREADY_UPLOADING` are gone.
  `paths` is a `Flow<List<String>>`: one list per finished run, in arrival order, none for a run that uploaded
  nothing or was cancelled. The terminal screen inserts a list as one (`pathsInsertion`, `composerWithPaths`). The
  run's end reason is `queueFailure` (one image: its reason; several: `<n> of <total> images failed: <first>`,
  also when all failed). `generation` still keeps a cancelled run's late end from speaking for the next run.
- **Cancel.** The run's coroutine is cancelled; it then runs each image still queued, in that cancelled
  coroutine, so each gives back its grant. `imagePreparation` now checks for a cancel before it opens anything,
  so a dropped image opens no provider and takes no `ImageReaders` place; the queue also checks after a
  preparation, so an image prepared as the cancel came is not uploaded. Reads stay one at a time per terminal,
  well inside `MAX_LIVE_READS`.
- **Picker.** `IMAGE_PICKER` (`PickMultipleVisualMedia(MAX_IMAGES)`, images only); without the system picker,
  Android's fallback allows several with no limit of its own, and the queue refuses past 10 in words.
- **Shares.** `sharedImages(intent)` reads `ACTION_SEND` (one stream) and `ACTION_SEND_MULTIPLE` (a list) into a
  `Share`: the `content:` streams in order, the first 10 kept (`over` counts the rest), every other item (another
  scheme, none, a missing stream) counted in `refused`. A share is taken once; what it does not send is said in
  the app's message (`shareNote`: `At most 10 images at a time: sending the first 10`, `<n> shared items are not
  images or2 can read`) as the terminal picker opens, or alone when nothing is readable. So a single `file:`
  share, silently ignored before, is now said too. The pending share's saved state holds all its URIs
  (`savedShare(vararg)`, `restoredShare` a list). The picker sheet's title stays `Send image to`.
- **Tests.** JVM `ImagePasteTest` (three in order inserted once, one joining a running queue, the 11th refused and
  room again once one is done, a failure in the middle, refused and unusable images counted, cancel prepares
  every queued image in a cancelled run and uploads none, a cancelled run's late end, the strip's `i of n`,
  `pathsInsertion`), `ImageSharesTest` (three images, `file:`/no scheme/missing skipped and counted, 12 keeps 10
  and says so, several URIs in saved state), `ImageSourceTest`
  (`aPreparationRunAfterItsQueueWasCancelledOpensNothingAndGivesTheGrantBack`). Device (compile):
  `IMAGE_PICKER`'s intent (`EXTRA_PICK_IMAGES_MAX` 10), the `SEND_MULTIPLE` filter, `sharedImages` of both actions.

# v0.1.2: owner QA of 2026-10-03 (Home, terminals, composer, upload speed)

Owner QA on the phone found these. Placeholders only: no real host details in tests or docs.

## One terminal per herdr session, and what the terminals are called (lane Terminals)

- **The bug.** An inbox or notification tap on an agent (`TerminalActivations.openAgent` →
  `openOrReuse`) reused only a terminal opened for that exact pane (`findOpenTerminal(hostId,
  Herdr(session, pane))`), so tapping a second agent opened a second full herdr client (its own mosh
  session) on the same herdr session. herdr's focus is shared, so every such terminal shows the same
  focused pane. The owner's host had `herdr`, `herdr w1:pT` and `herdr w12:p7` open at once.
- **The rule.** Every herdr open (picker, inbox, notification, share, reattach) reuses the open
  terminal for that host and herdr session whatever pane it was opened on: a pane-less one first, then
  any (the order `reusable` already uses), never a closed, retired or closing one. A target with a pane
  focuses that pane first, then shows the reused terminal; with none open, one is opened on
  `Herdr(session, pane)` as today. One rule in one place (`TerminalActivations`), used by `open`,
  `openAgent`, `reuse` and the reattach path. Duplicates already open are left alone (not auto-closed).
- **Titles.** `targetTitle` no longer carries a pane id: `herdr` (default session) or
  `herdr <session>`. Where a title is shown with a detail line (Home's session card), a herdr terminal's
  detail is the focused pane's agent label from that session's live view when there is one, else the
  focused pane's cwd, else nothing.
- **Home session card detail.** The working directory when known (`inbox.cwdOf`), else the herdr rule
  above, else empty (the card keeps its height). Never `user@host`: the card's pill already names the
  host.
- **Host card address.** A connected host shows the address in use (`HostState.Connected.addressIndex`)
  with `+N` for the others; otherwise the first address as today.
- **Picker tab** `Recent` is renamed `Open` (it lists the open terminals); test tags unchanged.
- **Toolbar keys** (the row has room): after History, three mono text keys in the existing `ToolKey`
  style: `⇧Tab` (Shift+Tab through the key encoder: Claude Code's mode cycle; Gboard cannot send it),
  `/` and `@`. `⇧Tab` always goes to the terminal. `/` and `@` insert into the composer at its cursor
  while the composer is open, else type into the terminal. They honour the Ctrl latch like the other
  keys only where that makes sense (`⇧Tab` ignores it).
- **Multi-line confirmation only where lines really run one at a time.** FFI API 17:
  `TerminalModes.bracketed_paste` (DECSET 2004, read where `mouse_tracking` is). The composer's
  "Send N lines?" and the toolbar's "Paste N lines?" are asked only while the program has bracketed
  paste **off**; with it on, multi-line text goes straight out as one paste (and one Enter for a send),
  as Rust's submit already does. `NativeContractTest` and the probe expect API 17.
- **Tests.** JVM: `TerminalActivationsTest` (an agent tap on pane B reuses the open terminal opened on
  pane A of the same session after focusing B; a pane-less one is preferred; another session or host
  opens a new one; a closed one is never reused), titles, the card detail and address rules, the
  confirmation rule both ways. Rust: the mode is reported (on and off). Device (compile): toolbar keys.

## Upload speed (lane Upload)

- **The problem.** An image took 4–5 s end to end on the owner's host (about 130 ms away): the upload
  makes about 30 SFTP requests strictly one after another (channel, subsystem and init on every upload;
  about 13 `lstat`s of the three directory parts, before the write and again before the rename; the
  7-day sweep before the write; open, mode, write, close, rename, realpath). The bytes are one round
  trip.
- **The fix keeps every check** of "Image paste" and its review fixes; only waiting changes:
  1. **Independent requests are sent together.** SFTP v3 allows many requests in flight
     (`RawSftpSession` matches replies by id). The `lstat`s of `.cache`, `.cache/or2` and
     `.cache/or2/images` go out at once in the common case where all exist; only a missing part falls
     back to today's sequential create path. The re-check before the rename is one batch too. Any
     other independent pair (e.g. the file's checks) likewise. Order is kept wherever a check depends
     on an earlier step.
  2. **One SFTP session per host connection**, opened on the first upload and reused by later ones
     (uploads on a connection are serialized). A session that fails at the transport level is dropped
     and reopened once for that upload; it ends with the connection.
  3. **The sweep runs after the path is delivered**, in the background (best effort, bounded as
     today, at most once per SFTP session per hour), never on the upload's critical path.
- **Target.** An upload into an existing directory on a reused session takes at most 9 sequential
  round trips (12 on a new session).
- **Tests.** Rust, in-process SFTP server with a per-request delay: the round-trip bound above
  (counted or timed), a reused session, a dropped session reopened once, the sweep after delivery and
  rate-limited, and every existing safety test unchanged and green.

**Implemented (branch `v012/upload-speed`).** Rust only (`ssh/upload.rs`, the host driver); no FFI change,
API stays 16 (only `upload_image`'s doc comment changed). Every check of "Image paste" and its fixes is kept, with
its error message.

- **Round trips, measured** (in-process server, a relay holding every reply back 20 ms and counting how many
  replies each request waited for; an existing private directory, a 100 KB image): **27 before** on every upload
  (each opened its own session), **11** on a new session (channel open, subsystem, init and 8 SFTP waves) and
  **8** on a reused one. The waves: the three `lstat`s of the directory parts and a `stat` of `~/.cache` (in case
  it is a symbolic link) together; `open`; `fsetstat`; `fstat` with the parts again (now checked with the
  owner, after the file); the writes (up to 16 in flight, so one wave up to 512 KiB); `close` with the parts
  again (the check before the rename); `rename`; `realpath`.
- **Batches keep the checks' order.** A batch's answers are checked in the order the sequential code checked
  them, so the first problem and its message are the same. `fsetstat` then `fstat` stays two steps (the
  `fstat` checks what the `fsetstat` did), the directory reads after the file's create are sent once it is
  made, and the re-check before the rename once every write was answered. Only when a part is missing or wrong
  does the upload fall back to the part-by-part path (create, `setstat`, check each before going below it),
  unchanged. A batch that failed for want of a session (not a server status) fails the upload at once,
  without the fallback, so nothing more is sent on a dead session.
- **The session** (`upload::Uploads`) is the host driver's, one per connection: opened by the first upload,
  reused by the next ones (a `tokio` mutex: one upload at a time; the path's delivery and the sweep run after
  the lock is released), dropped with the driver (and the upload tasks, which end with `closing`), which
  closes its channel. **Decision: "fails at the transport level"** is a request that got no answer: the
  session's stream ended or closed (`IO`, `UnexpectedBehavior`, `UnexpectedPacket`), or a request timed out.
  Such a session is dropped; when nothing was made yet (no temporary file's create sent) and the upload was
  not cancelled, the upload starts again on a new session, once. A server's error status keeps the session. A
  cancelled upload keeps it too, unless its cleanup gets no answer in 2 s.
- **Handles on a kept session.** A session now outlives its upload, so whatever ends an upload early also
  closes the temporary file's handle (with the removal, 2 s), and a sweep that hits its 5 s bound closes its
  listing's handle; before, both went with the session.
- **The sweep** (`upload::Sweep`) runs in the upload's driver task once `deliver` is done (taken or not), for
  an upload that succeeded, with that upload's owner and the phone's clock at that moment; it ends with the
  connection. **Decision:** at most one sweep starts per session per hour (`SWEEP_INTERVAL`, counted when it
  starts, so one that failed or timed out counts too); a new session sweeps again at its first upload.
- **Tests.** In-process (`ssh/connection_tests.rs`; `sftp_test_server.rs` gains `relay` and `Link`:
  `Quirks::latency`, the round-trip count, and `sftp_hang_ups`, which hangs up a session at its next request
  but `init`): `an_upload_into_an_existing_directory_waits_for_few_round_trips` (at most 12 new, 9 reused),
  `a_session_that_fails_under_an_upload_is_reopened_once` (one hang-up: reopened and done; two: failed after one
  reopen, nothing left behind; the next upload opens a new session),
  `the_sweep_runs_after_the_path_is_delivered_at_most_hourly_per_session` (the `opendir` after the image's
  `realpath`, no second sweep on the session, a new session sweeps again),
  `a_cancelled_upload_closes_its_file_and_leaves_the_session_to_the_next`,
  `the_kept_session_closes_its_channel_when_dropped`, `upload::tests::a_session_sweeps_at_most_once_an_hour`.
  **Adjusted** because they asserted the old order: `an_upload_makes_a_private_directory_and_a_private_file_through_a_rename`
  (asserted the SFTP channel closed after the upload; now that the second upload reuses the session and its
  channel stays open), `an_upload_sweeps_old_or2_files_once_delivered_and_makes_an_existing_directory_private`
  (was `an_upload_first_sweeps_…`: it waits for the sweep now; the same files go and stay),
  `the_directories_are_checked_again_before_the_rename` (it swapped the directory once the sweep's `opendir` was
  seen; now once the first write is, which the test server logs), and the sshd suite's
  `an_image_uploads_over_internal_sftp_into_a_private_cache_directory` (a second upload on the same connection,
  then the old file swept by a new connection's upload). Every other upload and safety test is unchanged.

# v0.1.2: streamline (owner-approved plan, 2026-10-03)

The owner, on the phone: *"I'm totally lost with the UI. I don't know which button does what and why."* A
read-only review of the whole app (four reviewers: the UI map, Kotlin app layer, Kotlin terminal layer, Rust
core and FFI) found no serious bug, about a dozen small ones, and roughly 1,500 lines that can go without
losing a feature. This section is the plan the owner approved. **No feature the owner uses is lost, and the
polish stays** (compact UI per `docs/ui.md`; the terminal header's coloured discs, SSH/mosh pill and centred
title; the transparent arrow pad). Four lanes own disjoint files; each fixes its bugs with a test.

## Words

- **Host**: a machine (never "connection"). **Terminal**: something open in or2. **Session**: only a tmux or
  herdr session. **Agent**: a herdr agent. **Connect** (the biometric prompt is implied), **Retry** after a
  failure. User-visible text follows this everywhere: Home's section is `Hosts` ("No hosts yet"), the form is
  `New host` / `Edit host`, the switcher is `Terminals`, the connection notification says `N open terminals`,
  `Unlock and connect` / `Unlock` become `Connect`.

## Lane A: the UI model (Kotlin: `app/Or2App.kt`, `app/Navigation.kt`, `app/HostPicker.kt`,
## `app/TerminalActivations.kt`, `app/Reattach.kt`, `home/*`, `host/*`, `session/SessionScreen.kt`,
## `hosts/HostFormScreen.kt`, `inbox/InboxScreen.kt`, the gallery screens for these, `docs/ui.md`)

- **Home is the one place for hosts and their terminals.** Sections: notices and the Resume card as today,
  then `Hosts` with `Connect all` as a compact text action in the section header (when more than one host can
  connect). Each host card:
  - Header row: the status dot, the name, the address-in-use or progress or failure line (as today). **Tapping
    the header opens the session picker over Home**, connecting first through the picker's gate exactly as the
    `>_` button does today. A trailing **`⋯`** button opens the host menu (Connect or Disconnect, Edit, Delete
    with its confirm); long press stays as an alias of `⋯`.
  - Below the header, when the host has open terminals: its terminals as the existing thumbnails (live preview,
    transport badge, title, detail line), in a horizontal row inside the card. Each thumbnail has a small `×`
    (24 dp disc, 40 dp touch box) that closes that terminal. A closed terminal is marked `Closed` on its
    thumbnail. Closing a herdr or tmux terminal only ends or2's view (it keeps running on the host): one tap.
    Closing a shell ends the shell: a confirm (`Close shell? Programs running in it end.`).
  - Removed: the separate `SESSIONS` row, the `>_` session button, the `Working` / `Needs attention` chips (the
    Inbox icon's badge says it), the card body's navigation to the host screen.
- **The host screen is removed** (`host/HostScreen.kt`'s `HostScreen`, `Destination.HostPage`). Its unique
  bits move: the host-key dialog is the global one already; the UDP-blocked line shows in the picker under its
  tabs (muted); the per-address detail and "address in use" are on the card. `NavStack.afterPaired` and
  `afterKeepAlive` land on Home with that host's picker open. A saved stack naming a host page decodes to
  Home. The Inbox host row loses its tap (its Connect / Retry pill stays). `SessionPickerSheet`, `PickerGate`
  and their helpers stay (move them out of `HostScreen.kt` into their own file).
- **Picker:** tabs `herdr` and `tmux`, the `Shell` pill (keeps the `>_` glyph, now its only meaning), Refresh,
  New tmux session, the gate. **No `Open` tab.** A herdr session or tmux session that already has an open
  terminal shows `● Open` at its row's end; choosing it switches to that terminal (the reuse rule).
- **Terminal:** system Back from a terminal does what the orange disc does (Home). The green disc opens the
  **`Terminals`** sheet: every open terminal grouped by host, the current one marked, each row with its `×`
  (same close rules as Home), then two rows: **`Copy screen`** (the visible screen's text to the clipboard,
  with the usual copied confirmation) and **`Gestures & shortcuts`** (opens the shortcuts sheet). The separate
  `Close session` row and pill go. Ctrl+Shift+W closes through the same close function as `×`.
- **Re-activating a terminal shows it as it is.** Switcher, Home thumbnail, `● Open` row: no herdr focus
  change (one terminal serves a whole herdr session now). Only an explicit agent request (inbox row,
  notification) focuses a pane. `TerminalActivations`: `openAgent` and `reopen` share one private path; drop
  the unreachable herdr-with-pane branch of `open()`; `reusable` becomes private (tests use `open`).
- **Host form:** a danger **Delete host** row at the bottom (same confirm as Home's), and one Save: the bottom
  button stays, the top-bar ✓ goes.
- **Or2App is split** (812 lines): the reattach/resume state into its own holder, the Home destination into a
  `HomeRoute`, the share handling, one notices overlay instead of two orders. `pendingResume` and
  `pendingAgent` become one `PendingOpen` (one saver, one effect); a resume or agent tap while another unlock
  runs connects once it ends (today it is silently dropped).
- **Bugs:** the Resume card says `Ssh` (use the transport's `display()`); the Resume card and `Resuming…` /
  `Focusing…` texts use `targetTitle`; a herdr terminal's card detail follows the focused pane (drop
  `Or2App.cwdOf`, use the herdr rule).
- **Tests:** update every Home, host-screen, picker and switcher device test to the new model (compile in the
  gate); JVM tests for the pending-open unification, the close rules, `● Open` marking, Back.

**Implemented (branch `v012/lane-a-ui`).** Where the plan was silent:

- **Host card.** One `Or2Card`: the header row is the click target (`host:<id>`, tap = picker, long press = menu,
  the state description on it); the `⋯` is a 44 dp `IconAction` (`host-menu:<id>`, "Options for <host>") with a new
  `Or2Icons.More`. The thumbnails sit 12 dp in from the card's sides and bottom, 38 % of the card wide; the host pill
  is gone from them (the card names the host): the transport pill is at the top left and the `×`
  (`session-close:<id>`) at the top right. A closed terminal shows a muted `Closed` pill in place of the transport,
  its preview dimmed to 50 %. The menu's **Connect** only connects (the header is the way to the picker). `Connect
  all` is `Chip`-sized `accent` text on the HOSTS header's line, hidden while an unlock runs.
- **Close rules** (`TerminalActivations.close`, `closeAsks(target, closed)`): one function for Home's `×`, the
  Terminals sheet's `×`, Ctrl+Shift+W and the closed strip's Close. Only an *open* shell asks (a closed one has
  nothing left to end); Ctrl+Shift+W on an open shell asks too. Closing the terminal on screen returns Home; closing
  another from the sheet leaves the sheet open.
- **Re-activation.** `reuse`, `launchReuse` and `needsFocus` are gone: a thumbnail, the sheet, a share pick and the
  reattach `Show` navigate directly. A reattach `Reopen` that finds the session's terminal open shows it as it is;
  one that opens a new terminal on a remembered pane target still awaits that open's own focus (as an agent's).
  `openAgent` and `reopen` share `activate(…, focusReused)`.
- **Picker.** `● Open` (`open-mark:herdr:<name>` / `open-mark:tmux:<name>`, `accent` dot) replaces the row's
  Running/Attached marker; herdr's default session counts by `null` or by its listed name (`OpenSessions.of`). The
  UDP line is `picker-udp-blocked`, under the tabs, only with the lists (never with the gate).
- **Terminals sheet.** One section header per host (also with one host), then 44 dp rows (`terminal-tab:<id>`,
  `● Current`, `terminal-row-close:<id>`); `terminals-copy-screen` copies lane B's
  `TerminalChromeState.screenText()` (`SessionScreen` hoists the chrome state; nothing is copied for an empty
  screen) through `ui/Clipboard.copyText` and closes the sheet, relying on Android's own copied confirmation
  (minSdk 34); `terminals-shortcuts` opens the existing **Gestures & shortcuts** sheet. Closing a row leaves the sheet
  open. A terminal that never connected shows the same grouped list (with its `×`) in place of the old Close pill.
- **Navigation.** `NavStack.backOrHome()` from any terminal is Home. `afterPaired` / `afterKeepAlive` return Home
  and `Or2App` opens that host's picker (`homePicker`) as it connects. A saved `host:N` decodes to Home, and what
  was under it is dropped (Home is only ever the bottom).
- **Pending open.** `PendingOpen` (`Resume` / `Agent`, with `started`) and `pendingStep(state, busy, started)`:
  connected opens; an unlock running or a host on its way waits; once nothing runs, one not started connects, one
  started gives up. An agent tap on a connected host opens at once (`agentOpenStart`). The reconnect chip's pending
  is already started (its own connect).
- **Or2App split:** `HomeRoute`, `rememberResume` (`app/Resume.kt`), `ImageShareRoute` (`app/Shares.kt`) and one
  `Notices` overlay (`app/Notices.kt`) in one order everywhere: message, progress, chip, unlock (the unlock card now
  shows over a terminal too). The host-key dialog is the first pending prompt (no host screen to skip).
- **Host form.** The Delete row is its own grouped card under the footnote, off while busy; a deleted host's form
  lands on Home.
- Outside lane A's files: `ui/Icons.kt` (`More`), `androidTest/ui/TopEdgeDeviceTest.kt` (gallery names), the tests
  of the removed API, and, after merging lanes B and C (as the lead asked): `app/OneTimePrompts.kt` (the deprecated
  `offer(use)` and `NotificationUse` removed; `AppActions` reads the one `offer`), and `data/AppDatabase.kt` (the
  unused `Host.moshFailedUntil` getter removed; the host form no longer copies the column). The inbox's dot colour
  and Home's herdr label use lane C's `linkStatusColor` and `agentLabel`. Left for others: `StatusChip`
  (`ui/Components.kt`) is now unused, and `dialogForOtherHost` (`inbox/InboxModel.kt`) is used only by tests.

## Lane B: terminal and UI code (Kotlin: `terminal/*`, `ui/*`, `paste/*`, `keys/*`, `pair/*`, `notify/*`,
## `session/TerminalHeader.kt`, `session/SessionMessages.kt`, the gallery for these)

- **Toolbar:** remove the `Panes` key (the green disc opens the same sheet; remove `openPanes` from
  `TerminalScreen` and its one use in `SessionScreen.kt`) and `History` (a swipe pages, the scroll-to-bottom
  button returns). Keep Copy/Clear while selecting, Ctrl, Esc, Tab, the arrow pad key, Paste, `⇧Tab`, `/`,
  `@`, then apart the composer and keyboard toggles. The row must fit a 411 dp-wide phone without scrolling
  (a JVM or device check on the computed width). Remove the second `/` from the pad's symbol row.
- **One paste path:** `pasteText` everywhere (delete `TerminalView.paste` and `TerminalInput.paste`), so a
  confirmed paste is bracketed when the program asks. **Clipboard:** one `ui/Clipboard.kt` (copy, read with
  `itemCount` checked and `coerceToText`, share) used by the terminal, Keys and pairing; one public-key Copy /
  Share group shared by Keys and pairing.
- **Composer:** closing it (its × or the toolbar toggle) gives focus back to the terminal; a multi-line send
  buzzes once; opening the pad closes the composer as opening the composer closes the pad.
- **Sheets:** `Or2Sheet` owns its body padding and the in-sheet card colour (`SurfaceRaisedRow`), fixing the
  Sessions, Shortcuts and share sheets that draw `Surface`.
- **The shortcuts sheet** gains a touch section first (tap, long press to select then Copy, swipes, pinch),
  then the hardware-keyboard list; lane A opens it from the switcher.
- **Simplify:** the arrow pad from a table (`PadActions` → one `send`); one confirm dialog for "Send N lines?"
  and "Paste N lines?"; one hardware-shortcut helper for the view and the composer; one notice strip in the
  terminal card; `terminalNotice` only for a closed terminal (its Connecting/Authenticating branch is never
  shown; update the gallery and `ui.md`); `TransportBadge` and `Badge` reduced to what ships; inline
  `identity(agent)`.
- **Dead code:** unused icons (`Minus`, `Mic`, `Grid`, `Fingerprint`), `Or2Type.MonoLarge`, `ExtraKeys`,
  `ToolbarActions.toggleAlt` and `ToolbarState.alt`, `sessionErrorMessage`, test-only `herdrStateMessage`,
  `headerTitleText`, `composerWithPath`, never-passed parameters (`Segmented.icons`, `Or2Card.shape`,
  `StatusDot.size`, `MonoBlock.container`, `PillButton.container`/`content`, `AttentionCard.icon`/
  `subtitleColor`, `NoticeStrip.icon`), `DemoFrames.SURFACE`; stale comments (`KeyToolbar` lists Alt; "greys
  its pill"; `PairInstallKeyScreen`'s doc; `ui.md`'s 30 vs 34 dp toolbar boxes).

**Implemented (branch `v012/lane-b-terminal`).** Kotlin only; no FFI change. Where the plan was silent:

- **Toolbar as a table.** `ToolbarKey` (tag, description, label or icon), `toolbarKeys(selecting)` and
  `ToolbarToggles`; the screen runs a key with one `press(key)` (`ToolbarActions` is gone; Ctrl+Shift+V and
  Ctrl+Shift+Enter press the same keys). **While text is selected** `Copy` and `Clear` lead the row and the typing
  keys (`⇧Tab`, `/`, `@`, which would clear the selection anyway) give way to them, so the row fits in both states.
  The `History` key's `TerminalView.pageUp` and `Or2Icons.History` went with it.
- **The fit check** (`KeyToolbarTest`, JVM) computes the row from the tokens (`KeyTouchWidth` 34, `KeyWidth` 30,
  `KeyLabelPadding` 6, `ToolbarMargin` 8, `ToolbarPadding` 6, `ToolbarTogglesGap` 4 dp; a label is its characters
  at DroidSansMono's 0.6 em of 12 sp, any non-ASCII glyph such as `⇧` a full em, at font scale 1): 405.6 dp
  without a selection, 384.8 dp with one, both within 411. On a device, `TerminalChromeDeviceTest` lays the
  screen out 411 dp wide and asserts the key row's scroll range is 0 in both states, and `TerminalVisualDeviceTest`
  asserts every key sits inside the pill on the phone. The row keeps its horizontal scroll only as the fallback for
  a large system font.
- **Arrow pad.** `PadRows` (`PadKey`: tag, description, glyph, `TerminalKey`, modifiers; Clear-line is Ctrl-U) and
  `PadExtras` (label to key); `ArrowPad(send, alt, toggleAlt)`.
- **Paste.** Every paste is `TerminalView.pasteText` (the toolbar, Ctrl+Shift+V, a confirmed "Paste N lines?", an
  image's paths). One `PendingLines` dialog serves "Send N lines?" and "Paste N lines?", body `They will run as
  typed, one line at a time.`, confirm tagged `composer-send-confirm` or `paste-confirm`.
- **One buzz.** The composer's send button buzzes only when the message went out; a confirmed one buzzes on the
  dialog's **Send** alone. Closing the composer (×, toggle, Ctrl+Shift+Enter, opening the pad) calls
  `requestFocus` on the terminal view.
- **Shortcut helper.** `consumeShortcut(event, fire, passes)` serves the view and the composer (which passes
  Paste and Copy to its text field).
- **Clipboard.** `ui/Clipboard.kt`: `copyText`, `clipboardText` (`itemCount` checked, `coerceToText`),
  `shareText`; no copy toast (Android 13+ confirms a copy itself; or2's minimum is 14). `PublicKeyActions`
  (`keys/KeysScreen.kt`, tags `<prefix>-copy` / `-share`, a `more` slot for the key sheet's Delete) is the one
  group. `Or2Application`'s host-clipboard write (app layer) is left to its owner.
- **Sheets.** `Or2Sheet` pads its scrollable body (12 dp at the sides and below); a `scrollable = false` sheet
  (the session picker) pads itself. `LocalCardColor` is `surface`, and `surfaceRaisedRow` inside a sheet:
  `GroupCard` and `Or2Card` default to it.
- **Shortcuts sheet.** Titled `Gestures & shortcuts` (the Terminals sheet's row and Ctrl+Shift+/): `TOUCH`
  (`TouchRows`: Tap, Tap a link, Long press, Drag ↑ / ↓, Swipe ← / →, Two fingers ← / →, Two fingers ↑ / ↓,
  Pinch) with a muted note that swipes move tmux and herdr, then `KEYBOARD`.
- **Notice.** `TerminalNotice(text, tone, busy, action)`; `uploadNotice` returns one too (`UploadNotice` is gone);
  the card shows `terminalNotice(state) ?: upload` in one strip, keeping the tags `terminal-notice` /
  `terminal-status` / `terminal-close` and `upload-notice` / `upload-status` / `upload-action`. The gallery's
  `terminal-connecting` is gone.
- **Badges.** `TransportBadge(transport, modifier)` and `Badge(text, container, content, modifier)` (the small mono
  pill only; `Or2Type.Badge` is gone). The header's green disc reads `Terminals` (tag `terminal-panes` kept).
- **For lane A.** `TerminalChromeState.screenText()` returns the visible screen's text while a `TerminalScreen`
  shows that state (`TerminalView.screenText`), for the Terminals sheet's **Copy screen**: hoist the chrome state
  and pass it to `TerminalScreen`. A sheet no longer pads its own body or asks for `SurfaceRaisedRow` cards: Home's
  host options sheet and the Sessions (Terminals) sheet still pad themselves on this branch, so they show a double
  gutter until lane A drops their `padding(horizontal = Gutter).padding(bottom = Gutter)`.
- **Outside the lane's files:** `session/SessionScreen.kt` (the `openPanes` line; `TerminalCard`'s strip, KDoc and
  imports; `TransportBadge`, which lives there) and `home/HomeScreen.kt` (`small = true` dropped from the
  `TransportBadge` call, forced by the signature).

## Lane C: Kotlin app-layer cleanup (`connection/*`, `service/*`, `data/*`, `inbox/InboxModel.kt`,
## `MainActivity.kt`, `app/OneTimePrompts.kt`; not `Or2App.kt`, `TerminalActivations.kt` or screens)

- Remove the dead per-host mosh memory (`markMoshFailed`, `MoshFailureStore`, `clearMoshFailure`, the getter;
  keep the Room column, marked unused, so no migration), and its tests.
- Remove test-only production API: `HostConnections.awaitCapabilities`, `hasOpenSession`,
  `InboxModel.linkStatuses`, `MoshServerLedger.allPids`, `NavStack.tab` (if unused after lane A, else leave),
  `Timing.mark(detail)`, `NetworkChanges(initial)`, `ServiceController.current` (move what tests need into test
  helpers).
- One offer instead of `NotificationUse` (keep the prefs keys and the legacy `notifications_asked` check); one
  battery-request intent.
- `HostConnections`: one `currentPort(hostId, requireConnected)` helper for the six lookups; `ActiveTerminal.isOpen`
  for the repeated "not closed, not retired, not disconnecting" test; drop checks the code already implies
  (`retired` implies `disconnectRequested`); the repeated recheck condition; misplaced KDoc.
- One `combineEach` helper for the nine "flatMapLatest, empty, combine" flows; `serviceSnapshots()` collected
  once and shared by the application and the service.
- One link-status message/colour and one agent-label function next to `LinkStatus` (lane A may use them).
- Stale comments (`hasOpenSession`, `moshFailedUntil`, `AppActions.notifications`).

**Implemented (branch `v012/lane-c-app`).** The mosh memory's DAO API and `MoshFailureStore` are gone and
`saveHost` no longer clears it; `hosts.mosh_failed_until` stays in the entity and the v4 schema, marked unused
(no migration, `4.json` unchanged). The test-only API is gone (tests carry `hasOpenSession` and
`recordedServerPids` helpers; `NetworkChanges` starts unseeded, the watch seeds it); `NavStack.tab` is lane A's and
stays. One `NotificationPermission.offer` (the agents card's dismissal key; `notifications_asked` still counts as
an earlier request) and one battery-request intent. `HostConnections.currentPort(hostId, requireConnected)` serves
`openTerminal`, `focusHerdrPane`, `replyToPane`, `scrollTarget`, `uploadImage` and `navigate`; `ActiveTerminal.isOpen`;
`requestBackground` trusts `openTerminal`'s recheck. `combineEach` (in `TerminalFlows.kt`) carries the nine
flows; `Or2Application.serviceSnapshots` is the one shared collection (`shareIn` its process scope, replay 1) that
the service's controller and the starter read. `linkMessage`, `linkStatusColor` and `agentLabel` sit by
`LinkStatus`. Left for the merge, in files of lane A: `Or2App.kt`'s `AppActions` default still calls the
deprecated `offer(NotificationUse.CONNECTION)` shim (switch it to `.offer`, then delete the shim and the enum), and
its `notifications` KDoc is stale; `HostFormScreen.kt` still copies `Host.moshFailedUntil` (then the getter can go);
`docs/ui.md` still names `NotificationUse.AGENT_ALERTS`; `InboxScreen`, `HostScreen`, `PickerGate` and
`HomeModel.herdrDetail` can use `linkMessage`, `linkStatusColor` and `agentLabel`.

## Lane D: Rust core and FFI (`core/*`; Kotlin only where an FFI change forces it)

- **Bugs:** tmux swipe-scroll targets the terminal's original session after a session switch (pass the
  terminal's tmux client id and resolve the session shown, as `navigate` does); the mosh connect timeout is
  applied twice for an explicit Mosh choice (compute the deadline once); a reply can be sent after its
  deadline when the cached socket is dead (budget the re-locate); a rejected subscription is retried on every
  invalidation; the receive drain can delay a disconnect by about 1.3 s.
- **Remove what only the probe or tests use:** the session host-key path (`ConnectRequest`/`ConnectError`,
  `approve_host_key`/`reject_host_key`, `Command::ApproveHostKey`/`RejectHostKey`, the
  `AwaitingHostKeyDecision`/`Authenticating` session states and their FFI errors; `contract_probe_session`
  wraps the probe host's `open_terminal`), the test-only mosh start path (`start`/`start_with`/`spawn`,
  `LinkControl`, `HealthObserver`; `tests/mosh_live.rs` moves to `run_session`), the herdr test-only wrappers
  (`watch`, `run`, `focus_pane`, `watch_with_timing`, `watch::run`, `Directory::locate`; `tests/herdr_live.rs`
  re-pointed), the peer-address plumbing, `transport::race`, `stranded_servers`, `probe::herdr_sessions`/
  `probe`/`probe_entries`, `HostHandle::open_terminal(_with)`, the ssp leftovers, `Link.peer_ipv6`.
- **FFI surface the app never reads:** `ViewportScroll::Top`, `terminal_size`/`TerminalSize`/`TerminalError`,
  `BuildInfo.minimum_android_sdk`/`renderer`/`Renderer`, `TmuxSession.created_unix`/`activity_unix`,
  `LinkHealth.since_ack_ms`, `HostCapabilities.utf8_locale` (unless Rust needs it internally), and the herdr
  view fields Kotlin never reads (workspace/tab/pane `focused`, `agent_status`, pane `tab_id`/`workspace_id`/
  `label`/`title`, agent `title`/`focused`, view `protocol`; check each against Kotlin main before removing).
  Fewer fields also means fewer redundant view deliveries. **FFI API 18.** `ReplyRoute`, `Session.state()` and
  the contract probes stay.
- **One of each:** a `spawn_query` helper and `From` impls for `dispatch`'s nine query arms and the
  `HerdrError`/`TmuxError` → `HostError` mappings, one `program()` helper; one input-command helper shared by
  the SSH pump and the mosh driver; one herdr cached-socket call/retry helper for focus, scroll, navigate and
  watch; a `HostHandle` query helper; `ExecOutput::stderr_line()`; one `or2_find` script fragment; `Client`/
  `relay` fixed to `HostEvent`; two connect entry points instead of eight.
- `pub(crate)` for the mosh `ssp`/`ghostty`/`bootstrap` modules and the tmux helpers used only inside, so the
  dead-code lint polices them. Stale comments (M1 driver, `start`/`HealthObserver`, `PROBE_PANES` doc on the
  wrong const, `docs/design.md`'s "API version 5").
- Not now: a generic lifecycle type, gating the probe behind a cargo feature.

**Implemented (branch `v012/lane-d-rust`).** Where this differs from the sections above, this note is
the contract.

- *Bugs, each with a test.* `scroll_target` takes the terminal's tmux client id (FFI: a last
  `client_id` argument; Kotlin passes the shown session's `clientId()`, as for `navigate`) and scrolls
  the session that client shows (`tmux::shown_session`, shared with `navigate`). The mosh connect
  timeout is computed once for the socket open and the first datagram. A reply's prompt re-locates a
  dead cached socket only within the time its own bound leaves before the deadline, and is never sent
  past it (`NO_TIME`). A pane subscription refused for another reason than `pane_not_found` is asked
  again only when a read finds other panes. A refused receive (ICMP) ends the driver's turn after
  20 ms, so a disconnect is served at once; the goodbye no longer spins on refusals.
- *Removed.* The session host-key path (sessions go `Connecting` → `Connected` → `Closed`; host keys
  are the host's); `contract_probe_session(columns, rows, listener)` (`EmptyDimension` for a zero
  size) runs the probe host's SSH terminal script, row 0 `or2 contract probe`. mosh has one entry,
  `run_session` (exported with `Plan`/`Ended` under `test-support` for `mosh_live`); roaming is
  `Session.roam`. herdr: `run_in(host, herdr, directory, session, driver, Timing)` and
  `focus_pane_in` are the entry points; `herdr_live` runs them. `HostHandle::peer_addr` (mosh still
  pins the IP the TCP connection reached, inside the driver), `transport::race` (`race_with`),
  `probe`/`probe_entries`/`herdr_sessions` (`probe_within` is public), `stranded_servers`,
  `Link.peer_ipv6`, the ssp leftovers. `HostHandle::open_terminal(target, transport, size, budget,
  observer)` is the one open.
- *FFI API 18.* Gone: `terminal_size`/`TerminalSize`/`TerminalError`, `BuildInfo.minimum_android_sdk`/
  `renderer`/`Renderer`, `ViewportScroll::Top` (core too), `TmuxSession.created_unix`/`activity_unix`
  (core keeps them for sorting), `LinkHealth.since_ack_ms` (core keeps it), `HostCapabilities.utf8_locale`
  (core keeps it for the mosh bootstrap), and the herdr fields listed above, from the core projection too
  (`HerdrView { version, focused_pane_id, workspaces { workspace_id, number, label }, tabs { tab_id,
  workspace_id, number, label }, panes { pane_id, agent, cwd }, agents }`; agents lose `title` and
  `focused`): a snapshot that changes only an unread field is no longer delivered.
- *One of each.* `spawn_query`/`spawn_until_closed` for every dispatch arm that runs a task,
  `From<RemoteError | TmuxError | HerdrError> for HostError`, `HostCapabilities::program(Program)`,
  `HostHandle::query`, `TerminalEngine::input_bytes`, `Directory::with_socket` (focus, scroll,
  navigate, the reply's prompt, the watch's opens; a watch's retry invalidates the directory),
  `ExecOutput::stderr_line`, one `or2_find` macro, `Client`/`relay` on `HostEvent`, and
  `connect_host` plus `connect_host_with(transport, datagrams, request, observer, options) ->
  (HostHandle, Receiver<SshRemote>)` (tests). `HostHandle::focus_herdr_pane` and `stop_mosh_server`
  now validate before checking the connection. mosh `ssp`/`ghostty`/`bootstrap` and tmux's inner
  helpers are `pub(crate)`.
- *Kotlin, forced by the FFI.* The port's `scrollTarget` gains `clientId`; `SessionMessages` loses the
  two removed session states and errors; tests, device tests and the debug gallery/probe follow the
  new records and `contractProbeSession`. Skipped: `terminal_session`'s own not-installed checks keep
  their `SessionFailure` (they are not host queries); the older contract sections are left as written.

## Integration

The lead merges A, B, C and D, runs the full gate (`docs/build.md`) and the device suite on the
`.devicetest` app, then a Codex review of the whole change, then installs a signed build for the owner's QA.

# v0.1.2: agents in the picker (owner QA, 2026-10-03)

The owner, from a host card: *"the archlinux host is not showing all the agent sessions active in the herdr default
session"* and *"the refresh button does nothing"*. Debugged on the phone: the Inbox lists every agent correctly; the
picker's herdr tab lists only herdr **sessions** (`default (default)`, `or2-spike`), never the agents inside them, so
from a host the agents look missing and reaching one needs the Inbox. Refresh works (a new tmux session appeared)
but shows no feedback, and on the herdr tab it can change nothing the owner looks for. The tmux list is read once per
connection, so it goes stale between openings. Kotlin only (lane Picker; `host/SessionPicker.kt`, `app/HostPicker.kt`
and their tests/gallery).

- **The herdr tab lists agents.** Under each running herdr session, its agents grouped by workspace (the workspace
  label as a small muted header), each row: the agent label (`agentLabel`), its status dot and word
  (working / blocked / done / idle, the Inbox's colours), and the pane's cwd muted. Data: the host's live herdr
  views (the same source as the Inbox: `HostConnections.herdrViews()` / `inbox`), so it is current without a refresh.
  A session the app does not watch yet, or one not running, shows as today (one row, `Not running` muted).
- **Tapping an agent** opens the herdr terminal focused on that pane: the Inbox tap's path
  (`TerminalActivations.openAgent` / `launchOpenAgent`), which reuses the session's open terminal. The sheet closes.
- **A `Whole session` row** under each running session keeps today's behaviour (open or switch to that session's
  terminal as it is); it carries the `● Open` mark when that session has an open terminal.
- **Labels:** the default session is `default` (no `(default)` suffix); a running session with no agents says
  `No agents` muted under its `Whole session` row.
- **tmux:** the list is re-read each time the picker opens (and when the tab is shown), with a small spinner in place
  of the list until the first answer; a later re-read keeps the old list visible with the spinner beside the
  `Refresh` row. **Refresh** stays on the tmux tab only, shows the spinner while it runs, and also re-probes the
  host's capabilities (new herdr sessions), as today. The herdr tab has no Refresh.
- **Tests.** JVM: grouping by session and workspace, labels, status mapping, the default-session label, an agent tap
  calling the agent path with host/session/pane, `Whole session` keeping the reuse rule. Device: the herdr tab with
  agents from fakes (rows, tap opens the terminal focused), the tmux spinner. Gallery: `picker-herdr` shows agents.

**Implemented (branch `v012/picker-agents`).** Kotlin only. Where the spec was silent:

- **Layout.** Each live session is a `SectionHeader` with its name (as the Terminals sheet heads a host), then one
  `surfaceRaisedRow` card: `Whole session` (tags `herdr:<name>` / `herdr-open:<name>`, the session row's own, so
  `● Open` and the reuse rule are unchanged), then per workspace a muted mono-small header
  (`herdr-workspace:<name>:<label>`) and its agent rows (`herdr-agent:<name>:<pane>`, the status
  `herdr-agent-status:<name>:<pane>`), or `No agents` (`herdr-no-agents:<name>`). Sessions without a live view keep
  today's single rows, in one card **after** the live ones (`pickerHerdrSessions`: live first, each part in the
  listing's order), so a plain row never reads as part of the session above it. A live view counts as running
  whatever the cached probe said.
- **Agent rows.** `agentLabel(agent, pane.agent)`, else `agent`; the cwd is the agent's, else its pane's, in mono
  small with the path's end kept (`StartEllipsis`); the status is the picker's marker (`● Working`: the Inbox's
  `statusColor` / `statusLabel`; Working pulses, Blocked's word is `attention`); 56 dp with a cwd. Workspaces follow
  herdr's numbers (empty ones left out), agents by tab then pane; agents in no listed workspace come last without
  a header.
- **The default session's label** is its listed name (the owner's is `default`); a default session named otherwise
  shows that name. Its view is the one keyed null (`herdrViews()`), and it opens with a null session.
- **Agent path.** `Or2App.openAgent(hostId, hostLabel, session, paneId)` is the one function behind the Inbox row
  and the picker (`OpenAgent`, through `HomeRoute` and `HomePickerSheet`): `message(null)`, then
  `launchOpenAgent`. `PickerChoices` (`app/HostPicker.kt`) holds where each choice goes, the sheet closing first.
- **tmux re-reads.** The source reads on the picker's opening (it lives as long as the sheet), when the tmux tab is
  shown (`tmuxShown`, skipped while a read runs, so opening on the tmux tab reads once) and on Refresh (which also
  re-probes; a read in flight is restarted). `refreshing` (the 14 dp spinner beside Refresh, `refresh-spinner`) is
  a re-probe or a read after the first answer; until that answer `tmux-spinner` (16 dp) stands in place of the
  list. Each read and probe tracks itself by a token, so a cancelled one never clears its successor's spinner.
- **Words.** The herdr tab's probe error reads `Could not query the host. Refresh it from the tmux tab.` (it has no
  Refresh); the tmux tab's keeps `Try Refresh.`
- Outside the lane's files: `app/Or2App.kt` and `app/HomeRoute.kt` (the shared `openAgent`), the device fakes'
  `UiPort.focused`, and `docs/ui.md`'s picker paragraph.

**Owner, on the phone (2026-10-03):** *"don't use the title `Whole session`; that's not how terminal lovers would
like to see this app."* The session's own row is now named as a tmux row is: the session's name (the default one
`default`), then `4 agents` / `1 agent` / `no agents` in muted mono; the separate section header and the `No agents`
line are gone. Tags (`herdr:<name>`, `herdr-open:<name>`, `open-mark:herdr:<name>`) are unchanged.

# v0.1.3: zero-config Reply (owner request, 2026-10-03)

Reply from a notification needs herdr's integration inside the agent (contracts.md, "Reply from an agent notification";
status.md, "Reply needs herdr's integration"); the owner found pi without it and no hint why. And or2's agents need
herdr itself: a host without herdr gets no inbox, no notifications, no Reply. Zero configuration: or2 sets this up,
the user only confirms. herdr's own install is `curl -fsSL https://herdr.dev/install.sh | sh` (also Homebrew, mise,
Nix: https://herdr.dev/docs/install/); integrations are `herdr integration install <id>` and their state
`herdr integration status` (text only: one line per integration, `<id>[ (experimental)]: <state> (<path>)`, state
`current (vN)`, `not installed`, or outdated). A running agent loads its integration when it next starts.

The integrations herdr 0.9.3 offers, with the agent's executable where it differs from the id: `pi`, `omp`,
`claude`, `codex`, `copilot`, `devin`, `droid`, `kimi`, `opencode`, `kilo`, `hermes`, `qodercli`, `qwen`,
`cursor` (`cursor-agent`), `mastracode`, `antigravity-cli` (`agy`), `grok`, `letta`. herdr's agent kinds map to them
the same way (`agy` → `antigravity-cli`); a kind without one (e.g. `amp`, `gemini`, `cline`) has no integration.

## Lane Pair: `or2-pair` (Rust, `core/or2-pair`)

- **herdr missing:** today an `Info` line points at herdr's docs. It becomes a `Warn` that says what is lost and the
  fix: `herdr not found: or2's agents inbox, notifications and Reply need it` then
  `install it: curl -fsSL https://herdr.dev/install.sh | sh` and `(or Homebrew, mise, Nix: https://herdr.dev/docs/install/)`.
  It is a recommendation: or2-pair never runs another project's installer. Pairing goes on.
- **herdr present: Reply for the agents on this host.** After the checks, or2-pair runs `herdr integration status`
  (through `hints::Commands`, time-limited; a failure or an unreadable answer is an `Info` line and nothing more),
  finds which agents are installed (their executable in the program directories the checks already search), and
  for those whose integration is `not installed` or outdated asks once, on the rail:
  `Set up Reply for pi, opencode? (runs herdr integration install for each) [Y/n]`. Yes runs each install
  (time-limited) and reports each on the rail (ok, or its failure's first line); then one line: running sessions of
  those agents load it when they next start. No, or no terminal (stdin not a TTY), prints the commands instead.
  Agents whose integration is current are one `Ok` line (`Reply ready for claude, codex`). Agents not installed on
  the host are not mentioned.
- **Tests:** the status parser on real 0.9.3 output (current, not installed, experimental, an outdated line, garbage),
  the executable mapping, the prompt (yes, no, non-TTY), an install failure, herdr missing per platform, the rail text.
  `tests/flow.rs` covers the step with a scripted `herdr`.
- **Implemented (branch `v013/pair-reply`).** `core/or2-pair/src/reply.rs`, a step of `run.rs` after the checks are
  drawn (and after a failed check has stopped the run) and before the code question; the checks still only look.
  `hints::Commands` gained `exec` (exit status, stdout and stderr apart). Decisions where the above was silent:
  `--check` and `--manual` run the step but never ask or install (they promise no change: commands printed);
  "a terminal" is standard input being one (`Env::interactive`), so the test host, whose code comes from a pipe,
  never asks; Enter, `y` or `yes` is yes, anything else (or the end of input) is no; outdated agents are named
  `<id> (outdated)` in the question; a failed install's line is the first non-empty line of its stderr (else stdout),
  with `run it yourself: herdr integration install <id>`; "running sessions … load it" names only the agents set up;
  an id herdr reports that is not in the list is taken to have an executable of the same name; status and each
  install get 15 s; on Windows the missing-herdr line points at herdr's docs only (no `curl | sh`). The Codex note
  shows whenever herdr and `codex` are found and `~/.codex/config.toml` (not `$CODEX_HOME`) does not set the key
  (`[features]`, a dotted `features.daemon_auto_start` or an inline `features = {…}`), even when herdr's status
  could not be read. The binary's tests set `OR2_PAIR_TEST_PROGRAM_DIRS` (`test-support` only) so the machine's own
  herdr and agents are never run.

## Lane App: Enable Reply from the phone (FFI API 19, Kotlin)

- **Rust/FFI:** `HostConnection.install_herdr_integration(id)`: the id must be one of the list above (else
  `InvalidName`, nothing sent); runs `<herdr path from the capability probe> integration install <id>` as one exec on
  the host connection (no shell interpolation: the id is from the allowlist), within the exec timeout; `Ok` on exit 0,
  else `CommandFailed` with the first line of its stderr. herdr not installed → `NotInstalled`.
- **Kotlin:** an agent herdr reports without a session (no `reply_identity`) whose kind maps to an integration gets
  **Enable Reply** in its Inbox row (a compact text action) and in its notification (a second action instead of
  Reply). It asks once (`Enable Reply for pi on archlinux? or2 installs herdr's pi integration there. Restart pi
  afterwards.`), runs the install, and says the outcome (`Done. Restart pi to reply to it.` or the failure). An agent
  whose kind has no integration shows nothing new. An agent whose integration is installed but still has no session
  (Codex with herdr 0.9.3, whose session report herdr refuses) shows nothing new either; the release notes explain it.
- **Tests:** Rust: the allowlist, the exec's argv, exit codes, not installed. JVM: the kind → id mapping, which rows
  and notifications offer it, the confirm and outcome texts. Device (compile): the Inbox action.

**Implemented (branch `v013/enable-reply`).**

- **Rust/FFI (API 19).** `herdr::install_integration` (`core/or2-core/src/herdr/integration.rs`) as specified; exit
  126/127 (herdr gone since the probe) is `NotInstalled` too. *Addition:* `HostConnection.herdr_integrations()`
  reads `<herdr> integration status` (one exec) as `HerdrIntegration { id, state: Current | Outdated | NotInstalled }`
  for the allowlisted ids only (`(experimental)` dropped, other lines skipped, no paths cross the FFI), because "an
  agent whose integration is installed shows nothing new" needs the state. The contract probe starts with `pi` not
  installed, `claude` and `codex` current, `opencode` outdated, `droid` not installed; an install makes one current,
  except `droid`, which fails with `error: cannot write the hook: permission denied`.
- **When the status is read.** Once per connection (and again after each `refresh`), the first time a live view
  shows an agent without a session whose kind has an integration; never for a host whose agents all have Reply. A
  failed read offers nothing until the next refresh. A successful install marks its id current, so the offer goes
  at once (the agent itself gets Reply when it restarts).
- **The offer** (`notify/EnableReply.kt`, `enableReplyFor`): no `reply_identity`, a kind that maps (`cursor-agent`
  → `cursor`, `agy` and `antigravity_cli` → `antigravity-cli`, case-insensitive), and a known state that is not
  `Current` (not installed or outdated). Unknown state, an id this herdr does not list, or a kind without an
  integration: nothing.
- **Inbox.** The row's **Enable Reply** is an `accent` 12 sp text action under the status word; the dialog is
  `EnableReplyDialog` ("Enable Reply?", **Cancel** / **Enable**). The agent label is the row's name (herdr's display
  name), the integration its id: `Enable Reply for Claude Code on <host>? or2 installs herdr's claude integration
  there. Restart Claude Code afterwards.`
- **Notification.** An alert without an agent instance but with an offer gets one action, **Enable Reply**: an
  immutable, one-shot activity PendingIntent to `MainActivity` (`ACTION_ENABLE_REPLY`, data
  `or2-agent-enable:<tag>#<nonce>`, the app's open token, the integration, title and host). The activity takes it
  only with the app's token, an allowlisted id and the pane's current capability (`AgentAlerts.admitEnableReply`:
  the same one-shot nonce as Reply, while the notification is up), cancels the notification as a tap does, and
  hands the request to `EnableReplyRequests`; the dialog shows over any screen (after a host-key prompt). Nothing is
  installed from the notification.
- **Outcome.** While it runs the message card says `Installing herdr's pi integration on <host>…`; then `Done. Restart
  pi to reply to it.` or `Not enabled: <host> is not connected` (no connection, or closed; nothing connects for
  it), `Not enabled: herdr is not installed on <host>`, the `CommandFailed` reason (80 characters at most),
  `Not enabled: herdr has no such integration` (`InvalidName`), `Not enabled: the host did not answer` (45 s Kotlin
  bound). It runs in the activity's view-model scope, so a rotation does not cut it short.
- **Tests.** Rust: `herdr::integration` (allowlist, argv, exit 0/1/2/126/127, a lost connection, status of real 0.9.3
  output with an outdated line, garbage), `host` (allowlist before anything is sent, the reply path, `Closed`),
  `ssh::connection` (one exec through the probed herdr, stderr's first line, `NotInstalled` without herdr). JVM:
  `EnableReplyTest` (mapping, offer, texts, outcomes, the notification's offer, the capability taken once, the
  intent parse), `InboxModelTest` (rows; the status read once a view needs it, an install marking current, a failed
  read, not connected), `HostContractTest` (both calls across the FFI to the probe). Device (compile):
  `InboxUiDeviceTest` (the row's action and the dialog), `AgentNotificationsDeviceTest` (the action and its intent),
  `TopEdgeDeviceTest` (the dialog). Gallery: `inbox` (a `pi` row with Enable Reply on build-box) and
  `inbox-enable-reply`.

**Codex (researched 2026-10-03).** Codex 0.160 runs its `SessionStart` hooks inside a shared, long-lived app-server
daemon, not in the pane's own Codex process; the daemon keeps the `HERDR_PANE_ID` of the pane it was first started
from, so its hook reports name a pane that may be gone, and herdr's `pane.report_agent_session` answers
`pane_not_found` (or, if that pane still exists, binds the session to the wrong pane). Seen on the owner's host: the
daemon carried `w1:pK`, which no longer existed; every Codex report since 2026-10-02 failed. Upstream:
herdrdev/herdr#4649 (open, `needs-upstream-fix`, tracking openai/codex#48500, #44902, #48880, #24638). Workaround:
`[features] daemon_auto_start = false` in `~/.codex/config.toml`, stop the running daemon when no session uses it, and
run Codex with `--no-daemon`. **Lane Pair** adds: when `codex` is installed and its config does not set
`daemon_auto_start = false`, one `Info` line under the Reply step: `codex: Reply may not work while Codex runs its
shared daemon (herdr#4649)` then the workaround lines. It changes nothing in Codex's config. **Lane App** shows nothing
new for an agent whose integration is installed but has no session (the Codex case), as above.

**Codex review of v0.1.3 (`1c81251..fcfd532`):** two P3, both fixed with tests. A failed `herdr integration status`
read now forgets what was known (it may be stale), so nothing is offered until a read succeeds
(`InboxModelTest.aHostsIntegrationsAreReadOnceAViewNeedsThemAndAnInstallMarksItsOwnCurrent`). `codex_daemon_off`
counts only a bare boolean `false` at the exact key: a quoted string, or `daemon_auto_start=false` written inside one,
never does, and a `#` inside a string is no comment (`reply.rs`, `the_codex_daemon_setting_is_found`).

**Wording (lead, on the phone):** the confirmation's title already asks (`Enable Reply?`), so its body only says what
happens: `or2 installs herdr's pi integration on archlinux. Restart pi afterwards.`

# v0.1.4: the herdr Spaces sheet, and agents named as herdr names them (owner request, 2026-10-03)

The owner, in a herdr terminal on the phone: herdr's own `switch` text (top right of herdr's compact status bar, which
opens herdr's folded sidebar) is unintuitive; they want an or2 control by the header's two discs. And: *"why can't we
read the agent workspace, tab, pane names? that'll help identify the target agent, just like herdr."* herdr reports
spaces (`workspace.label`: `~`, `code`, `or2`), tabs (`tab.label`, numbers unless renamed), an agent's `name` (when
started by name) and the **agent's terminal title** (`terminal_title_stripped`: the task an agent is on, e.g.
`Repository context gathering`, `Review v013 brief | or2`, `π - cliproxyapi.service - akram`). The last is what
herdr's sidebar shows and the best way to tell two agents apart; the streamline removed it from the FFI (lane D)
because no screen read it then. One lane (Rust and Kotlin).

- **The title comes back (FFI API 20).** `HerdrAgent.title`: herdr's `terminal_title_stripped`, trimmed, with a
  leading spinner or status glyph and its following space removed (Claude Code animates one: Braille spinners
  U+2800–U+28FF, `✳ ✶ ✻ ✽ ✢ · * ●` and similar single symbols), capped at 120 characters, empty → absent. A view whose
  only change is an agent title is delivered at most once per second per session (the newest wins), so a spinner
  never churns the UI; any other change is delivered at once, as today. Generated types are untouched (projection in
  `herdr/project.rs` / `view.rs`, FFI `herdr.rs`).
- **Agents named herdr-style, everywhere an agent appears** (Inbox row, the picker's agent rows, the Spaces sheet,
  notifications): the agent label as today (name, else display name, else kind), then the title on its own line in
  muted text when there is one and it is not the same as the label; then `space · tab` and the folder as today.
  A notification's text is `Needs input` / `Done`, followed by ` · <title>` when there is one.
- **The Spaces disc.** A third header disc right after the orange (Home) and green (Terminals) ones, same size and
  spacing, in `accent` (blue), with a grid/spaces glyph; shown only on a herdr terminal (absent for shell and tmux, so
  those headers do not change). Description `Spaces`.
- **The Spaces sheet** (`Or2Sheet`, title `Spaces`, the session name muted under it when it is not the default):
  from the terminal's live herdr view, each space (herdr's order) as a small section header with its label; under it
  one card with its tabs (herdr's order), each tab a row `tab <label>`, and under each tab its panes that hold an
  agent as agent rows (status dot and word in the Inbox colours, label, title); the focused tab and pane marked
  `● Current`. Tapping a tab focuses it, tapping an agent focuses its pane, through the existing focus path
  (`TerminalActivations` / `focusHerdrPane`, or herdr's tab focus if the API has one; else the tab's first pane),
  and closes the sheet; the terminal shows it as herdr draws it. Spaces with no tabs are left out. Nothing else in
  the sheet (no create, rename or close: herdr does those).
- **Tests.** Rust: the title projection (glyph stripping, cap, empty), the title-only throttle, other changes
  undelayed. JVM: label/title lines, the notification text, the sheet's grouping and current marks, a tap's focus
  call. Device (compile + run): the disc only on herdr terminals, the sheet, a tap. Gallery: `spaces`.
