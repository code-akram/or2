# Changelog

All notable changes to or2 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). One release stream covers the whole project:
a tag `vX.Y.Z` is the app's versionName, the Cargo workspace version and the `or2-pair` version at once.
Each release's notes are in [docs/releases/](docs/releases/).

## [Unreleased]

### Added

- Reply from an agent notification: type a reply in the notification and it reaches the agent's pane through
  herdr, submitted like the composer does, with no terminal open; the notification then shows what was sent.
  A host that is not connected says so instead.
- Image paste: send a picture to an agent from the composer's image button (the photo picker), from the
  keyboard (an inserted image), or from any app's Share menu. or2 shrinks it, strips its metadata (location
  included), uploads it over SFTP on the host's connection to `~/.cache/or2/images` (swept after seven days),
  and inserts its path at the prompt without pressing Enter.
- Several images at once: pick up to 10 in the photo picker or share several from another app. Each terminal has
  one queue that uploads them one after another (an image added meanwhile joins it), then inserts all their paths
  together; one that fails is skipped and reported, and Cancel stops the lot.

### Changed

- The arrow pad's keys are blue (accent glyphs on a blue-tinted fill with a blue edge, Enter solid accent), so they
  stand apart from the terminal behind them; the extras row has accent labels.
- A tap on a program that tracks the mouse (herdr, tmux with mouse on) is a click at that cell, so the program's own
  buttons work; a link still opens, and the keyboard key still opens the keyboard.
- A herdr terminal is called `herdr` (or `herdr <session>`), not after the pane it was opened on. Home's session
  card shows the working directory, else the agent (or the directory) herdr has in front, never `user@host`; a
  connected host's card shows the address in use. The picker's `Recent` tab is now `Open`.
- The toolbar has `⇧Tab` (Claude Code's mode cycle, which Gboard cannot send), `/` and `@` after History; `/` and
  `@` go into the composer at its cursor while it is open.
- "Send N lines?" and "Paste N lines?" are asked only while the program would run the lines one at a time; a
  program with bracketed paste on (a shell's line editor, an agent) gets them as one paste, and the toolbar's
  Paste is one bracketed paste there too.
- FFI API 18 (API 17 added `Session.mouse_click`, `Session.paste_text`, `HostConnection.reply_to_pane`,
  `upload_image`, `TerminalModes.bracketed_paste`; 18 adds `scroll_target`'s `client_id` and removes what the app
  never read: the session host-key path (`ConnectRequest`, `ConnectException`, `Session.approve_host_key` /
  `reject_host_key`, the `AwaitingHostKeyDecision` and `Authenticating` session states and their errors;
  `contract_probe_session` takes a size), `terminal_size`, `BuildInfo.minimum_android_sdk` / `renderer`,
  `ViewportScroll.Top`, `TmuxSession.created_unix` / `activity_unix`, `LinkHealth.since_ack_ms`,
  `HostCapabilities.utf8_locale`, the herdr workspace, tab and pane `focused` / `agent_status`, pane
  `tab_id` / `workspace_id` / `label` / `title`, agent `title` / `focused` and the view's `protocol`).
- Image upload is much faster on a distant host: a host's uploads share one SFTP session, its independent checks
  go out together, and old images are swept after the path is in, so an upload into an existing directory waits
  for 8 round trips instead of 27 (11 for the first on a connection).

### Fixed

- Nothing in the app can draw into the status bar any more: every screen is clipped to the safe area (only the
  background runs under it), sheets stop below it, and dialogs keep clear of it. One top bar everywhere, fixed,
  with a hairline that appears once content scrolls under it.
- After scrolling a herdr or tmux pane with a swipe that went to the program as wheel events, the scroll-to-bottom
  button now shows and returns the pane to its live screen; typing first returns it too.
- An agent that finishes its turn now notifies even when herdr reports it `idle` rather than `done`. herdr does
  that for a pane it counts as seen, which or2 makes it by opening it, so the usual flow (open the agent from the
  phone, send, lock the phone) never notified before.
- Tapping a second agent of a herdr session (inbox or notification) no longer opens a second herdr client next to
  the first, both showing the same focused pane: every way into herdr reuses the session's open terminal, after
  focusing the agent's pane. Duplicates already open are left alone.
- Swiping through a tmux terminal's history after switching it to another tmux session scrolls the
  session the terminal shows, not the one it was opened on.
- A terminal opened with an explicit Mosh choice gives up after its 15 s connect timeout when UDP is blocked; a slow
  socket open could make it wait up to twice that.

## [0.1.1] - 2026-10-02

Instant opens on every host, agent notifications, scrolling that follows the program, tap links and copy from the
host, gestures, a redesigned terminal top and Home cards, and `or2-pair` on a rail. See
[the release notes](docs/releases/v0.1.1.md).

### Added

- Agent notifications: one notification when a herdr agent needs input or finishes, none for the pane on
  screen; a tap opens the pane. On by default for every host shown in the inbox, with a switch in Settings.
- Wheel-aware scrolling: a swipe scrolls the program's view (wheel events when it tracks the mouse, tmux copy
  mode or herdr's own scroll otherwise) instead of shell history, and a scroll-to-bottom button.
- Tap a link to open it (http and https, OSC 8 hyperlinks and URLs wrapped across lines).
- Copy from the host: OSC 52 clipboard writes reach the phone's clipboard (on by default, a switch in
  Settings; the host can never read the phone's clipboard).
- Gestures: swipe for the next or previous tmux window or herdr tab, two fingers for panes and sessions.
- Hardware keyboard shortcuts: Ctrl+Shift+1..9, W, V, C, Enter and / (a shortcuts sheet).
- A Settings screen on Home.
- A redesigned terminal header: the minimise and sessions discs as a pair, the title centred (host, then the
  target), a slim drag handle, and the transport pill, which also shows how long a quiet mosh link has been
  silent. Status lines under it are one compact strip that never covers the terminal.
- `or2-pair` on macOS detects the application firewall blocking `mosh-server` and prints the commands that
  allow it.
- `or2-pair` and its installer print one calm rail from start to finish (clack-style): a symbol and colour per
  kind of line, fixes indented under their check, the code prompt and the QR on the rail, an explicit end
  (`Paired`, `Done`, `Cancelled`). Colour only on a terminal (`NO_COLOR` respected), an ASCII rail outside a
  UTF-8 locale.

### Changed

- Terminals open instantly on every host: under Auto, tmux and herdr terminals open over SSH at once and
  switch to mosh in the background when UDP gets through, so a firewall that drops mosh no longer costs
  seconds. The capability probe no longer waits for herdr's session listing. Whether UDP works is decided
  per connection (the 24-hour memory is gone), and a host whose UDP is blocked says so once on its screen.
- The Android native library build now always remaps the repository and cargo home paths, and fails if a
  build-machine path is left in `libor2_ffi.so` (v0.1.0 applied the remap by hand).
- The arrow pad floats over the terminal with no panel behind it.
- The session picker's "Skip" is now "Shell" (it opens a plain login shell).
- Home host cards have two actions: the card opens the host's page, and a `>_` button opens the session picker
  straight over Home (connecting the host first when needed). The picker no longer opens by itself.
- Opening a tmux or herdr session that is already open on that host brings its terminal to the front instead of
  opening a second one; a shell still opens a new one.
- Close session in the sessions sheet ends and removes a terminal in one tap.
- A host whose UDP was blocked is checked again after five minutes, unseen, on the next tmux or herdr open.
- FFI API 14.

### Fixed

- Status lines under the terminal header (the mosh fallback note, "last heard") no longer cover the terminal's
  top rows: the note is gone and "last heard" sits in the header row.

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
