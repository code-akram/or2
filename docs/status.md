# Status and next steps

Read this first when picking the work up. The details are in [design](design.md) (checklists),
[contracts](contracts.md) (what the code must do), [roadmap](roadmap.md) and [build](build.md).
Updated 2026-10-04.

## Where it stands

- **Released: [v0.1.4](releases/v0.1.4.md)** (2026-10-04, tag `v0.1.4`, commit `21c2e35`, versionCode 5,
  FFI API 20): Spaces and herdr-style agent names. The signed APK (SHA-256 `5586e541…547e`) and `or2-pair` for
  Linux x86_64/aarch64 (static) and macOS Intel/Apple silicon are published. All downloads' checksums and
  formats, the APK signature, and the public latest-release installer were verified. The installer was run
  only into a temporary directory, never against host authorization.
- **The owner's phone now runs the signed v0.1.5 candidate**, installed in place with the owner's explicit
  approval on 2026-10-04 at 12:54 (versionCode 6, FFI API 21; no uninstall or data clear). The signing certificate
  matches installed v0.1.4; the installed APK's full SHA-256 matches the candidate (`d09ad503…ac633`), first-install
  time stayed unchanged, the existing hosts remain visible, and launch passed with no fresh AndroidRuntime crash.
  This candidate is **not publicly released**; see [candidate notes](releases/v0.1.5.md).
- **Previous LAN upgrade QA (2026-10-04, v0.1.4):** after biometric unlock the phone resumed the same herdr
  terminal with a healthy Mosh badge and fresh output, using its existing key without re-pairing. The owner
  confirmed Spaces works as intended. This was LAN only, not the deferred M3 acceptance; real key unlock and
  a live-host connection were not repeated during the v0.1.5 candidate's automated cycle.
- **Release workflow passed:** [run 37177173798](https://github.com/code-akram/or2/actions/runs/37177173798).
  Earlier releases: [v0.1.3](releases/v0.1.3.md) and [v0.1.2](releases/v0.1.2.md) (2026-10-03),
  [v0.1.1](releases/v0.1.1.md) and [v0.1.0](releases/v0.1.0.md) (2026-10-02).

### In v0.1.2

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

### Checks on the v0.1.4 preparation (2026-10-04)

- Pass: `cargo fmt`, `clippy -D warnings`, the whole Rust workspace (1,041 tests at default parallelism;
  sshd, tmux, mosh and herdr required), `gen-herdr-types --check` and `gen-licenses --check`.
- Pass: Gradle debug/device-test/unsigned-release builds, 645 JVM tests and debug/device-test lint. The signed
  release APK also builds; its signature, versionName 0.1.4 and versionCode 5 were verified.
- Device suite: `OK (164 tests)` through the existing ADB tunnel, on the separate device-test app; the daily
  app's version and update time stayed unchanged. Notification-posting tests remain permission-dependent.
- Linux `dist --expect-version 0.1.4` passes: both static, stripped binaries built and checksummed; x86_64
  runs, aarch64 is format-checked only. The release workflow built both macOS binaries; the downloaded Mach-O
  formats and all four host binaries' checksums were verified (macOS binaries not run on Linux).
- The ADB tunnel stopped responding after the successful device suite, during an optional notification-only
  recheck. That recheck produced no result; no bridge, tunnel or phone-security settings were changed. The
  connection was rechecked and working again before the authorised release update.

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

1. **The deferred M3 acceptance**, deferred by the owner (2026-10-03) until they clear it: mobile data, the Wi-Fi to mobile handover, and
   unplugged (Doze) background runs. This is v0 acceptance step 3, still never tested.
2. **The rest of the roadmap.**
   - Next items: scanning for SSH servers, app lock. Recent directories is implemented, reviewed and installed
     on the phone below; the v0.1.5 candidate is not publicly released. Owner QA on real projects remains.
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

The in-app keyboard idea (2026-10-03) was dropped by the owner the same day.

**Connectivity work deferred by the owner (2026-10-04):** DHCP-resistant LAN discovery/hostnames and an optional
user-owned SSH relay for access across networks; no or2-hosted service. A changed LAN address was diagnosed and
corrected through the app, using the existing key without re-pairing. This is not a completed discovery feature.

**Mosh fixture hardened (2026-10-04):** port-owner lookup is restricted to the fixture's IPv4 loopback address
(a real server on another interface can use the same port), with a regression test. The fork-handover wait now
requires exactly the reported pid, not just at most one owner; three standalone runs and the full gate pass.

**Intermittent tests investigated and fixed (2026-10-04, unreleased):**
- **Pairing locks:** the original `a_state_file_no_run_holds_is_expired` failure reproduced on baseline full-suite
  run 8. Closing a guard's descriptor alone leaves its `flock` alive if a parallel spawn inherited the same
  open file description before exec. `Held` and `Liveness` now explicitly unlock on drop (retrying EINTR).
  Two deterministic duplicate-descriptor regressions failed before the fix and pass after it; they model the
  same sharing as fork without introducing another fork race. The old 30 s test workaround is removed.
- **SFTP close observation:** the original `the_kept_session_closes_its_channel_when_dropped` timeout did not
  recur in 12 SSH-suite and 8 full-core baseline runs. A deterministic EOF-before-Close regression reproduced
  the missing callback: the fixture's `into_stream` drop sent a server-initiated Close first, so russh removed
  the channel and ignored the client's later Close instead of invoking `channel_close`. The fixture now uses
  passive reader/writer adapters; the regression and the original test pass. No app-side SFTP change.
- **Verification:** 20 consecutive full pairing-unit suites (200 tests each), 12 full-core unit suites (589
  each), plus the full all-features workspace gate (1,044 tests; sshd/tmux/mosh/herdr required), formatting and
  Clippy with warnings denied pass. The CLI SIGKILL/expired-pairing regression passes too. Only isolated Rust
  fixtures were used; the owner's app, authorization and deferred connectivity acceptance were untouched.

**Recent directories implemented (2026-10-04, unreleased):**
- The owner's go-ahead followed committing the pairing/SFTP fixes (`a83f3b0`). The host picker's **Dirs** tab
  reads bounded Claude Code/Codex histories over the existing SSH connection and opens a new shell in a selected
  project with the host's transport preference. Read-only, no daemon or Room migration; FFI API 21. Details and v1
  format/size limits are in contracts, "Recent directories / one-tap shells". A tmux-in-directory option is not v1.
- Deterministic regressions cover metadata parsing, limits and malformed inputs, unsafe/injected-looking paths,
  newline file names, stale directories, timeout/cancellation, retired connections, shell transport/close/reuse
  rules, restoration and the real FFI. Disposable SSH and mosh hosts verify the actual working directory.
- **Full cycle, explicitly requested by the owner:** Codex reviewed `0ae18cc..f754316` and the follow-up
  version/fixture diff, with no actionable findings. Versions advance together to **0.1.5 / versionCode 6**
  (FFI API 21); the JVM sshd fixture also pins agent config roots under its temporary home.
- Pass: all-features Rust workspace **1,060 tests** with sshd/tmux/mosh/herdr required, formatting, Clippy with
  warnings denied, both generated-file checks; Android debug/device-test/**signed release** builds, **650 JVM
  tests** (zero failures/errors/skips), and debug/device-test lint. Linux `dist --expect-version 0.1.5` passes:
  static, stripped x86_64/aarch64 formats and checksums verified; x86_64 runs, aarch64 is format-checked only.
- **Cross-platform CI passed:** the nonpublishing release [run 37190954376](https://github.com/code-akram/or2/actions/runs/37190954376)
  on `053b403` built all four Linux/macOS artifacts. Downloads' checksums and ELF/Mach-O architectures verified;
  x86_64 Linux and Apple-silicon macOS ran and reported 0.1.5 on their runners, the other architectures not run.
  The publish job was skipped. Implementation and candidate preparation are pushed to `main`.
- **Device suite passed: `OK (166 tests)`**, on the separate device-test app, in 272 s through the existing ADB
  tunnel. The daily app's version and update time stayed unchanged during the suite. Notification tests remain
  permission-dependent. `picker-directories` was rendered and visually inspected on the phone with synthetic
  paths; its labels, tabs, Shell pill, paths and Refresh fit correctly.
- **Installed the signed candidate in place** only after the device suite passed. Its certificate matches the
  previously installed published v0.1.4 APK (which was pulled and checksum-verified); the phone's new APK hash
  matches the built artifact. v0.1.5/versionCode 6, unchanged first-install time, saved hosts, successful launch
  and no fresh AndroidRuntime crash were verified. No real-host connection or biometric key unlock was driven.
  The test APKs were removed afterwards; the daily app remains installed and open.
- One initial workspace invocation was backgrounded, so its children inherited ignored SIGINT and the existing
  pairing CLI Ctrl-C test waited indefinitely. Only that disposable fixture child was killed; the foreground
  workspace rerun passed. This was a test-launch issue, not a new pairing change.
- No public release/tag, real-host authorization change, ADB bridge/tunnel or security-setting change.
  Connectivity and mobile-data/Doze acceptance remain owner-deferred; real-project owner QA is not claimed.
