# herdr-over-russh M0 result

Tested 2026-09-30 on x86_64 Arch Linux with Rust 1.98.1, OpenSSH 10.5p1,
herdr 0.9.3 (protocol 22), Android NDK r30, and cargo-ndk 4.1.2.

## Verdicts

| Criterion | Verdict | Evidence |
|---|---|---|
| Ed25519 key authentication | **PASS** | A generated client key authenticated to the unprivileged test sshd. The client prints and compares the generated host's SHA-256 fingerprint. |
| Socket discovery by exec | **PASS** | A russh exec channel ran `herdr status server`; the `socket:` line was parsed. An explicit PATH was needed for non-interactive SSH. |
| OpenSSH streamlocal | **PASS** | `russh::client::Handle::channel_open_direct_streamlocal` opened `direct-streamlocal@openssh.com` channels through OpenSSH to both sockets. |
| Subscribe + ack + snapshot bootstrap | **PASS** | Subscription channel A returned `subscription_started`; independent channel B returned a real live `session.snapshot`. The live snapshot contained 5 workspaces, 7 tabs, 7 panes, and 1 agent at the instant tested (names and paths intentionally omitted). Connect-to-snapshot-installed was 365 ms live and 460 ms isolated. |
| Live events | **PASS** | The isolated test received ordered create/focus/layout events, `pane_closed`, and `workspace_closed` after create, split, close, and cleanup requests on independent channels. |
| Schema code generation | **PARTIAL** | cargo-typify generated concrete success/event types that compiled and deserialized the captured real snapshot and event. The bundle needs preprocessing and a `regress` runtime dependency; invoking typify on the bundle itself only generates a transparent `serde_json::Value` wrapper. |
| Android arm64 build | **PASS** | API 31 release cdylib built and is an AArch64 Android ELF. `llvm-readelf` reports only `libdl.so` and `libc.so`. |

The installed 0.9.3 documentation corrects an important design assumption: snapshots and
events have **no shared sequence boundary**, so buffered events must not be blindly applied over
the snapshot. Events received while snapshotting are invalidation signals: install the snapshot,
perform serialized authoritative refreshes, and refresh again if another event arrived during a
read. On `events_lost`, resubscribe and repeat the reconciliation.

## Versions and crypto

Direct dependencies are exactly pinned in `Cargo.toml`: russh 0.63.3, tokio 1.47.1,
serde_json 1.0.145, and anyhow 1.0.100. `Cargo.lock` pins the full graph. russh 0.63.3 defaults
to **aws-lc-rs**, not ring; this build resolved aws-lc-rs 1.18.1 and aws-lc-sys 0.45.0.
The NDK build succeeded without manual CMake flags: setting `ANDROID_NDK_HOME` was sufficient;
cargo-ndk supplied the API-31 compiler/toolchain environment and aws-lc-sys used CMake.

## Isolation and safety

The mutable test ran as the named session `or2-spike` using:

```sh
herdr --session or2-spike server
```

Its socket was `~/.config/herdr/sessions/or2-spike/herdr.sock`, distinct from the live default
socket `~/.config/herdr/herdr.sock`; this equality check was made before mutation. Only
`pane.list`, `events.subscribe`, and `session.snapshot` were sent to the live server. All create,
split, pane-close, and workspace-close requests went to the named server. It was stopped with
`herdr session stop or2-spike`.

## Reproduction

From the repository root (scratch material must remain under `.amp/in/herdr-spike`):

```sh
S="$PWD/.amp/in/herdr-spike"; P="$PWD/spikes/herdr-ssh"
mkdir -p "$S"
ssh-keygen -q -t ed25519 -N '' -f "$S/host_ed25519"
ssh-keygen -q -t ed25519 -N '' -f "$S/client_ed25519"
cp "$S/client_ed25519.pub" "$S/authorized_keys"; chmod 600 "$S/authorized_keys"
cat >"$S/sshd_config" <<EOF
Port 2222
ListenAddress 127.0.0.1
HostKey $S/host_ed25519
AuthorizedKeysFile $S/authorized_keys
PidFile $S/sshd.pid
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
StrictModes no
AllowStreamLocalForwarding yes
PermitOpen any
PermitTTY no
LogLevel VERBOSE
EOF
/usr/bin/sshd -D -e -f "$S/sshd_config" >"$S/sshd.log" 2>&1 & echo $! >"$S/sshd.launch.pid"
ssh -i "$S/client_ed25519" -p 2222 -o StrictHostKeyChecking=accept-new \
  -o UserKnownHostsFile="$S/kh" 127.0.0.1 true
FP=$(ssh-keygen -lf "$S/host_ed25519.pub" -E sha256 | awk '{print $2}')

cd "$P"
cargo build
EXPECTED_HOST_FINGERPRINT="$FP" ./target/debug/herdr-ssh-spike "$S/client_ed25519" live

herdr --session or2-spike server >"$S/herdr-isolated.log" 2>&1 &
test "$(herdr status | awk '/socket:/{print $2}')" != \
     "$(herdr --session or2-spike status | awk '/socket:/{print $2}')"
EXPECTED_HOST_FINGERPRINT="$FP" ./target/debug/herdr-ssh-spike \
  "$S/client_ed25519" or2-spike

herdr api schema --output "$P/herdr-api.schema.json"
cargo install cargo-typify --version 0.10.0-alpha.1 --locked
# Extract each member and rewrite local refs before generation, for example:
jq '(.schemas.success_response | walk(if type == "string" then \
  gsub("#/schemas/success_response/"; "#/") else . end))' \
  herdr-api.schema.json >"$S/success.schema.json"
cargo typify -B -o "$S/generated_success.rs" "$S/success.schema.json"

export ANDROID_NDK_HOME=$HOME/.local/share/android/android-ndk-r30
cargo ndk -t arm64-v8a --platform 31 build --release
file target/aarch64-linux-android/release/libherdr_ssh_spike.so
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf" \
  -d target/aarch64-linux-android/release/libherdr_ssh_spike.so

herdr session stop or2-spike
kill "$(cat "$S/sshd.launch.pid")"
```

Output excerpts (identifiers redacted):

```text
host_fingerprint=SHA256:<redacted>
auth=ed25519 success
socket=<live-socket>
subscription_ack={"id":"sub_1","result":{"type":"subscription_started"}}
snapshot_summary workspaces=5 tabs=7 panes=7 agents=1 installed_ms=365
live_event={..."event":"pane_closed"...}
live_event={..."event":"workspace_closed"...}

libherdr_ssh_spike.so: ELF 64-bit LSB shared object, ARM aarch64, ... for Android 31, built by NDK r30
NEEDED Shared library: [libdl.so]
NEEDED Shared library: [libc.so]
```

## Problems and workarounds

* Non-interactive sshd did not inherit the user's local-bin PATH. The exec command explicitly uses
  `PATH=/home/<user>/.local/bin:/usr/local/bin:/usr/bin:/bin`; production should probe common
  locations rather than embed a username.
* `pane.agent_status_changed` requires `pane_id`. A broad subscription containing only its type was
  rejected exactly as: `invalid request: missing field \`pane_id\` at line 1 column 540`. The spike
  now performs a read-only `pane.list` and adds one status subscription per pane, alongside broad
  workspace/tab/pane lifecycle subscriptions. Production must add subscriptions or reconcile when
  panes are created.
* `cargo typify herdr-api.schema.json` treats the nonstandard top-level bundle as unconstrained and
  emits only `pub struct HerdrApi(pub serde_json::Value)`. Extract `schemas.success_response`,
  `schemas.event`, etc. and rewrite their bundle-root `$ref`s before generation. Generated pattern
  validators also require `regress`; without it compilation failed with
  `error[E0433]: cannot find \`regress\` in the crate root`. With `regress = "0.12"`, real captured
  snapshot and lifecycle event JSON both deserialized successfully. Separate generated modules have
  duplicate helper names, so production should either generate a normalized combined schema or
  place each family in a module and expose a small facade.

## Recommendation

Proceed with russh 0.63.3 for `or2-core::herdr`. Model one long-lived event stream plus short-lived
request channels over a shared SSH handle. Pin host keys outside Rust storage, discover/cache the
remote herdr executable and socket per connection, resnapshot after every reconnect, and implement
events as cache invalidations with serialized authoritative refreshes and `events_lost` recovery.
Generate protocol-family modules from a deterministic schema-normalization script, include
`regress`, and test generated types against versioned sanitized fixtures. Keep aws-lc-rs unless APK
size or build-time measurements justify explicitly switching russh to its ring feature.

## Verification and cleanup

`cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` passed. The
Android release build passed and was inspected with `file` and NDK `llvm-readelf -d`.
The recorded spike sshd PID and isolated herdr launch PID are stopped. `herdr --session or2-spike
status` reports `not running`; default `herdr status` still reports version 0.9.3 `running` at the
original live socket. `pgrep -a sshd` shows only the pre-existing system listener/session processes;
the recorded spike sshd PID is absent.
