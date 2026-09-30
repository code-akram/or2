# M0 Rust mosh feasibility result

Tested 2026-09-30 on Arch Linux with Rust 1.98.1, stock `mosh-server 1.4.0`,
Android NDK r30, and Android API 31.

## Verdicts

- **Interop with stock server: PASS.** A real 1.4.0 server returned
  `hello-or2-42`; a subsequent `stty size` returned `SIZE:30 100`.
- **Embeddable API: PASS.** `mosh-rs::MoshSession<S: Screen>` is a library API with input,
  resize, pump, screen state, prediction, health, shutdown, and socket-handle methods. It does not
  require its CLI. Its primary integration is a framebuffer/`Screen` trait; it does not expose the
  decoded host byte stream as a public session API.
- **Roaming: PASS (source-port change).** `roam_now()` rebound the live client from UDP source port
  39881 to 45712, retained the old receive socket, and the same server session returned
  `roaming-or2-ok`. This exercises mosh's authenticated server retargeting. It does not prove a
  source-IP/interface change because the public API only supports a wildcard bind, not a caller-
  supplied socket/address.
- **Android aarch64 build: PASS.** The library and a tiny `cdylib` wrapper built for
  `aarch64-linux-android`, API 31. The result is an AArch64 Android ELF needing only `libdl.so` and
  `libc.so`.

## Candidate comparison

| Candidate (pinned revision) | Licence | Last commit at evaluation | Prediction/local echo | API and output shape | Result |
|---|---|---|---|---|---|
| `wilsonglasser/mosh-rs` `90b37125f5e4a598be91dec37d23921b6865276e` (`mosh-rs` 0.1.0) | GPL-3.0-or-later | 2026-08-23 | Yes, adaptive/always/never | Proper generic library, caller-provided `Screen`; framebuffer and terminal diffs, no public raw host-byte session stream | **Chosen; live-tested PASS** |
| `at-least/mosh` `8b8f75b328dbf8a568977840155983463f955f46` (`mosh-client` 0.1.0) | GPL-3.0-or-later | 2026-09-24; repository frozen and moved to conch | Yes, conservative echo prediction | Embeddable `MoshSession<MoshDisplay>`; display callbacks plus drainable `take_host_bytes()` raw capture | Strong alternative, source-reviewed, not live-tested |
| `Nopm/WinMosh` `8287f34bdbe726bad42650718504cea1c98792d5` (`mosh-client` 0.1.0) | AGPL-3.0-or-later | 2026-07-05 | Yes | Windows-oriented binary; protocol modules are private under `main.rs`, owns framebuffer/console; significant extraction needed | Rejected |

AGPL-3.0-or-later can be combined into this GPL-3.0-or-later application only if the combined work
is distributed under the AGPL's additional network-source terms; that is an unnecessary licence
and maintenance burden. `libmoshpit` was skipped as instructed: it is not stock-mosh-wire-compatible.

## Reproduction

The standalone spike pins the git revision in `Cargo.toml`; `Cargo.lock` pins its complete graph.
The harness starts and records its own daemon PID, connects directly (no SSH), tests command output,
resize, and forced source-port roaming, then kills that exact PID even when an assertion panics.

```sh
cd /home/akram/code/or2/spikes/mosh-rs
cargo run --bin interop
cargo test
cargo clippy --all-targets -- -D warnings
ANDROID_NDK_HOME=/home/akram/.local/share/android/android-ndk-r30 \
  cargo ndk -t arm64-v8a --platform 31 build --release --lib
file target/aarch64-linux-android/release/libor2_mosh_spike.so
/home/akram/.local/share/android/android-ndk-r30/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf \
  -d target/aarch64-linux-android/release/libor2_mosh_spike.so | grep NEEDED
```

Key output:

```text
MOSH CONNECT 60101 <redacted-session-key>
[mosh-server detached, pid = 1913778]
PASS interop resize roaming; source port 39881 -> 45712
cleanup kill status=exit status: 0

libor2_mosh_spike.so: ELF 64-bit LSB shared object, ARM aarch64, ... for Android 31,
built by NDK r30 (16248370), not stripped
NEEDED Shared library: [libdl.so]
NEEDED Shared library: [libc.so]
```

The harness's equivalent direct server command is:

```sh
LANG=C.UTF-8 mosh-server new -i 0.0.0.0 -p 60101 -- \
  /bin/bash --norc --noprofile
```

Without a UTF-8 locale, startup failed exactly as follows; setting `LANG=C.UTF-8` is the workaround:

```text
mosh-server needs a UTF-8 native locale to run.
Unfortunately, the local environment ([no charset variables]) specifies
the character set "US-ASCII",
```

## Crypto and dependencies

`mosh-rs` uses pure-Rust RustCrypto `aes 0.8.4` plus `ocb3 0.1.0` for AES-128-OCB3, and
`flate2 1.1.10`. It needs neither OpenSSL nor nettle. The successful Android dynamic table confirms
there is no external crypto shared-library dependency.

## Recommendation for `or2-core`

Use `wilsonglasser/mosh-rs` pinned to the revision above, initially as a git dependency, then vendor
or maintain a small GPL-compatible fork before production. It is the best fit because its
caller-owned `Screen` maps naturally to Rust-owned terminal state, local prediction is complete,
stock interoperability and roaming worked, and Android cross-compilation is already clean.

The required fork gap is transport injection: `session.rs` directly owns `std::net::UdpSocket`, and
its private `bind_socket(server)` wildcard-binds a fresh socket. Refactor construction, pumping, and
`hop_port`/`roam_now` around or2's UDP `Transport` trait (or accept supplied sockets). This also
allows an Android network callback to bind a new socket to a specific network/interface and proves
true address migration, rather than only source-port migration. Decide whether `Screen` should be
implemented directly over libghostty-vt; if or2 instead needs raw host bytes, add a public decoded
host-event callback/capture similar to `at-least/mosh`'s `take_host_bytes()`.

No official C++ fallback was attempted because a Rust candidate passed all required feasibility
criteria. The C++ path would additionally require NDK ports/build integration for protobuf,
OpenSSL or nettle, and ncurses, and would impose a C++ FFI layer.

## Cleanup

Before the spike, PID 1896108 was an unrelated pre-existing `mosh-server` and was not touched.
The spike-owned PID 1913778 was killed by exact PID. Final `pgrep -a mosh-server` showed only the
same pre-existing PID 1896108; no spike server remained.
