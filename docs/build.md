# M1 Android app: build and verify

The Compose app loads `or2-ffi` through generated UniFFI Kotlin/JNA bindings. Host settings,
encrypted key records and trusted host keys live in Room. `or2-core` remains free of Android,
UniFFI and persistence dependencies. The production connector calls the real `connect`
export (the M1 single-session path, kept until lane B lands; FFI API 4's `connect_host` is real
since lane A1 but not yet used by the app); the contract probes are used only by tests. The session screen embeds the Canvas terminal with IME and keys-row input,
keeping the final displayed frame visible through `Closed`.

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
android/gradlew -p android :app:assembleDebug :app:testDebugUnitTest :app:assembleDebugAndroidTest :app:lintDebug
```

Rust integration tests (`core/or2-core/tests/`): `host.rs` runs host connections against a
disposable loopback `sshd` (trust, address racing, probe, exec caps and timeout, streamlocal,
shell/tmux/herdr terminals, concurrency, close order, loss through a cuttable TCP relay);
`openssh.rs` does the same for the M1 single-session path; both share `common/mod.rs`. The
sshd sessions get a private `TMUX_TMPDIR` (the test's tmux server lives in the fixture
directory and is killed with it), a temporary `$HOME` and a fake `herdr` script there, so no
real tmux or herdr server is ever contacted. `local.rs` runs the capability probe and tmux
commands through `LocalHost` with a restricted `PATH`. Tests skip (printing `SKIP`) when
`/usr/bin/sshd` or `tmux` is absent.

Gradle builds the host library, generates Kotlin under `app/build/generated/uniffi/kotlin`,
and cross-builds the release Rust library into `app/build/generated/uniffi/jniLibs/arm64-v8a`.
The app has minSdk 34, compile/targetSdk 36, and no Google Play Services/FCM dependencies.
The Gradle wrapper verifies its distribution checksum. `core/Cargo.lock` and
`android/app/gradle.lockfile` pin dependency graphs; Gradle locking is strict.
Only update locks intentionally, using `--write-locks` when changing dependencies.
Android runtime classpaths exclude Kotlin's legacy `kotlin-stdlib-common` metadata module:
Gradle 8.13 otherwise writes a redundant record that fails its next locked resolution.
The actual JVM `kotlin-stdlib` remains present and strictly locked. No runtime classpath or
dependency group is exempted from locking.

Room 2.8.3 uses KSP 2.2.21-2.0.4 with Kotlin 2.2.21; generated DAO implementations are build
outputs. Maven resolves the additional AndroidX biometric/fragment/lifecycle and coroutine
artifacts without extra system tooling. To deliberately refresh all resolvable configuration
locks after a dependency change, run `:app:dependencies --write-locks`, then the full build
command above with `--write-locks`, then again **without** `--write-locks` to verify strict
resolution. Do not exempt KSP configurations from locking.

The native JVM contract tests load the real host `.so` with desktop JNA. Holder/ViewModel tests
use fakes to exercise callbacks before handle assignment, persist-before-approve, expired
prompts, disconnect-versus-destruction, display disposal, factory cancellation, and private-array
wipe timing. `SessionHolderNativeTest` uses the real production connector and a disposable
loopback OpenSSH fixture for first-use trust, trusted reconnect, changed-key rejection and
retained closed handles; it skips when `/usr/bin/sshd` is unavailable. No home SSH files or
system sshd settings are read or modified. `HostConnectNativeTest` drives the production
`connectHost` through the same fixture: address racing past a dead first address, host-key
approval, suspend `capabilities()`/`listTmuxSessions()`, two shell terminals on one connection
(echo, resize, exit status), trusted reconnect, changed-key reject, authentication failure
and close ordering (terminals before the host). The fixture gives its sshd sessions a private
`TMUX_TMPDIR`, so nothing in it can reach the user's tmux server. Key-operation tests use
real key exports and AES-GCM on the JVM (not Android Keystore). Device tests
load the packaged arm64 `.so` with Android JNA. Both cover the bootstrap geometry and errors,
key generation/import errors, and a `contract_probe_session` lifecycle whose listener callbacks
arrive on Rust threads (see [contracts](contracts.md)). On the JVM, `SessionContractTest` also
covers host-key prompts, frames, input echoes, resize, scroll and disconnect,
`HostContractTest` covers the host connection API against `contract_probe_host` (host-key
decision, suspend queries, terminals, herdr watch, disconnect ordering, validation errors), and
`KeyContractTest` checks generated and imported keys against `ssh-keygen` using throwaway keys
in a temporary directory; it is skipped (reported as such) when `ssh-keygen` is not on `PATH`.
The runtime Rust library does not enable the host-only `bindgen` feature. russh's `aws-lc-sys`
builds for the host and arm64 with the NDK toolchain; no system CMake was needed on this runner.

The pinned libghostty-vt dependency builds the terminal engine with Zig 0.16.0 on the host and
for Android arm64. Install that Zig version on `PATH` for a fresh setup and check `zig version`
before building. The dependency's Rust build script drives Zig; no checked-in terminal binary
or Kotlin protocol implementation is used.

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
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am instrument -w io.github.code_akram.or2.test/androidx.test.runner.AndroidJUnitRunner
adb -H "$ADB_HOST" -P "$ADB_PORT" -s "$ANDROID_SERIAL" shell am start -W -n io.github.code_akram.or2/.MainActivity
```

Run ADB only after receiving an explicit device slot. An unplugged phone is expected to be
absent; do not modify the bridge, tunnel or security settings. Compile instrumented tests on
Arch with `assembleDebugAndroidTest` while the phone is unavailable.

`PersistenceDeviceTest` uses an in-memory database: trust replacement, endpoint-change trust
clearing, stale-destination rejection, and foreign-key cleanup. `VaultDeviceTest` creates and
deletes a disposable Keystore alias: it verifies hardware security level, per-use strong
biometric policy, non-exportability and rejection without authentication (skips if strong
biometrics are not enrolled). `EntryUiDeviceTest` displays first-use/changed-key dialogs using
fake public-key metadata without a network or production DB writes, and checks the integrated
session screen retains the same terminal view and final grid through `Closed` until dismissal.
`TerminalDeviceTest` covers IME composition, keys, selection, resize and remount snapshots;
`TerminalVisualDeviceTest` captures renderer fixtures and reports frame timings.

Manual phone checks still required:
- Hosts: empty/list/add/edit/delete; changing address or port clears trust.
- Keys: Ed25519 generate; system-picker import, encrypted-file passphrase retry and format errors;
  copy/share the public line; delete and reselection on hosts.
- Biometric CryptoObject encrypt/decrypt success and cancellation; missing enrollment and
  enrollment invalidation must produce clear recovery messages. Never change enrollment or
  device security settings just to test these without separate user authorization.
- Activity recreation must keep established sessions; disconnect shows `Closed`, and "Close
  session" releases the handle after the renderer leaves composition.
- First-use and prominent changed-key warnings, previous fingerprints and closed/error states.
  Use test fixtures, not real hosts, until separately authorized.
- Integrated terminal IME show/hide geometry, committed/composing text, keys row, selection,
  scrolling, recreation and final-frame retention; collect apply/draw and Window frame timings.
- Capture representative screenshots and inspect them; visual verification and successful
  biometric round trips remain **pending phone**, not proved by compilation or JVM tests.

The vault accepts only StrongBox or TEE AES-256-GCM keys, prefers StrongBox when available,
requires BIOMETRIC_STRONG per operation and invalidates on new enrollment. No software or
device-credential fallback is allowed. Invalidated records remain for explanation/deletion;
re-import or generate a new SSH key to recover. Private keys are never exported or backed up:
`allowBackup=false` and cloud/device-transfer extraction rules exclude all app data.
Session ownership is application-scoped in M1, not a foreground service; process death ends it.
Disconnect leaves the active handle and final frame readable under `Closed`; "Close session"
or connecting elsewhere retires it. A session-screen display lease delays native `close()`
until the old screen leaves composition, then yields a main-loop turn for terminal disposal.
Activity recreation/navigation alone does not retire sessions. MainActivity uses
`adjustResize`; the root adds IME padding on host/key forms but not the session tab, where
TerminalScreen owns IME insets.
This does not prove SSH/IME/terminal acceptance or the broader v0 background-session test.
