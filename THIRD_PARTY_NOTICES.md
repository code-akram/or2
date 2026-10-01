# Third-party notices

What or2 ships from other projects, and under which licences. This file is authoritative for code
ported or vendored into the repository and for components built from source outside Cargo and
Gradle; the app embeds it (Home, About or2, Open source licenses, Vendored).

Dependencies pulled through Cargo or Gradle are listed, with their full licence texts, by the
generator `scripts/gen-licenses.sh` (see `docs/build.md`): the crates linked into the app's native
library in `android/app/src/main/assets/licenses/rust.json`, the release runtime classpath in
`android/app/src/main/assets/licenses/android.json` (both shown in the app under Open source
licenses) and the crates linked into the `or2-pair` CLI in `core/or2-pair/THIRD_PARTY.md`. All of
them were checked for GPL-3.0-or-later compatibility (see the README).

## Vendored or ported code

| Source | Commit | Path | Licence | Copyright |
|---|---|---|---|---|
| [Gradle 8.13](https://github.com/gradle/gradle/tree/v8.13.0) | `073314332697ba45c16c0a0ce1891fa6794179ff` | `android/gradlew`, `android/gradlew.bat`, `android/gradle/wrapper/gradle-wrapper.jar` | Apache-2.0; full text in `android/gradle/LICENSE` | © 2015–2021 the original authors (script headers); Gradle contributors |
| [herdr 0.9.3 API schema](https://github.com/herdrdev/herdr/tree/v0.9.3) (`herdr api schema --json`) | `7b116c05bfda646af39d2524c54e70c751f57ee8` (tag `v0.9.3`) | `core/or2-core/src/herdr/schema.json` (normalized copy), `core/or2-core/src/herdr/generated.rs` (generated from it) | Apache-2.0 | herdr contributors (the LICENSE file names no holder) |
| [Catppuccin Mocha](https://github.com/catppuccin/catppuccin) palette (colour values only: the UI tokens in `android/app/src/main/java/io/github/code_akram/or2/ui/Theme.kt` and the terminal defaults in `core/or2-core/src/terminal.rs`; no files copied) | n/a (values read from the palette's documentation) | `android/app/src/main/java/io/github/code_akram/or2/ui/Theme.kt`, `core/or2-core/src/terminal.rs` | MIT | © 2021 Catppuccin |
| [mosh-rs](https://github.com/wilsonglasser/mosh-rs) (library only; no CLI, platform terminal code, vt100 screen or prediction engine) | `90b37125f5e4a598be91dec37d23921b6865276e` | `core/or2-core/src/mosh/ssp/` (`crypto.rs`, `error.rs`, `key.rs`, `packet.rs`, `sender.rs`, `statesync.rs`, `transport.rs` with module paths and key zeroizing changed; `screen.rs`, `session.rs`, `terminal.rs` derived and reworked as described in each file header) | GPL-3.0-or-later (compatible with or2's; full text in `LICENSE`) | Wilson Glasser (mosh-rs author; upstream carries no per-file notices, only its `LICENSE` and the Cargo manifest licence); the protocol follows mosh, copyright 2012 Keith Winstein and contributors |

## Built from source outside Cargo

`libghostty-vt` is built by the Rust build script of `libghostty-rs` (pinned in `core/Cargo.toml`)
with Zig and linked statically into `libor2_ffi.so`. Nothing of it is copied into this repository.
`scripts/gen-licenses.sh` stops if the Ghostty commit that build script pins changes, so this table
and the licence texts in `scripts/licenses/ghostty/` cannot go stale unnoticed.

| Component | Pinned source | Linked as | Licence | Copyright |
|---|---|---|---|---|
| [Ghostty](https://github.com/ghostty-org/ghostty) (libghostty-vt) | commit `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018` | static archive built with Zig 0.16.0 | MIT | © 2024 Mitchell Hashimoto, Ghostty contributors |
| [Highway](https://github.com/google/highway) 1.2.0 | commit `66486a10623fa0d72fe91260f96c892e41aceb06` (Ghostty `pkg/highway`) | in the static archive | Apache-2.0 OR BSD-3-Clause | © The Highway Project Authors |
| [simdutf](https://github.com/simdutf/simdutf) 5.2.8 | vendored by Ghostty in `pkg/simdutf` | in the static archive | Apache-2.0 OR MIT | © The simdutf authors |
| [uucode](https://github.com/jacobsandlund/uucode) 0.2.0 | commit `2826a37a4562284fdacd8fa029d49509cc9bffcd` (Ghostty `build.zig.zon`) | tables in the static archive (generated from the Unicode Character Database, Unicode License v3) | MIT | © 2026 Jacob Sandlund; Unicode data © Unicode, Inc. |
| [Bjoern Hoehrmann's UTF-8 decoder](http://bjoern.hoehrmann.de/utf-8/decoder/dfa) | n/a (Ghostty `src/terminal/UTF8Decoder.zig` is based on it) | in the static archive | MIT | © 2008-2009 Bjoern Hoehrmann |
| [Zig](https://github.com/ziglang/zig) compiler runtime | Zig 0.16.0 | `compiler_rt` in the static archive | MIT | © Zig contributors |

## Not shipped

- Fonts: none are bundled. The terminal and the UI use the device's system fonts (the validated
  system monospace, see `docs/ui.md`); JetBrains Mono is not included.
- Icons: the UI's icons are path data in `ui/Icons.kt` and a status-bar glyph in
  `res/drawable/ic_stat_or2.xml`; no icon set is bundled.
- CameraX (`androidx.camera`, Apache-2.0) and ZXing core (`com.google.zxing:core`, Apache-2.0),
  used by Easy pair, arrive through Gradle and are listed by the generator in `android.json`; no code
  was taken from them. The crates linked into `or2-pair` are listed in `core/or2-pair/THIRD_PARTY.md`.
