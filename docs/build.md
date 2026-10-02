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
cargo run --quiet --locked --manifest-path core/Cargo.toml -p xtask -- gen-herdr-types --offline --check
cargo run --quiet --locked --manifest-path core/Cargo.toml -p xtask -- gen-licenses --check
android/gradlew -p android :app:assembleDebug :app:testDebugUnitTest :app:assembleDeviceTest :app:assembleDeviceTestAndroidTest :app:lintDebug :app:lintDeviceTest :app:assembleRelease
```

`:app:assembleRelease` signs with the release key when the machine has a signing file (see "Release signing"
below). A check that must not use the key (an agent's or a reviewer's gate run) sets `OR2_SIGNING_PROPERTIES` to
a path that does not exist, which builds the unsigned release (a `-Duser.home` in `org.gradle.jvmargs` was seen
not to hide `~/.config/or2` from the build).

Repository tooling is the `core/xtask` crate (Rust; no scripts in other languages). Inside `core/` the
cargo alias in `core/.cargo/config.toml` makes it `cargo xtask <task>`; from the repository root run the
same task as `cargo run --quiet --locked --manifest-path core/Cargo.toml -p xtask -- <task>` (the form
used above). The generators are `gen-herdr-types` and `gen-licenses`; each regenerates checked-in files and
has a `--check` mode that writes nothing and fails when they are stale. `dist` builds the `or2-pair` release
binaries (below; it generates nothing that is checked in). `xtask` is a workspace member
but is not linked into the app library, so it never appears in the licence data. Its unit tests run
with the rest of the workspace, and so does its integration test `core/xtask/tests/install_or2_pair.rs`,
which runs `scripts/install-or2-pair.sh` (see "or2-pair release binaries" below).

`core/or2-pair` (the Easy pair host CLI, a workspace crate that does not depend on `or2-core` at run
time) has unit tests next to the code and seven integration suites. `tests/code_agreement.rs`: the host's
reading of a typed code against the phone's (`or2_core::pair::PairCode`, a dev-dependency) on the same
inputs, the contract's vectors and their bootstrap keys included. `tests/fifo.rs`: the built binary with a
FIFO where `authorized_keys` should be (reports and exits, nothing changed). `tests/flow.rs`: the whole CLI flow in
a thread, in a temporary home and a temporary `/etc/ssh` with made-up interfaces, a scripted terminal and a
pretend sshd banner (nothing of the user's `~/.ssh`, sshd, tmux or herdr is read), the phone played by a
direct call of the forced command's code. `tests/cli.rs`: the built binary (usage, the removed `--bind` and
`--pair-port`, `enroll`, the cleanup on SIGINT, SIGTERM, SIGHUP and the timeout, and the output rail: the error that
ends it on standard error, the ASCII rail outside a UTF-8 locale and with `--ascii`, and, on a pseudo-terminal,
colour (none with `NO_COLOR`, `--no-color` or `TERM=dumb`) and the answered code question redrawn with the code
masked). `tests/manual_keys.rs`:
the no-key-installation (Windows) behaviour. `tests/process.rs`: what needs a process of its own, its other half
run in a child (this test binary again): the code prompt on a pseudo-terminal through Ctrl-Z and `fg`, an ending
signal in the prompt's teardown (a `test-support` hook), Ctrl-C during a slow login-shell check (no process of the
shell's group left running; Linux, read from `/proc`), and the lock file under umask 0777. `tests/sshd.rs`: the end to end suite against a **disposable
`sshd`** (its own host key, config and `authorized_keys`, a loopback port, run as the current user): the built
`or2-pair-testhost` pairs with a test-only russh phone (derives the bootstrap key, pins the host key,
authenticates, runs the exchange through the forced command, then logs in with the key it handed over), plus
a different code, a run that ended, a host key mismatch, a second phone (`gone`), rc-file noise, a
`ForceCommand`, `sshd -T` against the checks' reading of `sshd_config` values, `expiry-time` in UTC and a host in
another time zone than sshd. The `real_phone` module of the same file repeats the pairing with the
product's own client (`or2_core::pair`, a dev-dependency of `or2-pair`): its strict parser reads the code the
host printed and `pair_enroll` does the connection (success, a different code, a host key mismatch, a run
that ended). Like the other sshd tests it skips (printing `SKIP`) without
`/usr/bin/sshd` and `ssh-keygen`, unless `OR2_REQUIRE_SSHD` is set, which fails instead; set it in the full
gate (`cargo test -p or2-pair --all-features`). The code is typed through a `CodePrompt` the tests
script; the shipped binary reads a terminal only. An independent QR decoder (`rqrr`, dev-only) reads the
drawn code back, with the rail around it (`tests/flow.rs`) and in every style (`--ascii`, `--invert`, colour;
`qr.rs`). The rail itself (`rail.rs`) is unit-tested: both glyph sets, colour on and off (16-colour SGR only),
wrapping with the rail continued and commands never split, the locale and `NO_COLOR` rules, and the redraw of an
answered question. `cargo build -p or2-pair --release` builds the tool for the host
(`target/release/or2-pair`); `cargo install --path core/or2-pair --locked` installs it, and
`packaging/homebrew/or2-pair.rb` builds it from source for Homebrew. The `or2-pair-testhost` binary (feature
`test-support`, so never part of an install) reads the code from standard input, for the tests above and the
Kotlin end-to-end test below. `cargo clippy -p or2-pair --lib --bins --all-features --target
x86_64-pc-windows-gnu` cross-checks the non-Unix build (the dev-dependency `russh` needs a C toolchain for
that target, so the tests are not cross-checked).

The checks' fixes for a host (`hints.rs`: the package manager, the sshd unit, the service manager and the
firewall, read from files under a root directory) are unit-tested on fake trees in a temporary directory and on
fabricated facts, never on the machine running the tests. The macOS firewall's `socketfilterfw` answers are fed
from captured outputs through the `Commands` seam; no test runs the real command.

**or2-pair release binaries.** From the repository root:

```sh
cargo run --quiet --locked --manifest-path core/Cargo.toml -p xtask -- dist
```

builds `core/target/dist/or2-pair-<target>` and `core/target/dist/SHA256SUMS` for the targets this host can
build: on Linux `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (static, linked with the toolchain's
own `rust-lld`, so no C cross toolchain or musl install is needed; `rustup target add` both first), on macOS
`x86_64-apple-darwin` and `aarch64-apple-darwin` (Apple's linker and SDK). `--target` picks some, `--out` another
directory, `--expect-version` fails unless the workspace version is that one. Symbols are stripped, the
checkout's and cargo home's paths are mapped to `/or2` and `/cargo`, and the binary for the builder's own
OS and CPU is run once (`--version`). macOS targets are refused on Linux: linking Mach-O here would need the
SDK, or zig's stand-ins for it, which lack `libiconv` (tried with zig 0.16 as the linker: it fails there), do
not record the SDK version, and give a binary that cannot be run here to check it. The Arch runner builds
both Linux targets (`file`: x86-64 static-pie and aarch64 statically linked, both stripped); the aarch64 one is
not run here (no qemu).

`--expect-version X.Y.Z` also fails unless the app's `versionName` (`android/app/build.gradle.kts`) is `X.Y.Z`,
and the xtask unit test `the_workspace_version_and_the_apps_version_name_agree` keeps the two equal on every
test run (one release stream; see "Releases" below).

`scripts/install-or2-pair.sh` (POSIX `sh`, clean under ShellCheck 0.11; `AGENTS.md` allows user-facing install
scripts in `sh` when they are tested from xtask) installs a release (see [Pair a host](pairing.md) and
[contracts](contracts.md#host-cli), "Distribution"). `core/xtask/tests/install_or2_pair.rs` runs it with `sh`
against `file://` fixtures in a temporary directory laid out like the GitHub release tree
(`OR2_PAIR_RELEASES_BASE` pointing at `releases/` with `latest/download/` and `download/vX.Y.Z/`; a fake `uname`
and a fake `id` first on `PATH`; `HOME` and `TMPDIR` in the fixture): every OS and CPU spelling, the latest and a
named release in each `--version` spelling, a version that does not exist, strings that are not versions, a
binary that is not the version it is filed under, no release at all, `--dir`, `OR2_PAIR_INSTALL_DIR`, the `PATH`
hint, a checksum mismatch, a missing `SHA256SUMS` line and a missing asset (nothing installed), an unsupported CPU
or OS and a non-HTTPS base (refused before any download), the script cut after every one of its lines and piped
into `sh` (nothing installed, no staging file, nothing left in `TMPDIR`), links planted at staging-like names and
at `or2-pair` (never followed; the victim file unchanged), a checked binary that fails `--version` (the old
binary stays), a directory at the destination, a directory anyone can write, and a fake root with a directory
that is not root's (the notice, then refused); and the output: the whole rail of an install, a refusal on standard
error ending it, a usage error before it as one plain line, the ASCII rail outside a UTF-8 locale, and colour only
on a pseudo-terminal (none with `NO_COLOR` or `TERM=dumb`; Linux, through `libc`, a Unix dev-dependency of
`xtask`). It needs `curl` (`wget` cannot read `file://`) and skips,
printing `SKIP`, without it.

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

`core/or2-core/tests/host_nav.rs` runs `HostHandle::navigate` (the swipe gestures' moves) for a
tmux target end to end: the disposable sshd, tmux on its private `TMUX_TMPDIR`, a real tmux client
attached by an SSH terminal, and window, pane and session moves checked with `tmux display` and
`list-clients` (an `or2-a2` session next to `or2-a` proves the targets are exact); a second test
attaches two SSH terminals to one session and checks that each gesture moves only the client its
terminal's attach recorded (`@or2-client-<id>`), and that a closed terminal releases it. It needs `sshd`
and `tmux`, with the same skips and `OR2_REQUIRE_*` switches as `host.rs`. The herdr moves run
against a real isolated herdr in `herdr_live.rs` (below) and a scripted one in `herdr::navigate`'s
unit tests.

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
That build remaps the repository to `/or2` and the cargo home to `/cargo` (`CARGO_ENCODED_RUSTFLAGS`, as
`xtask dist` does), so no build-machine path is left in `libor2_ffi.so`; the task fails if either path is
still in the library. The variable replaces any `RUSTFLAGS` or configured `rustflags` for that build.
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
androidTest classpath at the tested runtime's version, so the 1.8.1 BOM is a `debugImplementation` and a
`deviceTestImplementation` (the `deviceTest` build type, see "Device tests"): debug, deviceTest and androidTest
resolve 1.8.1, the shipped release runtime stays at 1.7.3. No runtime classpath or
dependency group is exempted from locking. The lockfile has no `debugAndroidTest*` configurations: the
instrumented tests belong to `deviceTest` (`deviceTestAndroidTest*`).

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
fresh database of that version. `HostConnectionsTransportTest` covers the instant opens (the AUTO choice table,
the per-connection UDP verdict and its reset, the background mosh attempt and the swap, one attempt
per terminal and one in flight per host, cancellation, the shell's budget, the transport choice's wait
for `mosh_server()` only on a tap that connected the host), `ServiceTest` the roaming triggers (`NetworkChanges`: transport-set and
interface changes, no reaction to bandwidth ticks, the foreground return, one debounce for all),
`ReattachTest` the auto-resume after process death, `ReconnectOfferTest` the chip and the sleeping hosts,
and `ManifestTest` the permission list. `HostConnectionsProbeTest` drives the whole holder
over the real FFI with `contract_probe_host` (host-key relay and persistence, capabilities,
agents into the inbox, terminals and frames, disconnect ordering).
`TimingTest` covers the `or2.timing` markers (connect, capabilities and first herdr view per host, the
unreachable host's `failed`); `TerminalActivationsTest` the tap, reuse and reopen paths including the
terminal that starts beside its focus and is dismissed when the pane vanished; `OneTimePromptsTest` the
battery step (the last step of adding a host, once; the card) and the notification offer, `AddHostEndTest`
how adding a host ends on that step, and `ReattachTest` the cold-launch marker and decision.
`AgentAlertsTest` (`notify/`) covers agent notifications: the edge rule (one per sequence number, none on a
watch's first snapshot or for a pane first seen later, none for the pane on screen), the cancel rules, the
switch, the tap's token and connect-first decision, and the holder feeding it from live watches; the device
test `AgentNotificationsDeviceTest` checks the `agents` channel and a posted notification (the latter only
where the permission is already granted).
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

## Releases

One release stream for the whole project, tagged `vX.Y.Z`: the tag, the Cargo workspace version
(`core/Cargo.toml`) and the app's `versionName` are the same `X.Y.Z`. A release contains the app's APK and the
`or2-pair` binaries; its notes are `docs/releases/vX.Y.Z.md` and it gets a [changelog](../CHANGELOG.md) entry.
A bump changes the workspace version (then `cargo update --workspace --offline` for the lock), the app's
`versionName` and `versionCode` (one more each release), and the version `NativeContractTest` expects from the
library.

`.github/workflows/release.yml` runs `dist` on an Ubuntu and a macOS runner (Rust 1.98.1), with
`--expect-version X.Y.Z` on a tag, and checks each binary's `file` output (Linux: a static or static-pie,
stripped ELF of its architecture; macOS: a Mach-O executable of its architecture), failing on a mismatch. On a
pushed `v*` tag (`github.event_name == 'push'` and the tag ref: a manual run never publishes, even on a tag) the
Linux job also checks that `docs/releases/<tag>.md` exists and hands it on as an artifact, and a third job joins
the two `SHA256SUMS`, checks every binary against it (`sha256sum --check --strict`, all four targets present,
exactly five files) and runs `gh release create <tag> --verify-tag --notes-file <the notes>` with the five files.
That job has `contents: write` and checks nothing out; the build jobs read only. Actions are pinned to commit
SHAs. `workflow_dispatch` builds the binaries as workflow artifacts and publishes nothing. ShellCheck 0.11 and
actionlint 1.7 pass on the script and the workflow.

The APK is **not** built in CI: the release signing key stays on the maintainer's machine. It is built and signed
locally (next section) and added to the release afterwards with `gh release upload <tag> or2-<tag>.apk`; its
SHA-256 goes into the notes.

### Release signing

`android/app/build.gradle.kts` signs the `release` build type when it finds a properties file: the path in
`OR2_SIGNING_PROPERTIES`, else `$XDG_CONFIG_HOME/or2/signing.properties` (normally
`~/.config/or2/signing.properties`). It holds four keys:

```properties
storeFile=or2-release.jks
storePassword=<the keystore password>
keyAlias=or2
keyPassword=<the key password>
```

`storeFile` is absolute or relative to the properties file's directory. A keystore that JDK 17's `keytool`
makes is PKCS12, whose key has the store's password: `keyPassword` is then the same as `storePassword` (a
different one fails at `packageRelease` with "Get Key failed"). A missing key or keystore stops the build with the
file's name and the key's name, never a value. Without the file the release build is unsigned
(`app-release-unsigned.apk`), exactly as F-Droid builds it from source; with it, `app-release.apk`. To make the
key once (a 4096-bit RSA key valid for about 27 years; keep the files mode 600):

```sh
mkdir -p ~/.config/or2 && chmod 700 ~/.config/or2
keytool -genkeypair -keystore ~/.config/or2/or2-release.jks -alias or2 -keyalg RSA -keysize 4096 -validity 10000
```

**Back up the keystore and both passwords, offline and somewhere other than this machine.** Android installs an
update only over an APK signed with the same key: with the key lost, no later or2 release can update an installed
one, and every user would have to uninstall (losing their hosts and Keystore-bound SSH keys) to move on. No key,
password or properties file is ever committed, and the build never prints them. Check a build with
`apksigner verify --print-certs android/app/build/outputs/apk/release/app-release.apk` (build tools 35.0.0); an
unsigned one fails `apksigner verify`. The release APK is signed by a different key than debug builds, so it
cannot be installed over a debug build of `io.github.code_akram.or2`.

## Open-source licences

or2 ships other projects' code, so it ships their licences. `cargo xtask gen-licenses` (Rust: cargo
only, no network and no extra tool) generates, from the locked dependency graphs:

| File | Content | Source |
|---|---|---|
| `android/app/src/main/assets/licenses/rust.json` | the crates linked into `libor2_ffi.so` (name, version, SPDX expression, repository, full licence and notice texts), the Zig-built libghostty-vt components (Ghostty, Highway, simdutf, uucode, the UTF-8 decoder, Zig's runtime) and the Rust standard library | `cargo tree -p or2-ffi --target aarch64-linux-android -e normal` (the features that build uses; build-script and dev dependencies ship no code and are left out) joined with `cargo metadata --locked --offline`; the texts are the `LICENSE*`/`COPYING*`/`NOTICE*` files in each crate's source in the cargo registry or checkout (for a crate that bundles C sources, such as `aws-lc-sys`, those of the bundled code too) |
| `android/app/src/main/assets/licenses/android.json` | the release runtime classpath: coordinates, SPDX licence, project URL, the licence text and any `LICENSE`/`NOTICE` at the root of the artifact | every `releaseRuntimeClasspath` line of `android/app/gradle.lockfile` (the strict lock) and each artifact's POM from the offline Gradle cache (`$GRADLE_USER_HOME` or `~/.gradle`; parent POMs for inherited licences). An artifact whose POM licence is not in the generator's table, or that has no POM in the cache, stops the generator |
| `android/app/src/main/assets/licenses/notices.md` | a copy of `THIRD_PARTY_NOTICES.md` (authoritative for vendored code and components built outside Cargo and Gradle) | the file itself |
| `android/app/src/main/assets/licenses/COPYING` | a copy of `LICENSE` (or2's GPL-3.0 text, shown by About or2) | the file itself |
| `core/or2-pair/THIRD_PARTY.md` | the same for the `or2-pair` host CLI: all targets, one numbered copy of each distinct text | `cargo tree -p or2-pair --target all -e normal` (the crates it links, with their texts) |

A Maven artifact whose POM lists a second licence for code it bundles (camera-core and libyuv) is shown
with `AND` and that project's own text, kept in `core/xtask/licenses/libyuv/`; a licence name the generator
does not know still stops it.

A crate that ships no licence file (russh, uniffi, ...) is shown with the SPDX standard text of its
declared licence from `core/xtask/licenses/spdx/`, flagged `fallback` with a note. The Ghostty
components are not Cargo crates: their texts live in `core/xtask/licenses/ghostty/` (read from the
pinned Ghostty commit) and the generator stops when `libghostty-vt-sys` starts building a different
Ghostty commit, so a bump must re-read them. Run `cargo xtask gen-licenses` after any dependency
change and commit the result; `cargo xtask gen-licenses --check` fails when a generated file is
stale and is part of the verification list above. The generated data is the source of the app's
Open source licenses screen. `LicenseDataTest` parses the real files and fails when a listed licence
has no GPL-3.0-compatible alternative, `MiniJsonTest` covers the JSON reader, and the device test
`AboutUiDeviceTest` opens About or2 and the list and finds a known library (compile-checked while
no phone is available).

## herdr client

`core/or2-core/src/herdr/generated.rs` is generated; do not edit it. After a herdr update run
`cargo xtask gen-herdr-types` (needs `rustfmt` and `cargo install cargo-typify
--version 0.10.0-alpha.1 --locked`; `--herdr PATH` picks the binary, `--offline` regenerates
from the checked-in `schema.json`, `--check` fails when the checked-in files are stale), then
review the diff of `schema.json` and the protocol note in `docs/contracts.md`. The normalization
lives in `core/xtask/src/herdr.rs` and has unit tests.

`core/or2-core/tests/herdr_live.rs` runs the client against a real herdr: each test starts its
own `herdr --session or2-test-<pid>-<n> server` (every `HERDR_*` variable removed, so it never
reaches the session it may be run from), talks only to that socket, and stops and deletes that
session. It looks for `herdr` in `OR2_HERDR`, `PATH`, then `~/.local/bin`; without one it skips
with a message, or fails if `OR2_REQUIRE_HERDR` is set. The restart test waits for two 10 s
retry intervals (about 20 s in all). The navigation test builds two workspaces (one with two
panes side by side and two tabs) and checks every tab, pane and workspace move, wrapping and a
vanished pane against the session's own snapshot.

## UI gallery (debug builds)

`UiGalleryActivity` (debug source set, like `TerminalProbeActivity`) renders every screen and key
state with fake data: no network, no biometrics, no database. Without an extra it lists the
screens; `am start -n io.github.code_akram.or2/.gallery.UiGalleryActivity --es screen <name>`
opens one directly. Names: `home`, `home-empty`, `home-notices` (the battery and notification cards), `host-cards` (unlocking, checking,
authenticating, connected with a blocked agent, failed, idle), `inbox`, `inbox-empty`,
`picker-herdr`, `picker-tmux`, `picker-recent`, `host-form`, `host-form-new-key` (no stored key: **New key** chosen), `host-form-edit`, `keys`,
`keys-empty`, `about`, `licenses`, `hostkey-first`, `hostkey-changed`, `add-host` (the add-host chooser in its sheet; `home-empty` shows it inline), `pair-scan`,
`pair-scan-denied`, `pair-review`, `pair-review-new` (with a failure), `pair-progress`, `pair-install`
(Easy pair; the camera preview itself is not in the gallery), `keepalive` (the battery step that ends adding a
host), `keepalive-waiting` (Android's dialog up), `terminal` (a shell over SSH), `terminal-tmux` (a tmux
target over Mosh), `terminal-long` (a title long enough to ellipsize), `terminal-stale` (Mosh, `Last heard
12 s ago`), `terminal-connecting` and `terminal-closed` (the notice strip under the header, the latter with
**Close**), `terminal-arrowpad`, `terminal-composer` (opens with a message typed and the keyboard up, to show
the caret and the lit send button). The terminal screens run the native contract probe and replace its first
frame with a Catppuccin demo session (`gallery/DemoFrames.kt`). Use it to screenshot the phone
without touching real hosts or the biometric prompt.

In the device-test app the same gallery is
`am start -n io.github.code_akram.or2.devicetest/io.github.code_akram.or2.gallery.UiGalleryActivity`
(the class name in full: the `/.gallery...` shorthand resolves against the application id).

Debug artifacts:
- `android/app/build/outputs/apk/debug/app-debug.apk`: the daily app, `io.github.code_akram.or2`
- `android/app/build/outputs/apk/deviceTest/app-deviceTest.apk`: the app under test,
  `io.github.code_akram.or2.devicetest` (label "or2 devicetest")
- `android/app/build/outputs/apk/androidTest/deviceTest/app-deviceTest-androidTest.apk`: the tests,
  `io.github.code_akram.or2.devicetest.test`, instrumenting `io.github.code_akram.or2.devicetest`

## Device tests

The debug build is the owner's daily app: `io.github.code_akram.or2`, with their hosts in its
database and their SSH keys in its Android Keystore entries. A connected test run installs over the
app under test and uninstalls it when it ends, which deletes both. So the instrumented tests never
target debug. They belong to the `deviceTest` build type (`testBuildType = "deviceTest"`), which is
debug in every respect (`initWith(debug)`: debug signing, debuggable, the `src/debug` sources such as
`TerminalProbeActivity` and `UiGalleryActivity`, the debug-only dependencies) except its application
id, `io.github.code_akram.or2.devicetest`. It has its own data directory, Keystore namespace,
permissions, notification channel and battery-optimization entry, and its `${applicationId}`
authorities and permissions (`androidx-startup`, `DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`) do not
clash with the daily app's, so both are installed side by side. The debug variant has no androidTest:
`connectedDebugAndroidTest` and `assembleDebugAndroidTest` do not exist, so no Gradle task can install
a test APK against the daily app.

The device suite (the device chosen by `ANDROID_SERIAL` when several are attached):

```sh
android/gradlew -p android :app:connectedDeviceTestAndroidTest
```

It installs `io.github.code_akram.or2.devicetest` and `io.github.code_akram.or2.devicetest.test`, runs
`Or2TestRunner`, and uninstalls both; `io.github.code_akram.or2` is never installed, cleared or
uninstalled. Reports land under `android/app/build/reports/androidTests/connected/`.
Compile the suite without a phone with `:app:assembleDeviceTestAndroidTest`.

The uninstall at the end also deletes the screenshots the suite writes into the app under test's
`files/` (`terminal-review/` from `TerminalVisualDeviceTest`, with `timings.txt`, and `pair-review/` from
`PairUiDeviceTest`). To keep them, leave the APKs installed and read them with `run-as`:

```sh
android/gradlew -p android :app:connectedDeviceTestAndroidTest -Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true
adb shell run-as io.github.code_akram.or2.devicetest ls files/terminal-review files/pair-review
adb exec-out run-as io.github.code_akram.or2.devicetest cat files/terminal-review/<name>.png > <name>.png
adb uninstall io.github.code_akram.or2.devicetest.test
adb uninstall io.github.code_akram.or2.devicetest
```

`ConnectionServiceDeviceTest`'s notification test needs `POST_NOTIFICATIONS` granted in Settings to "or2
devicetest", which a fresh install does not have; run it with the APKs left installed (or installed by
hand as below) after granting it once.

Never point a test command at `io.github.code_akram.or2`: no `pm clear`, `adb uninstall` or
`run-as` writes against it, and no instrumentation targeting it.

## Phone smoke test

Use an already-authorized ADB endpoint. For a remote server, supply the endpoint explicitly to
every command; do not start/kill the server or change SSH authorization as part of these tests.
The generic example below uses caller-supplied endpoint and serial variables, not real hosts. It runs
the suite by hand against the device-test app, then updates the daily app in place (`install -r` keeps
its data) and starts it:

```sh
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" install -r android/app/build/outputs/apk/deviceTest/app-deviceTest.apk
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" install -r android/app/build/outputs/apk/androidTest/deviceTest/app-deviceTest-androidTest.apk
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am instrument -w io.github.code_akram.or2.devicetest.test/io.github.code_akram.or2.Or2TestRunner
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" install -r android/app/build/outputs/apk/debug/app-debug.apk
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am start -W -n io.github.code_akram.or2/.MainActivity
```

Run ADB only after receiving an explicit device slot. An unplugged phone is expected to be
absent; do not modify the bridge, tunnel or security settings. Compile instrumented tests on
Arch with `assembleDeviceTestAndroidTest` while the phone is unavailable.

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
Its counterpart, `theServiceStartsAndKeepsRunningWithoutTheNotificationPermission`, runs only where the
permission is *not* granted (the usual case for "or2 devicetest", since nothing asks for it on connect) and
checks the service starts and keeps holding the connection without it.
`TransportChromeDeviceTest` covers the header badge, the "Last heard N s ago" text past five seconds
and the AUTO-fallback note; `TerminalHeaderDeviceTest` the header's composition (the title centred on the
card and clear of the discs, the stale label and the pill, also when long; the handle over it), the notice
strip taking layout space (Connecting, Closed with **Close**) and drag-down to minimise. `VaultDeviceTest` creates and
deletes a disposable Keystore alias: it verifies hardware security level, per-use strong
biometric policy, non-exportability and rejection without authentication (skips if strong
biometrics are not enrolled). `EntryUiDeviceTest` displays first-use/changed-key dialogs using
fake public-key metadata without a network or production DB writes, checks the terminal screen
and its switcher against fake host connections, and that the integrated screen retains the same
terminal view and final grid through `Closed` until the session is closed. `InboxUiDeviceTest`,
`HostScreenUiDeviceTest` and `HostFormUiDeviceTest` render the inbox, host screen (connection
state, host-key prompt, the session picker sheet with herdr, tmux and Recent, "Shell" for a plain shell,
name validation) and the address-list host form (with **New key**) from fabricated state; `HomeUiDeviceTest` the Home
screen (card progress and failure in place, long-press options, session thumbnails, chips, FAB, the empty
state's add-host chooser matching the sheet's)
and that a thumbnail holds the terminal's native handle until it leaves composition;
`TerminalChromeDeviceTest` the key toolbar, latched modifiers, the arrow pad with auto-repeat (floating with no
backing, closed by the toolbar's key), the
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
- Easy pair: Add host, Easy pair shows the code `K`; run `or2-pair` on a host and type it; the camera permission
  dialog appears once, the preview reads the QR off the monitor (light and dark terminals), a pasted code
  works, a denied camera leaves the paste field; the review shows the host key fingerprint the host printed;
  Pair saves the host and connects with no first-use prompt, and `or2-pair` prints the pairing; a mistyped code
  on the host is asked again; Ctrl-C on the host removes its temporary key (the phone then says the host
  stopped); `--manual` shows the key line to install. Over a host's public address with only port 22 open.
- First run (fresh install, no key): the empty Home shows the same two cards as **+** and no key step; **Set up
  manually** has **New key** chosen; **Save** asks for the biometric once, then shows the key line to install;
  cancelling the prompt saves nothing and says so.
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
