# Changelog

All notable changes to or2 are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). One release stream covers the whole project:
a tag `vX.Y.Z` is the app's versionName, the Cargo workspace version and the `or2-pair` version at once.
Each release's notes are in [docs/releases/](docs/releases/).

## [Unreleased]

## [0.1.7] - 2026-10-08

A demo of the app in the README, and the tooling that makes it (versionCode 13, FFI API 23; the app is
unchanged from 0.1.6 apart from its version). See [the release notes](docs/releases/v0.1.7.md).

### Added

- The README opens with a demo GIF: the real app on a phone, driven by `ReadmeDemoDeviceTest` through
  invented hosts and agents (no network, keys or real hosts), filmed with screenshots and rendered by
  `cargo xtask demo-gif` into a captioned card with touch marks. `demo-gif --check` checks the checked-in
  GIF. See docs/build.md, "README demo".

## [0.1.6] - 2026-10-05

Terminal rendering, scrolling, RGB themes and launcher/picker polish (versionCode 12, FFI API 23).
See [the release notes](docs/releases/v0.1.6.md).

### Added

- A launcher icon: the or2 mark (a blocky "or", white on black) as an adaptive icon with a monochrome
  layer for Android's themed icons. The app previously showed Android's default icon.
- A minimalist launch splash: the white mark alone on the app's background, fading into the first screen.

### Changed

- Native terminal frame pulls and UniFFI decoding run off the UI thread, with bounded ordered
  handoff to vsync; the probe reports native reads, grid merges and ingress latency separately.
- Terminal rendering batches synthetic bold, caches row display lists across scrolling, and reuses
  moved rows over the FFI with coalescing-safe references (API 23); the native probe measures scrolling.
- Terminal rendering batches bounded glyphs at exact cell positions and merges background/decorations
  runs, reduces frame copies, and avoids redraws for wheel input that changes no local state.
- The terminal's default colours are Tokyo Night (background `#1A1B26`, foreground `#C0CAF5`, its 16 ANSI
  colours), matching the herdr theme on the owner's machines, so herdr's chrome and pane content agree. The app
  UI stays Catppuccin Mocha; the arrow-pad keys follow the new background.

### Fixed

- Host picker uses lowercase `shell` / `dirs`, with the shell pill matching the tab capsule's height.
- SSH/Mosh terminal launches advertise true-colour support, so programs keep their exact RGB
  themes instead of falling back to 256 colours (notably Pi in a plain Mac shell). Host themes
  and or2's palette are unchanged; new shells preserve login startup and directory selection.
- Cached terminal rows restore HWUI display lists discarded while offscreen instead of returning
  blank after scrolling; immutable drawing commands survive without reshaping cells.
- Every terminal delta rejects a missing sequence base, not just moved rows, and requests a full
  snapshot until recovery. A remount retries when a retiring display consumed its requested frame.

## [0.1.5] - 2026-10-04

Project directories and one-tap shells (versionCode 8, FFI API 22). See [the release notes](docs/releases/v0.1.5.md).

### Added

- Project directories: the host picker's **Dirs** tab includes live herdr agent/pane working directories
  (including Pi and plain shells), then recent Claude Code/Codex projects. Tap a path to open a new shell there
  with the host's transport preference. Live rows follow existing watches; no extra queries, daemon or migration.
  FFI API 22 adds pure Rust merging/validation, shared exact deduplication and a 20-path cap.

### Fixed

- Dirs now reflects working directories already shown in herdr's default/named sessions, independently of
  agent history. Changes/removals update automatically, and buffered/retired/other-host views cannot supply paths.
- Recent-directory discovery no longer discards realistic Codex metadata headers (instructions make them
  exceed the former 8 KiB limit) or large complete Claude entries. Codex headers allow 64 KiB with a combined
  512 KiB output budget; the five-second deadline and 1 MiB exec cap remain unchanged.
- Pairing lock guards explicitly unlock on drop, so a child inheriting a descriptor between fork and exec
  cannot keep an ended run looking live or delay the next authorization change.
- The SFTP test fixture no longer races the client's channel Close with its own close-on-drop, avoiding a
  missed callback and a false cleanup failure. Deterministic regressions cover both lock and close races.

## [0.1.4] - 2026-10-04

Spaces and herdr-style agent names. See [the release notes](docs/releases/v0.1.4.md).

### Added

- Spaces: a third, blue disc in a herdr terminal's header (after the orange and green ones, the same size) opens
  the session's spaces as herdr has them: each space's tabs (`tab ui`) and, under each tab, its agents with their
  status, name and task; the focused tab and pane are marked `● Current`. Tapping a tab or an agent focuses it in
  herdr and the terminal shows it. Shell and tmux headers are unchanged.
- FFI API 20: `HerdrAgent.title` (herdr's terminal title of the agent, trimmed, without a leading spinner or status
  glyph, at most 120 characters; a view whose only change is a title is delivered at most once a second),
  `HerdrView.focused_tab_id`, and `HostConnection.focus_herdr_tab(session, tab_id)` (herdr's tab focus, in order
  with the session's pane focuses).

### Changed

- Agents are named as herdr names them, everywhere they appear (Inbox, the picker, Spaces, notifications): the
  agent's name when it was started by one, else herdr's display name, else its kind; then the task it is on, from
  its terminal title, on its own muted line. A notification reads `Needs input · <task>` or `Done · <task>`.

### Fixed

- Host-supplied agent titles, display names and space/tab labels lose Unicode bidi and invisible formatting
  characters before being shown in the app or notifications, so those characters cannot disguise the text.
  Identifiers and agent names used as Reply identity are unchanged.
- The live mosh test's cleanup identifies only the fixture's IPv4 loopback socket, never another interface's
  server using the same UDP port; its fork-handover wait requires the reported server to be the sole owner.

## [0.1.3] - 2026-10-03

Zero-config Reply: `or2-pair` recommends herdr when it is missing and sets up herdr's integrations for the agents
on the host; the app offers **Enable Reply** for an agent that has none. See
[the release notes](docs/releases/v0.1.3.md).

### Added

- `or2-pair` sets up Reply for the agents on the host: after the checks it asks herdr which of its integrations
  are installed and, for the agents found on the host whose integration is missing or outdated, asks once
  (`Set up Reply for pi, opencode? [Y/n]`) and runs `herdr integration install` for each, saying how each went;
  running sessions load it when they next start. Without a terminal, with `--check` or `--manual`, or on no, it
  prints the commands instead. Agents already set up are one line (`Reply ready for claude, codex`). Nothing in
  it stops the pairing.
- `or2-pair` says when Codex's shared daemon will keep Reply from working (herdr#4649) and how to turn it off;
  it changes nothing in Codex's config.
- Enable Reply from the phone: an agent with no Reply because herdr's integration for it is missing on the host
  (pi, opencode, ...) gets **Enable Reply** in its Inbox row and in its notification. One confirmation, then or2
  runs `herdr integration install <id>` on the host and says the outcome (`Done. Restart pi to reply to it.`).
  The notification's action only opens the app to that confirmation. An agent whose integration is already there
  (Codex with herdr 0.9.3) or whose kind has none shows nothing new.
- FFI API 19: `HostConnection.install_herdr_integration(id)` (one exec through the probed herdr; the id must be one
  of herdr 0.9.3's integrations) and `HostConnection.herdr_integrations()` (`herdr integration status`, read as
  `HerdrIntegration { id, state }`, no paths).

### Changed

- `or2-pair` without herdr is a warning, not a note: it says that or2's agents inbox, notifications and Reply need
  it, and gives herdr's own install command (`curl -fsSL https://herdr.dev/install.sh | sh`, or Homebrew, mise,
  Nix), which it never runs.

## [0.1.2] - 2026-10-03

Reply to an agent from its notification, image paste (several at once, about a second each), and one simpler
model for the whole app: Home holds each host's terminals, the picker lists herdr agents, one Terminals sheet,
a toolbar that fits; notifications for every finished turn. See [the release notes](docs/releases/v0.1.2.md).

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
- The session picker's herdr tab lists the agents of each running session, grouped by workspace, each with its
  status and directory, as the inbox shows them; tapping one opens it as the inbox does (its pane focused, the
  session's terminal reused). The session's own row (its name and agent count, as a tmux row shows its windows) opens it as it is.

### Changed

- The arrow pad's keys are blue (accent glyphs on a blue-tinted fill with a blue edge, Enter solid accent), so they
  stand apart from the terminal behind them; the extras row has accent labels.
- A tap on a program that tracks the mouse (herdr, tmux with mouse on) is a click at that cell, so the program's own
  buttons work; a link still opens, and the keyboard key still opens the keyboard.
- A herdr terminal is called `herdr` (or `herdr <session>`), not after the pane it was opened on. Home's session
  card shows the working directory, else the agent (or the directory) herdr has in front, never `user@host`; a
  connected host's card shows the address in use. The picker's `Recent` tab is now `Open`.
- The toolbar keys run `Ctrl`, `Esc`, `Tab`, `⇧Tab`, `⇧`, arrows, Paste, `/`, `@`; `⇧` latches Shift for the next
  key as `Ctrl` latches Ctrl (Shift+Enter, Shift+arrows, a capital).
- The toolbar has `⇧Tab` (Claude Code's mode cycle, which Gboard cannot send), `/` and `@` after Paste; `/` and
  `@` go into the composer at its cursor while it is open.
- The terminal toolbar fits a phone without scrolling: the Panes key is gone (the header's green disc opens the same
  sheet) and so is History (drag down to scroll back; the round button returns to the live screen). While text is
  selected, Copy and Clear take the place of `⇧Tab`, `/` and `@`. The arrow pad's symbol row no longer repeats `/`.
- The shortcuts sheet is now **Gestures & shortcuts**: what a tap, a long press, a drag, the swipes and a pinch do
  comes first, then the keyboard shortcuts.
- Closing the composer gives the keys back to the terminal, and the arrow pad and the composer no longer stay open
  together: opening one closes the other.
- Cards in a sheet sit a step above the sheet instead of below it (the sessions, shortcuts and share sheets drew
  them darker).
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
- Home is the one place for hosts and their terminals. Each host card shows its open terminals as live thumbnails,
  each with a small `×`; a tap on the card opens the session picker (connecting the host first when it is not), and
  `⋯` (or a long press) opens the host's menu. `Connect all` sits at the end of the HOSTS header.
- Closing a tmux or herdr terminal only ends or2's view of it (the session runs on) and takes one tap; closing a
  shell asks first, since its programs end with it. A closed terminal is marked `Closed` on its thumbnail.
- The session picker has two tabs, herdr and tmux; a session that already has a terminal in or2 is marked
  `● Open`, and choosing it switches to that terminal. The line about mosh's UDP being blocked is in the picker.
- The picker reads the tmux sessions again each time it opens and whenever its tmux tab is shown, with a small
  spinner while it does; Refresh is on the tmux tab only and shows the same spinner. herdr's default session is
  listed by its name alone (no `(default)`).
- The terminal's green disc opens **Terminals**: every open terminal by host, each with its `×`, then **Copy
  screen** and **Gestures & shortcuts**. System Back from a terminal goes Home, as the orange disc does.
- Going back to an open terminal (a thumbnail, the Terminals sheet, the picker, a reattach) shows it as it is;
  only an agent you tap in the inbox or a notification moves herdr's focus to its pane.
- The host form is **New host** / **Edit host** with one Save at the end, and an edited host can be deleted from
  it (the same confirmation as Home's). The words are Host, Terminal, Connect and Retry throughout: `Unlock` and
  `Unlock and connect` are `Connect`.

### Removed

- The host screen: its picker, address detail and UDP line are on Home's card and in the picker. A saved host
  screen opens Home. The inbox's host rows are no longer links (their Connect or Retry stays).
- Home's `SESSIONS` row, the `>_` session button and the `Working` / `Needs attention` chips (the inbox icon's badge
  says it), the picker's `Open` tab, and the sheet's separate `Close session` row and pill.

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
- A Resume or a notification's tap made while another unlock was running is no longer dropped: it connects once
  that unlock ends. The Resume card says `Mosh` / `SSH` (it said `Ssh`), and it and the `Resuming…` / `Focusing…`
  cards name the terminal by its title.
- A herdr terminal's thumbnail follows the pane herdr has in front, whatever pane it was opened on.
- Ctrl+Shift+W closes a terminal the way its `×` does (an open one is disconnected, so its mosh server stops).
- A message of several lines sent from the composer buzzes once, on the confirmation's **Send**, not twice.
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

[Unreleased]: https://github.com/code-akram/or2/compare/v0.1.7...HEAD
[0.1.7]: https://github.com/code-akram/or2/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/code-akram/or2/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/code-akram/or2/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/code-akram/or2/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/code-akram/or2/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/code-akram/or2/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/code-akram/or2/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/code-akram/or2/releases/tag/v0.1.0
