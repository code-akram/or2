# Status and next steps

Read this first when picking the work up. The details are in [design](design.md) (checklists),
[contracts](contracts.md) (what the code must do), [roadmap](roadmap.md) and [build](build.md).
Updated 2026-10-03.

## Where it stands

- **Released: [v0.1.1](releases/v0.1.1.md)** (2026-10-02, after [v0.1.0](releases/v0.1.0.md) the same day): the
  signed APK and `or2-pair` for Linux x86_64/aarch64 (static) and macOS Intel/Apple silicon.
- **On `main`, not released: the v0.1.2 candidate** (`3c667b2` or later). The app's `versionName` is still
  `0.1.1` (`versionCode 2`); the bump to 0.1.2 happens at release time. The
  [CHANGELOG](../CHANGELOG.md) "Unreleased" section lists everything, and FFI `API_VERSION` is 18.
- **The owner's phone runs that candidate** (installed 2026-10-03 18:38, with agents in the picker) as the signed release build: an in-place
  update, so the hosts and keys are kept. It awaits the owner's QA of the streamlined UI (below).

### In the v0.1.2 candidate

- **Reply from an agent notification.** It sends to the agent's pane through herdr, with no terminal open.
  - The reply is tied to the exact agent instance: herdr's `agent_session` id, or the name of an agent herdr
    started. Agents with neither get no Reply action.
  - For agents herdr did not start (Claude Code panes), the reply is typed: guarded checks, then one
    `pane.send_input` carrying the text and Enter.
  - **Owner decision, 2026-10-02:** keep this guarded typed reply. The residual risk, stated in the contract:
    an agent that exits within about one network round trip after the checks gets the reply typed into its
    shell. Closing that needs an atomic, agent-bound herdr operation.
- **Image paste.** The composer's image button (photo picker), images inserted by the keyboard, and
  Share → or2.
  - The image is downscaled and its metadata stripped, then uploaded over SFTP on the existing connection
    to `~/.cache/or2/images` (private directory, swept after 7 days).
  - Its path is inserted with no Enter.
- **UI polish since v0.1.1.**
  - Blue arrow-pad keys.
  - A tap is a mouse click when the program tracks the mouse (herdr's own buttons work).
  - The scroll-to-bottom button also appears after wheel-scrolling herdr or tmux.
  - One fixed 48 dp top bar everywhere, with a scroll-edge hairline.
  - The app shell clips every screen to the safe area: nothing draws into the status bar.
  - Sheets stop below the status bar.
- **Added 2026-10-03, after the owner's first QA:** the Idle-edge notification fix; several images at once (picker
  up to 10, share several, one queue per terminal); image upload in 8 SFTP round trips instead of 27; one terminal
  per herdr session; and **the streamline** (contracts.md, "# v0.1.2: streamline"): one UI model (Home holds each
  host's terminals with × to close, a tap opens the picker, `⋯` for host actions; no host screen; the Terminals
  sheet with Copy screen and Gestures & shortcuts; Back = Home), a toolbar that fits (⇧Tab, `/`, `@`; no Panes or
  History), one paste path, the multi-line confirm only without bracketed paste, five Rust bug fixes (tmux scroll
  after a session switch among them), and about 670 lines less across app and core (FFI API 18).
- **Reviews.** Codex reviewed three rounds, plus one time-boxed Fable 5.1 review; a fourth Codex round on the
  2026-10-03 work (`1064526..2743d89`) found nothing. Every finding was fixed with
  a test, apart from the accepted reply race above and the binary SFTP handles noted under Known limits.

### Checks on `main` (last run)

- All pass: `cargo fmt`, `clippy -D warnings`, `cargo test` (the whole workspace, sshd required),
  `gen-herdr-types --check`, `gen-licenses --check`, and the Gradle build, unit tests and lint.
- The device suite passes: 155 tests (2026-10-03, after the streamline). The two notification-posting tests skip, because the phone refuses
  the permission to a test build.

## Fixed: finished turns that herdr reports as Idle never notified (2026-10-03)

The owner's report (2026-10-02 evening: no agent notifications) turned out to be partial. Posting works: the owner
replied from an or2 notification on 2026-10-03. But a finished turn notified only when herdr reported it
**Done**. herdr reports **Idle** instead when the pane counts as *seen*, and a `pane.focus` (or2 opening the pane)
makes it seen, so the usual flow (open the agent from the phone, send, lock the phone) ended in Idle, which the
edge rule ignored. Seen live on the owner's host (herdr 0.9.3): a focused Claude Code pane went `working` → `idle`
with `state_change_seq` advancing by one, twice, and no notification arrived either time.

The fix (Kotlin only): an advance into Idle posts `Done` when the watch saw the pane Working since it last
settled; an Idle not reached from Working still never notifies. The contract ("Agent notifications", "The rule")
and three JVM tests cover it. It still needs the owner's eyes on the phone with a signed build.

**Confirmed on the phone (2026-10-03):** the signed build with the fix notifies for Claude Code and pi panes.

**Reply needs herdr's integration for the agent** (owner QA, 2026-10-03). A notification has Reply only when
herdr reports the agent's `agent_session` (or the agent was started by name), and herdr learns the session from its
integration hook inside the agent (`herdr integration status` lists them). Without the integration, the agent still
notifies (herdr reads its state from the screen) but never gets Reply, with no hint why. On the Linux host:
- `pi` had no integration. Installing it (`herdr integration install pi`) and restarting pi gave it Reply.
- `codex` has its integration, but herdr answers every one of its `pane.report_agent_session` reports with an
  error (in `~/.config/herdr/herdr-server.log`, `outcome="error"`, no reason, every report since 2026-10-02).
  The same request sent by hand afterwards was accepted, so Codex has Reply until it restarts. Not or2's bug:
  report it to herdr upstream with those log lines.
- Proposed (v0.1.3): `or2-pair` checks `herdr integration status` for the agents installed on the host and
  installs the missing ones with one confirmation; the app says why a notification has no Reply.

Found on the way: the JVM test `anImageUploadCrossesTheFfiAndItsPathIsPastedBracketedWithoutEnter` failed on
`main` since `1064526`, because the FFI probe dropped the upload's acknowledgement receiver. The probe now keeps it
until the caller acknowledges, as a real host does.

## Next, in order

1. **The owner's QA of the streamlined v0.1.2 candidate** on the phone (installed 2026-10-03):
   1. Home: each host's terminals inside its card; × closes (herdr/tmux one tap, a shell asks); a tap on the host
      opens the picker (connecting first); `⋯` for Connect/Disconnect, Edit, Delete; `Connect all`.
   2. Picker: the herdr tab lists each session's agents by workspace (a tap opens that agent); the session's row (`default · 4 agents`) opens it whole; tmux
      re-read on every opening, Refresh with a spinner; `● Open` on a session already open.
   3. Terminal: Back and the orange disc go Home; the green disc's Terminals sheet (× per row, Copy screen,
      Gestures & shortcuts); toolbar fits (⇧Tab, `/`, `@`; Copy/Clear while selecting).
   4. Images: several from the picker and from Share; the strip's `Uploading image 2 of 4…`; about 1 s per image.
   5. Multi-line composer text to an agent goes without the "Send N lines?" dialog.
   6. Notifications and Reply (Claude Code, pi; Codex only while herdr accepts its session report).
2. **Fix what the QA finds**, then **release v0.1.2**:
   1. Bump `core/Cargo.toml` to 0.1.2 (then `cargo update --workspace --offline`), the app to
      `versionName 0.1.2` / `versionCode 3`, and the version `NativeContractTest` expects.
   2. Write `docs/releases/v0.1.2.md` and turn "Unreleased" into `[0.1.2]`.
   3. Build and sign, put the SHA-256 in the notes, tag and push, `gh release upload`, then test the
      one-liner. [build](build.md), "Releases", has the details.
3. **The deferred M3 acceptance**, when the owner approves it: mobile data, the Wi-Fi to mobile handover, and
   unplugged (Doze) background runs. This is v0 acceptance step 3, still never tested.
4. **Keyboard (owner idea, 2026-10-03; not decided).** An in-app keyboard in or2's look, with Ctrl, Esc,
   Tab, arrows and the pane, paste and history keys in one layout, replacing the system keyboard, the toolbar and
   the arrow pad. No library does for a keyboard what libghostty does for the terminal, but the parts exist:
   our own Compose layout (Unexpected Keyboard, GPL-3.0-only, as the reference), CleverKeys' on-device glide
   typing (GPL-3.0-only, Kotlin), Android's spell-checker API or AOSP LatinIME's dictionary engine, and
   sherpa-onnx for dictation. Moshi's Android app keeps the system keyboard and polishes a toolbar. Steps: the
   owner tries CleverKeys as the system keyboard for a few days; then a spike under `spikes/` (Compose layout
   and CleverKeys' decoder, in the gallery); then the decision: everywhere, terminal only, or the toolbar pass.
5. **The rest of the roadmap.**
   - Next items: scanning for SSH servers, recent directories, app lock.
   - M4: history sheet, ntfy, dictation, Wake-on-LAN.
   - M5: Chat View, diff viewer, web preview.

## Known limits (documented, not bugs)

- **Reply:** the typed-reply race above. A herdr-started agent replaced by another with the same name in the
  same terminal can't be told apart until it reports a session.
- **Image paste:** the SFTP client is OpenSSH-compatible only (`russh-sftp` decodes handles as UTF-8). SFTP v3
  cannot stop a process of the same account racing the directory checks.
- **tmux:** older than 2.6 gets no session-switching swipe; windows and panes work.
- **Device:** a posted agent notification and real content providers aren't covered by device tests (see
  above). The notification flow needs the owner's eyes.

## Releases

- One release stream, tagged `vX.Y.Z`. The Cargo workspace version, the app's `versionName` and the tag must
  agree; `dist --expect-version` and an xtask test check this.
- Iterations ship as patch releases. A minor version (v0.2.0) is the owner's call.
- The release signing key is in `~/.config/or2/` on the owner's machine, never in the repo.

## Open decisions for the owner

- An HTTPS transport in Rust, or a documented exception, for future web features.
- The scope of Chat View: which agents it supports first.
- Dictation languages.
- Asking herdr upstream for an atomic, agent-bound "type and submit" request, which would close the reply race.
- Whether to rewrite git history to remove real host details pushed before the scrub (recommended: no).

## How the work is run

- The lead writes the contracts, plans and integrates. Opus workers implement lanes in their own git
  worktrees and branches; the lead merges, runs the full checks, pushes, and checks UI in the gallery on the
  phone.
- External review is Codex in the herdr pane next to the lead. Fable 5.1 is used only when the owner approves
  it.
- Device runs use only the separate `io.github.code_akram.or2.devicetest` app, through the ADB tunnel. The
  owner's app is updated only with signed release builds (an in-place update).
- Worker worktrees are removed once merged; the 2026-10-02 cleanup freed about 600 GB.
- [AGENTS.md](../AGENTS.md) has the repository rules.
