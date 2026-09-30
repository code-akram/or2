# libghostty-vt Android feasibility spike

**Host grid correct: PASS**

**Android aarch64 cdylib builds: PASS**

## Pinned inputs

- `libghostty-vt` / `libghostty-vt-sys` 0.2.1 from libghostty-rs commit
  `8953a740bc378cec3e07e1f6ca949f0595eab19b` (Cargo git dependency pinned by `rev`).
- Ghostty commit `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`, pinned by that binding's
  `build.rs` and checked out in `ghostty-source/`.
- Zig 0.16.0. Ghostty's `build.zig.zon` specifies minimum version 0.16.0, so the installed
  system Zig was suitable and no second Zig download was needed.
- Rust 1.98.1, Android NDK r30, Android API 31.

## Reproduction

From this directory:

```sh
git clone --filter=blob:none --no-checkout https://github.com/ghostty-org/ghostty.git ghostty-source
git -C ghostty-source checkout 22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018
export GHOSTTY_SOURCE_DIR="$PWD/ghostty-source"
cargo run --release

export ANDROID_NDK_HOME=/home/akram/.local/share/android/android-ndk-r30
cargo ndk -t arm64-v8a --platform 31 build --release

SO=target/aarch64-linux-android/release/libghostty_android_spike.so
file "$SO"
stat -c '%s bytes' "$SO"
$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm -D --defined-only "$SO" | grep or2_ghostty_smoke
$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf -d "$SO" | grep NEEDED
```

`run.sh` runs the host and Android builds after the pinned Ghostty checkout exists.

## Host result

The test opens `top` in a real 100x30 PTY, waits two seconds, saves the 4,518 raw bytes to
`top-raw.bin`, feeds those bytes to an equally sized `Terminal`, and saves all rendered cells to
`top-grid.txt`. Excerpt:

```text
Tasks: 324 total, 1 running, 323 sleep, 0 d-sleep, 0 stopped, 0 zombie
%Cpu(s):  0.3 us,  0.3 sy,  0.0 ni, 99.4 id,  0.0 wa,  0.0 hi,  0.0 si,  0.0 st
MiB Mem :  63935.3 total,  15907.1 free,  13833.0 used,  44806.7 buff/cache
MiB Swap:  32734.0 total,  32734.0 free,      0.0 used.  50102.3 avail Mem

    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
      1 root      20   0   21668  16200  11328 S   0.0   0.0   0:12.88 systemd
```

The deterministic escape-sequence test verifies cursor movement, bold red SGR (the `R` cell's
safe-API style is `Palette(RED)`), primary/alternate-screen switching, and wide-cell layout. Both
`界` and `😀` occupy a head cell followed by an empty tail cell. `handwritten-grid.txt` contains:

```text
before alt screen:
base
  RED 界  😀

after returning:
base
  RED 界  😀
```

The safe key encoder produced (decimal bytes):

```text
Ctrl+C: [3]
ArrowUp: [27, 91, 65]
Enter: [13]
```

## Android result

The binding's current `build.rs` already maps Rust `aarch64-linux-android` to Zig
`aarch64-linux-android`; no patch, explicit sysroot, or custom linker workaround was needed.
`cargo-ndk` supplied the NDK clang/link environment while Zig built the static VT archive.

```text
ELF 64-bit LSB shared object, ARM aarch64, ... for Android 31, built by NDK r30, not stripped
size: 2,371,888 bytes
export: 000000000006fa08 T or2_ghostty_smoke
NEEDED: libdl.so, libc.so
```

## Problems and recommendation

- `top` restores cursor state when killed and emits a final newline, scrolling its title row out
  of the final grid. Assertions therefore use the stable `Tasks:` and `PID USER` rows rather than
  `load average`; this is application cleanup behavior, not an emulation error.
- First compilation took about 48 seconds and the unstripped release `.so` is 2.26 MiB. Production
  packaging should strip it and cache the Zig archive.
- The API is explicitly pre-1.0 and all terminal objects are `!Send`/`!Sync`. Pin both commits and
  own each terminal on one Rust task/thread behind channels.

**Recommendation:** adopt libghostty-vt for `or2-core`. The safe Rust API covers rendering,
styles, Unicode wide cells, alternate screens, and key encoding, and its pinned Zig build works
unchanged in an API-31 arm64 Android cdylib. Keep the bindings/ Ghostty revisions locked and add
the same host fixture plus an Android cross-build to CI.
