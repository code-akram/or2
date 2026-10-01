# Android app: build and verify

The Compose app loads `or2-ffi` through generated UniFFI Kotlin/JNA bindings. Host settings
(with their ordered address lists), encrypted key records and trusted host keys live in Room
(schema version 4, exported to `android/app/schemas/`). `or2-core` remains free of Android,
UniFFI and persistence dependencies. The production connector calls `connect_host` and opens
terminals with `HostConnection.open_terminal`; the contract probes (`contract_probe_session`,
`contract_probe_host`) are used only by tests. The terminal screen embeds the Canvas terminal
with IME and a floating key toolbar, arrow pad and composer, keeping the final displayed frame
visible through `Closed`; Home (open sessions as live thumbnails, host cards) is the start
destination and the agents inbox its sibling. Visuals follow [the UI system](ui.md) through one
theme (`ui/Theme.kt`, `ui/Components.kt`, `ui/Icons.kt`).

## Shared user-local toolchain

Source `scripts/env.sh` from the repository root before running builds. It provides defaults
without changing shell startup files; existing `JAVA_HOME`, `ANDROID_HOME` and
`ANDROID_NDK_HOME` overrides win. The following tools are installed on the Arch runner:

| Tool | Version | User-local location |
|---|---|---|
| Eclipse Temurin JDK | 17.0.20.1+1 | `$HOME/.local/share/jdk/17` (versioned symlink) |
| Android command-line tools | build 15859902 | `$ANDROID_HOME/cmdline-tools/15859902` |
| Android platform | API 36, revision 2 | `$ANDROID_HOME/platforms/android-36` |
| Android build tools | 35.0.0 | `$ANDROID_HOME/build-tools/35.0.0` |
| Android platform tools | 37.0.1 | `$ANDROID_HOME/platform-tools` |
| Android NDK | r30 / 30.0.16248370 | `$ANDROID_HOME/android-ndk-r30` |
| Gradle | 8.13 | `$HOME/.local/share/gradle/gradle-8.13` |
| Rust / cargo-ndk | 1.98.1 / 4.1.2 | Existing runner installation |
| Zig | 0.16.0 | Existing runner installation (`zig` on `PATH`) |

`ANDROID_HOME` defaults to `$HOME/.local/share/android`. The SDK's
`ndk/30.0.16248370` symlink points to the existing r30 installation, so Gradle and cargo-ndk
use the same NDK without another download. No root installation is required.

For a fresh Linux setup, obtain JDK 17 from [Adoptium](https://adoptium.net/temurin/releases/),
[Android command-line tools](https://developer.android.com/studio#command-tools), and
[Gradle 8.13](https://services.gradle.org/distributions/gradle-8.13-bin.zip). Verify the
published SHA-256 checksums before extracting. This runner's download checksums were:

```text
Temurin 17.0.20.1+1 Linux x64:
3808d1d15e3ec6bd5b84057fb5d84c33d8a1536a258146bcea2e603fc726e08e
commandlinetools-linux-15859902_latest.zip:
4e4c464f145a7512b57d088ac6c278c03c9eea610886b35a5e0804e74eedf583
gradle-8.13-bin.zip:
20f1b1176237254a6fc204d8434196fa11a4cfb387567519c61556e8710aed78
```

Install SDK packages after reading/accepting their licenses:

```sh
source scripts/env.sh
sdkmanager --sdk_root="$ANDROID_HOME" --licenses
sdkmanager --sdk_root="$ANDROID_HOME" 'platforms;android-36' 'build-tools;35.0.0'
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
```

The current command-line tools warn that `sdkmanager` is deprecated in favor of `android sdk`;
the installed `sdkmanager` commands above still work. Keep `local.properties`, credentials,
signing keys and device/host details out of version control.

## Rust and Android checks

From the repository root:

```sh
source scripts/env.sh
cargo fmt --manifest-path core/Cargo.toml --all --check
cargo test --manifest-path core/Cargo.toml --workspace --all-features --locked
cargo clippy --manifest-path core/Cargo.toml --workspace --all-targets --all-features --locked -- -D warnings
scripts/gen-licenses.sh --check
android/gradlew -p android :app:assembleDebug :app:testDebugUnitTest :app:assembleDebugAndroidTest :app:lintDebug
```

`core/or2-pair` (the Easy pair host CLI, a workspace crate that does not depend on `or2-core` at run
time) has unit tests next to the code and `tests/e2e.rs`: the whole CLI flow in a thread against the
`or2-core` client over loopback, in a temporary home and a temporary `/etc/ssh` with made-up interfaces
(nothing of the user's `~/.ssh`, sshd, tmux or herdr is read; its sshd probe asks a port nobody listens on).
The confirmation is a test double (`Auto`) that only exists in the tests; the shipped binary has no such
flag. An independent QR decoder (`rqrr`, dev-only) reads the drawn code back. `cargo build -p or2-pair
--release` builds the tool for the host (`target/release/or2-pair`); `cargo install --path core/or2-pair
--locked` installs it, and `packaging/homebrew/or2-pair.rb` builds it from source for Homebrew. The
`or2-pair-testhost` binary (feature `test-support`, so never part of an install) is the same flow with an
automatic yes, for the Kotlin end-to-end test below.

Rust integration tests (`core/or2-core/tests/`): `host.rs` runs host connections against a
disposable loopback `sshd` (trust, address racing, probe, exec caps and timeout, streamlocal (missing socket, forbidden
forwarding), shell/tmux/herdr terminals, RSA keys and key input, certificate-only hosts,
concurrency, close order, loss through a cuttable TCP relay); it uses `common/mod.rs`. The
sshd sessions get a private `TMUX_TMPDIR` (the test's tmux server lives in the fixture
directory and is killed with it), a temporary `$HOME` and a fake `herdr` script there, so no
real tmux or herdr server is ever contacted. `local.rs` runs the capability probe and tmux
commands through `LocalHost` with a restricted `PATH` (programs the probe must find are fakes
under the temporary `$HOME`, so a real herdr or mosh-server in `/usr/bin` cannot change the
result). Both sshd fixtures (this one and the Kotlin `OpenSshFixture`) pick a free port by
binding and releasing it; if another process takes it first, sshd exits with "Address already in
use" and the fixture retries on a new port (up to 5 times). Tests skip (printing `SKIP`) when `/usr/bin/sshd` or `tmux` is absent, unless
`OR2_REQUIRE_SSHD` / `OR2_REQUIRE_TMUX` is set, which fails instead: set both in CI so the
suite is never vacuously green. The herdr terminal test also skips (printing why) on a machine
with a real herdr in a standard directory, because the probe would find it whatever `$HOME`
is. The in-process russh server tests in `ssh/connection_tests.rs` (no sshd needed) cover
refused channels, terminal setup timeout, cancelled execs, the connect timer around the
host-key prompt and a dying connection task.

`core/or2-core/tests/host_mosh.rs` runs mosh terminals through a host connection to the
disposable sshd with a real `mosh-server` (bootstrap, roam, link health, host loss and disconnect,
blocked UDP, cleanup of unreached servers, and AUTO's absolute start budget: a blocked start and a
slow bootstrap both ending `TimedOut` at the budget with the server stopped (and a bootstrap that fails late, or outlasts the grace, still `TimedOut`), unconfirmed goodbyes stopped
over SSH, stops kept as host debt when no channel is free, and a server orphaned by a dead client stopped by
pid over a new connection with a non-mosh pid left alone); it needs `sshd`, `tmux` and `mosh-server`, skips
with a message otherwise, and `OR2_REQUIRE_SSHD`, `OR2_REQUIRE_TMUX` and `OR2_REQUIRE_MOSH`
make the skips failures. It finds the `mosh-server`s its sshd started by the fixture's private
`TMUX_TMPDIR` in their environment and kills exactly those, however the test ends.

`core/or2-core/tests/latency.rs` measures the critical paths over a 120 ms round trip (a real
sshd behind the relay with `Proxy::slow`, a fake `herdr` and a Unix-socket herdr server in the
test, a real `mosh-server`): connect, `capabilities()`, the inbox's first `Live` view, a focus, an
agent tap (focus then a mosh terminal, and both started together), an SSH terminal on a pane, an
unreachable host not delaying another, and a terminal on a vanished pane failing without leaving
a `mosh-server`. It prints a table with `-- --nocapture`, asserts the number of `herdr session
list` runs and `pane.focus` requests exactly, and the times with generous bounds. It needs
`sshd` and `mosh-server`.

`core/or2-core/tests/mosh_live.rs` starts a real
`mosh-server` on loopback (through `mosh::bootstrap` over `LocalHost`) and checks the
roaming, resize and disconnect interop; it needs `mosh-server`, `/bin/bash` and the `kill`
binary on `PATH`/at `/bin/bash`, and kills exactly the process ids it started, even when it
panics. **CI must install mosh and set `OR2_REQUIRE_MOSH=1`** (any value), which turns the
skip into a failure; otherwise that interop claim is never verified. It needs no network
beyond 127.0.0.1 and does not touch `~/.ssh` or any sshd.

`core/or2-core/tests/terminate.rs` runs the real stop script of `mosh::terminate` through `LocalHost`
with a restricted `PATH` (no `ps`, a failing `ps`, a fake `ps` naming a vanished pid) against
processes the test starts itself (a `sleep` symlinked as `mosh-server`, one named otherwise): a
signalled server, an unrelated process and a vanished one are `Ok`; an unusable `ps` and a failed
signal are errors. It needs only `sh`, `ps` and `sleep`.

Gradle builds the host library, generates Kotlin under `app/build/generated/uniffi/kotlin`,
and cross-builds the release Rust library into `app/build/generated/uniffi/jniLibs/arm64-v8a`.
The app has minSdk 34, compile/targetSdk 36, and no Google Play Services/FCM dependencies. Easy pair adds
CameraX 1.5.3 (`camera-core`, `camera-camera2`, `camera-lifecycle`, `camera-view`; Apache-2.0) and ZXing core 3.5.4
(Apache-2.0); the lockfile has no `gms`, `firebase` or `play-services` entries (check it with `grep -i` after
changing dependencies), and `CAMERA` is declared with `uses-feature ... required=false` so a device without a
camera can still paste a code.
The Gradle wrapper verifies its distribution checksum. `core/Cargo.lock` and
`android/app/gradle.lockfile` pin dependency graphs; Gradle locking is strict.
Only update locks intentionally, using `--write-locks` when changing dependencies.
Android runtime classpaths exclude Kotlin's legacy `kotlin-stdlib-common` metadata module:
Gradle 8.13 otherwise writes a redundant record that fails its next locked resolution.
The actual JVM `kotlin-stdlib` remains present and strictly locked.
`room-testing` (androidTest) needs kotlinx-serialization 1.8.1, and consistent resolution holds the
androidTest classpath at the debug runtime's version, so the 1.8.1 BOM is a `debugImplementation`:
debug and androidTest resolve 1.8.1, the shipped release runtime stays at 1.7.3. No runtime classpath or
dependency group is exempted from locking.

Room 2.8.3 uses KSP 2.2.21-2.0.4 with Kotlin 2.2.21; generated DAO implementations are build
outputs. The KSP argument `room.schemaLocation` writes the schema JSON for every database
version to `android/app/schemas/` (commit them: they are the migration test fixtures, and
`androidTest` packages them as assets). Changing an entity means bumping the version, adding a
`Migration` and committing the new JSON. Version 1 is the shipped M1 shape. `sqlite-jdbc`
(Apache-2.0, test-only) lets JVM tests run the migration SQL on real SQLite. Maven resolves the additional AndroidX biometric/fragment/lifecycle and coroutine
artifacts without extra system tooling. To deliberately refresh all resolvable configuration
locks after a dependency change, run `:app:dependencies --write-locks`, then the full build
command above with `--write-locks`, then again **without** `--write-locks` to verify strict
resolution. Do not exempt KSP configurations from locking.

The native JVM contract tests load the real host `.so` with desktop JNA. Holder/ViewModel tests
use fakes to exercise callbacks before handle assignment, persist-before-approve, expired
prompts, disconnect-versus-destruction, display disposal, factory cancellation, and private-array
wipe timing. `HostConnectionsTest` covers the per-host rules on fakes: one connection per host,
terminals sharing it (independent lifecycle, display lease, final frame through `Closed`),
capability probe and herdr watches (the default session is watched with no name, and every
listed session is watched whether running or not, because the probe is cached per connection;
hidden hosts are not watched), and a key array shared by several hosts being wiped only after the
last `connect_host` call. Fakes implement the app's `HostPort` (the generated `HostConnection`
returns concrete `Session`/`HerdrWatch` classes). `TerminalActivationsTest` covers the pane focus every agent-terminal entry point awaits (inbox
A to B to A reusing A's terminal after focusing A, switcher and thumbnail resume, `PaneNotFound`
and other errors not navigating, progress, cancellation) on fakes, and `TerminalActivationsProbeTest`
the same over the real FFI against `contract_probe_host`'s deterministic answers. `UnlockPlanTest` covers biometric grouping
(one prompt per distinct key record), `InboxModelTest` the inbox ordering and the flow that
assembles it, `HostRecordsTest` trust clearing on any address-list change and the clearing of the mosh failure
memory on a new transport or addresses (over a fake of the DAO's primitives), and `MigrationSqlTest`
runs the real v1 to v2, v2 to v3 and v3 to v4 SQL with foreign keys on and compares each result with a
fresh database of that version. `HostConnectionsTransportTest` covers AUTO's 5 s budget, the 24 h
failure memory and its expiry, `ServiceTest` the roaming triggers (`NetworkChanges`: transport-set and
interface changes, no reaction to bandwidth ticks, the foreground return, one debounce for all),
`ReattachTest` the auto-resume after process death, `ReconnectOfferTest` the chip and the sleeping hosts,
and `ManifestTest` the permission list. `HostConnectionsProbeTest` drives the whole holder
over the real FFI with `contract_probe_host` (host-key relay and persistence, capabilities,
agents into the inbox, terminals and frames, disconnect ordering).
`TimingTest` covers the `or2.timing` markers (connect, capabilities and first herdr view per host, the
unreachable host's `failed`); `TerminalActivationsTest` the tap, reuse and reopen paths including the
terminal that starts beside its focus and is dismissed when the pane vanished; `ReattachTest` the
battery prompt (asked once, up front; the card) and the cold-launch marker and decision.
Timing markers are read on a debug build with `adb logcat -v time -s or2.timing:D` (one tag; one line per
marker; host and pane ids only); see [contracts](contracts.md) "Timing markers".
Tests that wait on those callbacks wait for the specific thing (a frame whose row shows the echo, the
recovered link health) with a bounded timeout, never for a fixed sleep or for `frameReady.first()`,
which replays its last signal; `linkHealth` is a StateFlow that conflates, so only the lossless
listener in `HostContractTest` asserts the whole health sequence.
Easy pair (`pair/`): `PairFlowTest` (scan, review, key choice, submit, save-before-connect, failure and retry,
the wipe of the code) on fakes with the real native parser, `QrDecoderTest` (ZXing's writer into the camera
decoder: row stride, light-on-dark, rotated, noisy, 1 KB), `PairMessagesTest`, `HostRecordsTest` (the
host-with-trust transaction), and `PairEndToEndTest`: Gradle builds `or2-pair-testhost` first
(`buildPairTesthost`, passed to the tests as `or2.pair.testhost`), the test starts it on loopback in a
temporary home, pairs through the flow with the real native exchange, copies what the CLI wrote into the
fixture sshd's `AuthorizedKeysFile` (sshd reads its own file, not `~/.ssh`) and connects with the paired host
key trusted: `Connected` with no prompt. It needs `sshd` (`OR2_REQUIRE_SSHD`). `PairUiDeviceTest` renders the
sheet and the screens from fabricated state; the camera itself needs the phone.
`HostConnectionsNativeTest` (the holder over the production connector: first-use trust persisted
before approval, trusted reconnect, changed-key reject, retained closed handles, and a mosh terminal that
survives an SSH loss and a reconnect and then still answers, until an explicit disconnect or "Disconnect
all" closes it; the fixture's `dropConnections` kills its own sshd's session processes) and
`HostConnectNativeTest` (the FFI itself: address racing past a dead first address, trust,
suspend `capabilities()`/`listTmuxSessions()`, shell terminals with echo, resize, UTF-8, key
input and exit status, close ordering with terminals before the host, authentication failure) run
against a disposable loopback OpenSSH fixture (`OpenSshFixture.kt`, shared by both). They skip
when `/usr/bin/sshd` is unavailable, unless `OR2_REQUIRE_SSHD` is set, which fails instead. The
fixture's sshd sessions are hermetic: `TMUX_TMPDIR` is a private directory (no test can reach the
user's tmux server), `$HOME` is the fixture directory, and `PATH` starts with a fake `herdr` that
lists no sessions, so the holder's automatic capability probe and herdr watches (every connected
inbox host gets them) can never find or subscribe to a real herdr session. No home SSH files or
system sshd settings are read or modified. Key-operation tests use
real key exports and AES-GCM on the JVM (not Android Keystore). Device tests
load the packaged arm64 `.so` with Android JNA. Both cover the bootstrap geometry and errors,
key generation/import errors, and a `contract_probe_session` lifecycle whose listener callbacks
arrive on Rust threads (see [contracts](contracts.md)). On the JVM, `SessionContractTest` also
covers host-key prompts, frames, input echoes, resize, scroll and disconnect,
`HostContractTest` covers the host connection API against `contract_probe_host` (host-key
decision, suspend queries, terminals, herdr watch and `focusHerdrPane`, disconnect ordering, validation errors), and
`KeyContractTest` checks generated and imported keys against `ssh-keygen` using throwaway keys
in a temporary directory; it is skipped (reported as such) when `ssh-keygen` is not on `PATH`.
The runtime Rust library does not enable the host-only `bindgen` feature. russh's `aws-lc-sys`
builds for the host and arm64 with the NDK toolchain; no system CMake was needed on this runner.

The pinned libghostty-vt dependency builds the terminal engine with Zig 0.16.0 on the host and
for Android arm64. Install that Zig version on `PATH` for a fresh setup and check `zig version`
before building. The dependency's Rust build script drives Zig; no checked-in terminal binary
or Kotlin protocol implementation is used.

## Open-source licences

or2 ships other projects' code, so it ships their licences. `scripts/gen-licenses.sh` (python3 and
cargo only; no network and no extra tool) generates, from the locked dependency graphs:

| File | Content | Source |
|---|---|---|
| `android/app/src/main/assets/licenses/rust.json` | the crates linked into `libor2_ffi.so` (name, version, SPDX expression, repository, full licence and notice texts), the Zig-built libghostty-vt components (Ghostty, Highway, simdutf, uucode, the UTF-8 decoder, Zig's runtime) and the Rust standard library | `cargo tree -p or2-ffi --target aarch64-linux-android -e normal` (the features that build uses; build-script and dev dependencies ship no code and are left out) joined with `cargo metadata --locked --offline`; the texts are the `LICENSE*`/`COPYING*`/`NOTICE*` files in each crate's source in the cargo registry or checkout (for a crate that bundles C sources, such as `aws-lc-sys`, those of the bundled code too) |
| `android/app/src/main/assets/licenses/android.json` | the release runtime classpath: coordinates, SPDX licence, project URL, the licence text and any `LICENSE`/`NOTICE` at the root of the artifact | every `releaseRuntimeClasspath` line of `android/app/gradle.lockfile` (the strict lock) and each artifact's POM from the offline Gradle cache (`$GRADLE_USER_HOME` or `~/.gradle`; parent POMs for inherited licences). An artifact whose POM licence is not in the script's table, or that has no POM in the cache, stops the generator |
| `android/app/src/main/assets/licenses/notices.md` | a copy of `THIRD_PARTY_NOTICES.md` (authoritative for vendored code and components built outside Cargo and Gradle) | the file itself |
| `android/app/src/main/assets/licenses/COPYING` | a copy of `LICENSE` (or2's GPL-3.0 text, shown by About or2) | the file itself |
| `core/or2-pair/THIRD_PARTY.md` | the same for the `or2-pair` host CLI: all targets, one numbered copy of each distinct text | `cargo tree -p or2-pair --target all -e normal`; written only when that package exists in the workspace |

A crate that ships no licence file (russh, uniffi, ...) is shown with the SPDX standard text of its
declared licence from `scripts/licenses/spdx/`, flagged `fallback` with a note. The Ghostty
components are not Cargo crates: their texts live in `scripts/licenses/ghostty/` (read from the
pinned Ghostty commit) and the generator stops when `libghostty-vt-sys` starts building a different
Ghostty commit, so a bump must re-read them. Run `scripts/gen-licenses.sh` after any dependency
change and commit the result; `scripts/gen-licenses.sh --check` fails when a generated file is
stale and is part of the verification list above. The generated data is the source of the app's
Open source licenses screen. `LicenseDataTest` parses the real files and fails when a listed licence
has no GPL-3.0-compatible alternative, `MiniJsonTest` covers the JSON reader, and the device test
`AboutUiDeviceTest` opens About or2 and the list and finds a known library (compile-checked while
no phone is available).

## herdr client

`core/or2-core/src/herdr/generated.rs` is generated; do not edit it. After a herdr update run
`scripts/gen-herdr-types.sh` (needs `python3`, `rustfmt` and `cargo install cargo-typify
--version 0.10.0-alpha.1 --locked`; `--herdr PATH` picks the binary, `--offline` regenerates
from the checked-in `schema.json`, `--check` fails when the checked-in files are stale), then
review the diff of `schema.json` and the protocol note in `docs/contracts.md`.

`core/or2-core/tests/herdr_live.rs` runs the client against a real herdr: each test starts its
own `herdr --session or2-test-<pid>-<n> server` (every `HERDR_*` variable removed, so it never
reaches the session it may be run from), talks only to that socket, and stops and deletes that
session. It looks for `herdr` in `OR2_HERDR`, `PATH`, then `~/.local/bin`; without one it skips
with a message, or fails if `OR2_REQUIRE_HERDR` is set. The restart test waits for two 10 s
retry intervals (about 20 s in all).

## UI gallery (debug builds)

`UiGalleryActivity` (debug source set, like `TerminalProbeActivity`) renders every screen and key
state with fake data: no network, no biometrics, no database. Without an extra it lists the
screens; `am start -n io.github.code_akram.or2/.gallery.UiGalleryActivity --es screen <name>`
opens one directly. Names: `home`, `home-empty`, `host-cards` (unlocking, checking,
authenticating, connected with a blocked agent, failed, idle), `inbox`, `inbox-empty`,
`picker-herdr`, `picker-tmux`, `picker-recent`, `host-form`, `host-form-edit`, `keys`,
`keys-empty`, `about`, `licenses`, `hostkey-first`, `hostkey-changed`, `add-host` (the two-card sheet), `pair-scan`,
`pair-scan-denied`, `pair-review`, `pair-review-new` (with a failure), `pair-progress`, `pair-install`
(Easy pair; the camera preview itself is not in the gallery), `terminal`, `terminal-arrowpad`,
`terminal-composer` (opens with a message typed and the keyboard up, to show the caret and the
lit send button). The terminal screens run the native contract probe and replace its first
frame with a Catppuccin demo session (`gallery/DemoFrames.kt`). Use it to screenshot the phone
without touching real hosts or the biometric prompt.

Debug artifacts:
- `android/app/build/outputs/apk/debug/app-debug.apk`
- `android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk`

## Phone smoke test

Use an already-authorized ADB endpoint. For a remote server, supply the endpoint explicitly to
every command; do not start/kill the server or change SSH authorization as part of these tests.
The generic example below uses caller-supplied endpoint and serial variables, not real hosts:

```sh
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" install -r android/app/build/outputs/apk/debug/app-debug.apk
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am instrument -w io.github.code_akram.or2.test/io.github.code_akram.or2.Or2TestRunner
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am start -W -n io.github.code_akram.or2/.MainActivity
```

Run ADB only after receiving an explicit device slot. An unplugged phone is expected to be
absent; do not modify the bridge, tunnel or security settings. Compile instrumented tests on
Arch with `assembleDebugAndroidTest` while the phone is unavailable.

`PersistenceDeviceTest` uses an in-memory database: trust replacement, address-list changes
(host, port, add, remove, reorder) clearing trust while label/key/inbox edits keep it,
stale-destination rejection, and foreign-key cleanup. `MigrationDeviceTest` migrates a populated
v1 database (keys, hosts, trusted keys) through Room's `MigrationTestHelper`, validating against
`2.json`, opens it with the production database builder, and checks that the phone's SQLite is
at least 3.35 (the migration uses `DROP COLUMN`; no JVM test can check the platform's version); it
also migrates populated v2 and v1 databases to v3 (`hosts.transport`, validated against `3.json`) and
round-trips the transport through the DAO. `ConnectionServiceDeviceTest` runs the foreground service
over a scripted connection (`Or2Application.connectorOverride`, never set in production): it starts
with a host, owns its session, "Disconnect all" closes everything and the service stops, all
asserted unconditionally. The notification's contents (title, "Disconnect all", the session
count) are a separate test that runs only where `POST_NOTIFICATIONS` is already granted to the
app and is skipped with a message otherwise: the test never grants it, because OEM builds
(OxygenOS) refuse shell grants (`GRANT_RUNTIME_PERMISSIONS`); grant it in Settings once to run it.
`TransportChromeDeviceTest` covers the header badge, the "Last heard N s ago" text past five seconds
and the AUTO-fallback note. `VaultDeviceTest` creates and
deletes a disposable Keystore alias: it verifies hardware security level, per-use strong
biometric policy, non-exportability and rejection without authentication (skips if strong
biometrics are not enrolled). `EntryUiDeviceTest` displays first-use/changed-key dialogs using
fake public-key metadata without a network or production DB writes, checks the terminal screen
and its switcher against fake host connections, and that the integrated screen retains the same
terminal view and final grid through `Closed` until the session is closed. `InboxUiDeviceTest`,
`HostScreenUiDeviceTest` and `HostFormUiDeviceTest` render the inbox, host screen (connection
state, host-key prompt, the session picker sheet with herdr, tmux and Recent, "Skip" for a shell,
name validation) and the address-list host form from fabricated state; `HomeUiDeviceTest` the Home
screen (card progress and failure in place, long-press options, session thumbnails, chips, FAB)
and that a thumbnail holds the terminal's native handle until it leaves composition;
`TerminalChromeDeviceTest` the key toolbar, latched modifiers, the arrow pad with auto-repeat, the
composer's `submit_text` send and pinch-to-zoom persistence (with its own preferences file).
Every device test runs against a scratch terminal-preferences file (`Or2TestRunner`, the
instrumentation runner: a `TerminalView` reads the saved font size when it is built, so grid sizes
must not depend on the owner's pinch setting, and a pinching test must not write it); the runner
restores the real file name when the run ends. The composer-geometry test waits (a bounded `waitUntil`, 400 ms of stillness, 8 s at most) until the composer, input and send bounds stop moving before it measures, because the IME and the row are still settling right after `show` (it passed 2 of 3 runs on the phone before).
`TerminalChromeDeviceTest` also covers a slow pinch
(1.02x per event), the finger left after a pinch not scrolling, the composer keeping a message it
could not send, and the multi-line confirmation. Tests that call `show()` more than once wrap the content in a fresh `key(...)`, because
`remember`/`rememberSaveable` state survives a second `setContent` otherwise. `TerminalDeviceTest` covers IME composition, keys, selection,
resize and remount snapshots; `TerminalVisualDeviceTest` captures renderer fixtures and reports
frame timings.

Manual phone checks still required:
- Easy pair: run `or2-pair` on a host, Add host, Easy pair with QR; the camera permission dialog appears
  once, the preview reads the QR off the monitor (light and dark terminals), a pasted code works, a denied
  camera leaves the paste field; the review shows the host key fingerprint the host printed; Pair and add host
  shows "Confirm on the host" with the phone key's fingerprint; answering `y` saves the host and connects with
  no first-use prompt; `n` leaves nothing saved; `--no-listen` shows the key line to install.
- Upgrade: install the M1 build, add a key and a host, trust its key, then install the M2 build
  over it. The host, key and trusted key must all survive (the key must still unlock), and the
  host must reconnect without a new host-key prompt.
- Hosts: empty/list/add (FAB)/edit/delete (long press a card); the address list (add, remove,
  reorder, port per address, at most 8); the inbox switch; changing any address or port clears trust, changing only the
  label, key choice or inbox switch does not (a changed key or username ends a live connection).
- Keys: Ed25519 generate; system-picker import, encrypted-file passphrase retry and format errors;
  copy/share the public line; delete and reselection on hosts.
- Biometric CryptoObject encrypt/decrypt success and cancellation; missing enrollment and
  enrollment invalidation must produce clear recovery messages. Never change enrollment or
  device security settings just to test these without separate user authorization.
- Unlock grouping: with several hosts that share one key, "Connect all" in the inbox shows one
  biometric prompt and connects them all; hosts with different keys prompt once per key; a
  cancelled prompt ends the batch with a clear message and leaves no half-open connection.
- Host connection: first-use and prominent changed-key warnings (with previous fingerprints)
  on the host screen, and as a dialog naming the host when another screen is showing; closed
  and error states explain themselves; a host with several addresses connects through the first
  that answers (the host screen names the address used) and falls back when the first is
  unreachable. Use test fixtures, not real hosts, until separately authorized.
- Inbox: agents from every host flagged for it appear, blocked first, then working, done and
  idle; each row shows host, agent, workspace/tab, status and cwd; per-host status and the
  connect/retry action; a host without herdr or a running session says so; tapping a row opens
  that pane in a terminal and focuses it in herdr; the list follows agent changes live.
- Host screen: Shell, tmux sessions (attach, create by name, invalid names refused with the
  reason, a host without tmux explained, refresh) and herdr sessions (the default opens without
  a name; a stopped session cannot be opened).
- Several terminals on one connection: open a shell, a tmux session and a herdr pane, switch
  with the switcher, go Back to the inbox or host screen and return without disconnecting;
  disconnecting one session leaves the others and the connection; disconnecting the host closes
  all of them and each keeps its final frame until closed.
- Activity recreation must keep established connections and terminals; disconnect shows
  `Closed`, and "Close" releases the handle after the renderer leaves composition.
- Terminal chrome and Home: pinch zooms the font (remembered after a restart; the default gives
  about 55 columns), drag down on the handle or the minimise button returns to Home with the
  session still running and its live thumbnail under SESSIONS (tap resumes), the arrow pad keys
  repeat while held, a composer message is submitted, not left as a pasted newline (try it on a blocked agent),
  haptics on modifier latch, send and host-key approval. Compare each `UiGalleryActivity` screen
  with the Moshi references named in [the UI system](ui.md).
- Integrated terminal IME show/hide geometry, committed/composing text, keys row, selection,
  scrolling, recreation and final-frame retention; collect apply/draw and Window frame timings.
- Capture representative screenshots and inspect them; visual verification and successful
  biometric round trips remain **pending phone**, not proved by compilation or JVM tests.

The vault accepts only StrongBox or TEE AES-256-GCM keys, prefers StrongBox when available,
requires BIOMETRIC_STRONG per operation and invalidates on new enrollment. No software or
device-credential fallback is allowed. Invalidated records remain for explanation/deletion;
re-import or generate a new SSH key to recover. Private keys are never exported or backed up:
`allowBackup=false` and cloud/device-transfer extraction rules exclude all app data.
Connection and terminal ownership are application-scoped (`HostConnections`, held by
`Or2Application`); `ConnectionService`, a foreground service, keeps the process alive while anything
is open (it does not own the connections); process death ends them. There is at most one connection per host and any number of
terminals per connection. A disconnect leaves a terminal's handle and final frame readable under
`Closed` until "Close" retires it; reconnecting a closed host replaces its connection object but
leaves its terminals alone (a lost connection whose mosh terminals still run is kept, owned, until the last
one closes, because releasing the native object would close them; see [contracts](contracts.md)). A terminal-screen display lease delays native `close()` until the
old screen leaves composition, then yields a main-loop turn for terminal disposal. Activity
recreation and navigation alone neither disconnect nor retire anything. MainActivity uses
`adjustResize`; the root adds IME padding everywhere except the terminal destination, where
TerminalScreen owns IME insets.
This does not prove SSH/IME/terminal acceptance or the broader v0 background-session test.
