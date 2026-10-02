# Changelog

All notable changes to or2 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). One release stream covers the whole project:
a tag `vX.Y.Z` is the app's versionName, the Cargo workspace version and the `or2-pair` version at once.
Each release's notes are in [docs/releases/](docs/releases/).

## [Unreleased]

## [0.1.0] - 2026-10-02

The first release: the Android app (a signed APK) and the `or2-pair` host CLI for Linux and macOS. See
[the release notes](docs/releases/v0.1.0.md) for what works, how to install it and the known shortcomings.

### Added

- SSH terminal (M1): Ed25519 keys generated in the app or imported, kept in the Android Keystore behind a
  biometric unlock; first-use and changed host-key prompts; libghostty-vt terminal with IME, key toolbar, arrow
  pad, composer, selection, scrollback and pinch zoom.
- Multiplexers (M2): one SSH connection per host carrying every terminal; tmux and herdr session pickers;
  multi-address hosts raced; a live herdr agent inbox across hosts that opens an agent's pane on a tap.
- Stays connected (M3): mosh terminals that roam and survive SSH loss, a foreground service, automatic
  reattach to the last pane (also after process death), AUTO transport with a mosh budget and fallback.
- Easy pair v2: `or2-pair` on the host and a QR scan on the phone, pairing over the SSH port with a one-time
  key; the host key trusted from the scan; `--manual`; host checks that print this host's exact fix.
- One add-host chooser (Easy pair with QR, Set up manually) and **New key** in the host form.
- No permission dialogs during a connect: the battery exemption ends host setup, the notification permission
  is offered on Home.
- Compact UI throughout, in Catppuccin Mocha; open-source licence screens generated from the dependency locks.
- `or2-pair` release binaries (static Linux x86_64 and aarch64, macOS Intel and Apple silicon) with
  `SHA256SUMS`, built by `cargo xtask dist` and the release workflow; `scripts/install-or2-pair.sh`, a
  checksum-verified installer that needs no GitHub API and no `sudo`.
- Optional local release signing for the APK (`~/.config/or2/signing.properties`); without it the release
  build stays unsigned for F-Droid.

[Unreleased]: https://github.com/code-akram/or2/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/code-akram/or2/releases/tag/v0.1.0
