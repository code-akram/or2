# Status and next steps

Read this first when picking the work up. The details are in [design](design.md) (checklists),
[contracts](contracts.md) (what the code must do), [roadmap](roadmap.md) and [build](build.md).
Updated 2026-10-02, end of day.

## Where it stands

- **Released: [v0.1.1](releases/v0.1.1.md)** (2026-10-02, after [v0.1.0](releases/v0.1.0.md) the same day): the
  signed APK and `or2-pair` for Linux x86_64/aarch64 (static) and macOS Intel/Apple silicon.
- **On `main`, not released: the v0.1.2 candidate** (`1064526` or later). The app's `versionName` is still
  `0.1.1` (`versionCode 2`); the bump to 0.1.2 happens at release time. The
  [CHANGELOG](../CHANGELOG.md) "Unreleased" section lists everything, and FFI `API_VERSION` is 16.
- **The owner's phone runs that candidate** as the signed release build: an in-place update, so the hosts and
  keys are kept.

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
- **Reviews.** Codex reviewed three rounds, plus one time-boxed Fable 5.1 review. Every finding was fixed with
  a test, apart from the accepted reply race above and the binary SFTP handles noted under Known limits.

### Checks on `main` (last run)

- All pass: `cargo fmt`, `clippy -D warnings`, `cargo test` (the whole workspace, sshd required),
  `gen-herdr-types --check`, `gen-licenses --check`, and the Gradle build, unit tests and lint.
- The device suite passes: 150 tests. The two notification-posting tests skip, because the phone refuses
  the permission to a test build.

## Next, in order (tomorrow morning)

1. **The owner's QA of the v0.1.2 candidate** on the phone:
   1. Reply from a notification. Include a reply to an agent waiting at an approval dialog (Claude Code's
      "Do you want to …?"), and note exactly what a typed reply does there.
   2. Image paste three ways: the composer's image button, the keyboard, and Share → or2. The path appears
      with no Enter, and Claude Code reads the image.
   3. A second image while one uploads shows "An image is already uploading".
   4. Spot-check: Home's card versus its `>_` button, the top bars, the blue arrow pad, and tap-to-click in
      herdr.
2. **Fix what the QA finds**, then **release v0.1.2**:
   1. Bump `core/Cargo.toml` to 0.1.2 (then `cargo update --workspace --offline`), the app to
      `versionName 0.1.2` / `versionCode 3`, and the version `NativeContractTest` expects.
   2. Write `docs/releases/v0.1.2.md` and turn "Unreleased" into `[0.1.2]`.
   3. Build and sign, put the SHA-256 in the notes, tag and push, `gh release upload`, then test the
      one-liner. [build](build.md), "Releases", has the details.
3. **The deferred M3 acceptance**, when the owner approves it: mobile data, the Wi-Fi to mobile handover, and
   unplugged (Doze) background runs. This is v0 acceptance step 3, still never tested.
4. **The rest of the roadmap.**
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
