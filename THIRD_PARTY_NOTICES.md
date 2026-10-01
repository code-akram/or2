# Third-party notices

Code ported or vendored into or2. Dependencies pulled through Cargo or Gradle are not listed here.

| Source | Commit | Path | Licence | Copyright |
|---|---|---|---|---|
| [Gradle 8.13](https://github.com/gradle/gradle/tree/v8.13.0) | `073314332697ba45c16c0a0ce1891fa6794179ff` | `android/gradlew`, `android/gradlew.bat`, `android/gradle/wrapper/gradle-wrapper.jar` | Apache-2.0; full text in `android/gradle/LICENSE` | © 2015–2021 the original authors (script headers); Gradle contributors |
| [mosh-rs](https://github.com/wilsonglasser/mosh-rs) (library only; no CLI, platform terminal code, vt100 screen or prediction engine) | `90b37125f5e4a598be91dec37d23921b6865276e` | `core/or2-core/src/mosh/ssp/` (`crypto.rs`, `error.rs`, `key.rs`, `packet.rs`, `sender.rs`, `statesync.rs`, `transport.rs` with module paths and key zeroizing changed; `screen.rs`, `session.rs`, `terminal.rs` derived and reworked as described in each file header) | GPL-3.0-or-later (compatible with or2's; full text in `LICENSE`) | Wilson Glasser (mosh-rs author; upstream carries no per-file notices, only its `LICENSE` and the Cargo manifest licence); the protocol follows mosh, copyright 2012 Keith Winstein and contributors |
