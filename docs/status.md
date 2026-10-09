# Status and next steps

Read this first when picking the work up. The details are in [design](design.md) (checklists),
[contracts](contracts.md) (what the code must do), [roadmap](roadmap.md) and [build](build.md).
Updated 2026-10-09.

## Where it stands

### Release v0.1.10 (2026-10-10)

- Owner approved the M4 candidate ("looks good") and said proceed: `m4` merged, **v0.1.10 / versionCode 19 / FFI API
  24 / Room 5**. [Release notes](releases/v0.1.10.md).
- Gates: **1,131 Rust**, formatting, Clippy, both generators, `demo-gif --check`; **736 JVM**, all builds incl. the
  signed release (same certificate), both lints, 183 tasks rerun; Linux `dist --expect-version 0.1.10`; **device
  suite `OK (192 tests)`**. Signed APK SHA-256 `d9e272dd93c5fb34be5b89cfff1702bec20f9977f73e9718cf18c7a450cf07ea`
  (40,042,383 bytes); evidence in `/tmp/or2-release-v0.1.10/`. Daily app stayed on the code-18 candidate.

### M4: history, wake and permission answers (2026-10-09, branch `m4`, unreleased)

- Owner asked for M4 items 1 to 3 and deferred dictation, Chat View and the diff viewer; push through a self-hosted
  ntfy (UnifiedPush) was designed with the owner (own server, UnifiedPush into or2, metadata plus the last lines,
  `or2-pair` installing hooks for Claude Code, Codex, pi, opencode v2 and amp) and then **deferred** until the
  mobile-data and Wi-Fi handover tests: while or2 stays connected, its in-app notifications cover the need. A trial
  ntfy user service on this box was removed again; nothing of it remains.
- Built as three lanes in worktrees (contracts, "M4"), merged on `m4` with FFI **API 24**: the history sheet (tmux
  `capture-pane` / herdr `pane.read`, from the Terminals sheet and a mosh chip); Wake-on-LAN, the TCP wake probe on
  every connect, Room v5 (MAC address, wake probe), Keep screen on and Reopen the last terminal; Approve/Deny of
  Claude Code permission prompts from the notification (herdr's detection rule, the seq, "❯ 1. Yes", the foreground
  agent; phone unlock required) and the agent summary in the connection notification, a Live Update while an agent
  needs input. One merge conflict (the fake herdr host's two `pane.read` handlers) resolved with both lanes' tests.
- Checks: **1,131 Rust** (sshd/tmux/mosh/herdr required), formatting, Clippy, both generators, `demo-gif --check`;
  **736 JVM**, debug/device-test builds, both lints; **full device suite `OK (192 tests)`** on the separate app.
  Gallery screens reviewed on the phone (History sheet, Terminals sheet, Settings, host form). Not yet tried for
  real: Wake against the sleeping Mac, a real Claude Code permission prompt answered from the notification (the
  device test app has no notification permission, so the notification tests skip), and the Live Update on
  OxygenOS 16. Daily app unchanged (0.1.9, code 17); test apps removed.
- **Signed M4 candidate installed in place** (owner's request) at 00:22 phone-local time (2026-10-10): **0.1.9 /
  versionCode 18 / FFI API 24**, SHA-256 `8e87b1ddf3cc50cde9517b12dcb3a72ee9894dd99aae774faaabccd054d86ec7`. The
  replaced APK matched public v0.1.9 (kept in `/tmp/or2-candidate-18/`), same certificate, first-install time
  unchanged (Room migrates to v5 on first open), cold launch ok, no crash. Awaiting the owner's QA.

### Flaky header pixel device test fixed (2026-10-09, after v0.1.9; test-only)

- `TerminalVisualDeviceTest.terminalCannotPaintOverComposeHeaderOrOutsideItsSideMargins` failed once in the v0.1.9 and
  once in the v0.1.7 full suites, never alone (0 of 10) or in 14 class runs: the header pixel 92 dp from the top read
  `#19304D` for `#336699`. Ruled out: the activity's enter animation (finished before `ActivityScenario.launch`
  returns, and at every replayed screenshot); our own tests (the classes before it post nothing).
- Cause: the test screenshot the **whole screen** (`UiAutomation`), so any system overlay passing over the header (a
  heads-up notification from another app on the owner's phone, the shade, a scrim) changed its pixels. Shown on the
  phone: with the shade over the fixture the screen read `#3D5D82` while a `PixelCopy` of the app's window still
  held `#336699`. (A heads-up could not be posted to reproduce it exactly: OxygenOS refuses `pm grant` from the shell.)
- Fix: `windowPixels()` copies the probe's own window (`PixelCopy`) for the bounds test, `raster()` (the
  pixel-for-pixel comparisons) and `capture()`; the terminal and the Compose header share that window, so the check
  is unchanged. The README demo still films the screen on purpose. Checks: the bounds test 5 of 5 with the shade
  toggled over it every 400 ms; the visual class 3 of 3; **full device suite `OK (183 tests)`**; device-test build
  and lint. Daily app unchanged (0.1.9, code 17); test apps removed.

### Release v0.1.9 (2026-10-09)

- Owner approved the font candidate ("polished and beautiful") and said proceed: merged, **v0.1.9 / versionCode 17 /
  FFI API 23**, the bundled terminal font below. [Release notes](releases/v0.1.9.md).
- Gates: **1,087 Rust**, formatting, Clippy, both generators, `demo-gif --check`; **684 JVM**, all builds incl. the
  signed release (same certificate), both lints, 183 tasks rerun; Linux `dist --expect-version 0.1.9`. Signed APK
  SHA-256 `8f2ee9ac438249956a9905eb2cd1ad314ded60f1d96c9177c45c2f25a1ab9eb8` (39,419,771 bytes); evidence in
  `/tmp/or2-release-v0.1.9/`.
- Device suite (separate app): 183 cases, 1 failure: `TerminalVisualDeviceTest.terminalCannotPaintOverComposeHeaderOrOutsideItsSideMargins`
  saw the header pixel half as bright (`#19304D` for `#336699`: a passing dim, not a glyph). 10 of 10 alone and its
  class twice then passed; the same check failed once in the v0.1.7 cycle. **Since fixed (test only), above.** The suite on
  the font change itself (before the version bump) passed all 183. Daily app stayed on the code-16 candidate.
- **Published and verified:** [run 37970443500](https://github.com/code-akram/or2/actions/runs/37970443500) built and
  published; the signed APK was uploaded. All six public assets downloaded and checked (checksums, formats, APK hash
  and certificate, APK identical to the local build); public `or2-pair 0.1.9` runs, and the installer from `main`
  installs it into a temporary directory with a matching checksum. Latest release is v0.1.9.
- **Installed in place** (owner's request) at 22:10 phone-local time: 0.1.9 / code 17 from the public download; the
  replaced code-16 candidate matched its recorded hash (kept in `/tmp/or2-release-v0.1.9/`), installed APK matches
  the release hash, first-install time unchanged, cold launch ok, no crash in the log. At the owner's request the
  GitHub release v0.1.8 is now marked pre-release (as v0.1.7); v0.1.9 stays the latest.

### Bundled terminal font (2026-10-09, branch `terminal-font`, unreleased)

- Owner asked for the deferred font step. Coverage was checked first: JetBrains Mono 2.304 alone lacks `⎿`, Claude
  Code's `✻✢✽✳` spinner, `✔✘`, `↳` and `☐☑`. DejaVu Sans Mono 2.37 (monospace, Menlo's ancestor) has all of them but
  `⎿`, which no free monospace font has; it became a sprite (`⎾⎿`). The terminal now uses JetBrains Mono (four real
  faces), then DejaVu Sans Mono, then the system fallbacks (contracts, "Terminal font"). Cells follow the font's line
  spacing (1.32 em, as Ghostty), not its tallest glyph (+14% rows); the probe grid stays 56 x 47.
- Licences: JetBrains Mono OFL-1.1, DejaVu Bitstream Vera (+ Arev; DejaVu changes public domain). Files vendored
  unmodified from pinned release tags with SHA-256 in THIRD_PARTY_NOTICES.md ("Fonts"); their licence files ship
  beside them and the About screen's Vendored list shows them (parser reads `assets/` paths named in the notices).
  **Owner note:** the FSF lists OFL-1.1 as free for fonts; it has no entry for the Bitstream Vera licence, which has
  the same shape (rename modified fonts, do not sell alone) and is in Debian main. Both are data files, not linked code.
- Checks: **684 JVM** (new: asset licence texts, font entries), all builds incl. signed release, both lints,
  `gen-licenses --check`; **full device suite `OK (183 tests)`** (new: every style draws `✻✢✽✳✶✔✘✓✗❯●…⚠↳☐☑·→λ`
  at the cell's advance; italic may add one hinted pixel from DejaVu's synthesized slant, fitted to the cell). The
  sprite capture was reviewed on the phone. APK +736 KB (39,419,771 bytes). Daily app unchanged (0.1.8, code 15);
  test apps removed.
- **Signed candidate installed in place** (owner's request) at 21:43 phone-local time: **0.1.8 / versionCode 16**, SHA-256
  `4e3ac74883cdd99402df65ae51a0c25bf79dc559112dc80e3339caf7d4a37f55`; replaced APK matched public v0.1.8 (kept in
  `/tmp/or2-candidate-16/`), same certificate, first-install time unchanged, cold launch ok, no crash. Awaiting
  the owner's look at a live Claude Code pane.

### Flaky composer device test fixed (2026-10-09, after v0.1.8; test-only)

- Cause, from per-frame traces of the toggle's position, the IME inset and the composer state: the keyboard asked
  for by opening the composer arrives about 330 ms later and slides the toolbar (y 2999 to 1774 px) over some 20
  frames. Compose's test idling does not follow that animation, so a `performClick` between two of its frames pressed
  a key that moved before the release and the tap was lost: `open` lost, then `toggle` opened the composer.
- Fix: `toolbarAtRest()` (the class's bounded `settledBounds` wait: no movement for 400 ms) before toolbar or composer
  taps that follow a composer open/close, in that test and in two others with the same race
  (`thePadAndTheComposerAreNeverOpenTogether`, `theComposerSubmitsTheTextAndTheSendButtonWaitsForText`). No app change.
- Before: 1 failure in the v0.1.8 suite and 1 in 9 runs alone. After: **40 of 40** alone and the class (27 tests)
  3 of 3 on the phone, on the separate device-test app (removed afterwards); daily app unchanged (0.1.8, code 15).
  A person tapping the toolbar during that slide could lose a tap too; judged not worth pinning the toolbar.

### Release v0.1.8 (2026-10-09)

- Owner approved the sprites candidate ("beautiful work") and asked for merge, push and a release:
  **v0.1.8 / versionCode 15 / FFI API 23**, the terminal sprites below. [Release notes](releases/v0.1.8.md).
- Gates: **1,087 Rust** (sshd/tmux/mosh/herdr required), formatting, Clippy, both generators, `demo-gif --check`;
  **683 JVM**, all builds incl. the signed release (same certificate), both lints, 183 tasks rerun; Linux
  `dist --expect-version 0.1.8`. Signed APK SHA-256 `87bf7708aa4323113ba2a0e6eda1ef1cd358c4296c03473cac8537cecf40b6e3`;
  evidence in `/tmp/or2-release-v0.1.8/`.
- Device suite (separate app): 182 cases, 1 failure: `TerminalChromeDeviceTest.closingTheComposerGivesTheKeysBackToTheTerminal`
  (composer still shown after toggling twice). It then failed 1 of 9 runs alone and passed its class twice; the
  composer and its test are unchanged since v0.1.7. Daily app stayed on the code-14 candidate during the tests.
  **Since fixed (test only), below.**
- **Published and verified:** [run 37958819166](https://github.com/code-akram/or2/actions/runs/37958819166) built and
  published; the signed APK was uploaded. All six public assets downloaded and checked (checksums, formats, APK hash
  and certificate, APK identical to the local build); public `or2-pair 0.1.8` runs, and the installer from `main`
  installs it into a temporary directory with a matching checksum. Latest release is v0.1.8.
- **Installed in place** (owner's request) at 20:30 phone-local time: 0.1.8 / code 15 from the public download; the
  replaced code-14 candidate matched its recorded hash (kept in `/tmp/or2-release-v0.1.8/`), installed APK matches
  the release hash, first-install time unchanged, cold launch clean. At the owner's request the GitHub release
  v0.1.7 is now marked pre-release; v0.1.8 stays the latest.

### Terminal sprites: candidate for owner QA (2026-10-09, merged for v0.1.8)

- Owner report with a phone screenshot: Claude Code's mascot showed seams between cells and its permission-mode
  arrows `⏵⏵` were two hollow boxes. Diagnosis from all 224 phone fonts: U+23F5 and Powerline exist in none;
  block elements only in fallback Symbols/CJK fonts whose glyphs do not fill a cell; `⏺` only as a colour emoji.
- Owner chose "draw it ourselves first" over bundling a font: box drawing, block elements, `⏴⏵⏶⏷⏸⏹⏺` and
  Powerline are now sprites (contracts, "Sprites"). `⎿ ✻ ✔` still come from mismatched fallback fonts; a
  bundled terminal font (JetBrains Mono) is the deferred second step.
- Checks: **683 JVM** (8 new sprite geometry tests), debug/device-test lints, and on the phone the **48 terminal
  device tests** (2 new: seam-free mascot and `⏵⏵` pixel checks, a hardware capture reviewed zoomed in).
  Frame timings unchanged. Rust/FFI unchanged, so no Rust gate was rerun. Separate test apps removed afterwards.
- **Signed candidate installed in place** at 19:57 phone-local time: **0.1.7 / versionCode 14 / FFI API 23**,
  SHA-256 `96561ce7850b3f8cd0bc4dde337c37a19cdc319f220f0c1e84daa4fbed09bbbb`. The replaced APK matched public
  v0.1.7 and is kept with the candidate in `/tmp/or2-candidate-14/`; same certificate (`1418c4aa…cbe1a2a2`),
  first-install time unchanged, cold launch `Status: ok`, no crash. No tag or release. Awaiting the owner's
  look at a live Claude Code pane.

### Release v0.1.7 (2026-10-08)

- Owner asked for the GIF on GitHub and a patch release: **v0.1.7 / versionCode 13 / FFI API 23**, the README
  demo and its tooling (below) with no app change besides the version. [Release notes](releases/v0.1.7.md).
- Gates: **1,087 Rust** (sshd/tmux/mosh/herdr required), formatting, Clippy, both generators, `demo-gif --check`;
  **675 JVM**, all builds incl. the signed release (same certificate as v0.1.6), both lints, 183 tasks rerun;
  Linux `dist --expect-version 0.1.7`. Signed APK SHA-256
  `2f342ff1cc50d454ae0f266cb2f162ad9bd85c837ccc42e6c30cc9fd6c8881b5`; evidence in `/tmp/or2-release-v0.1.7/`.
- Device suite (separate app): 180 cases, 176 passed, 3 skipped (demo, two notification-permission), 1 failure: the
  header pixel check of `TerminalVisualDeviceTest` once saw another colour, then passed five reruns. A second
  full run lost its stream at case 166 to a USB link reset (162 passed so far); the remaining 15 passed alone.
  The daily app stayed on 0.1.6 during the tests.
- **Published and verified:** [run 37758561140](https://github.com/code-akram/or2/actions/runs/37758561140) built and
  published; the signed APK was uploaded. All six public assets downloaded and checked (checksums, formats, APK hash
  and certificate); public `or2-pair 0.1.7` runs, and the installer from `main` installs it into a temporary
  directory. The README's GIF is served from `main`. Latest release is v0.1.7.
- **Installed in place** (owner's request) at 13:51 phone-local time: 0.1.7 / code 13; replaced APK matched public v0.1.6
  on device, installed APK matches the release hash, first-install time unchanged, cold launch clean. At the owner's
  request the GitHub release v0.1.6 is now marked pre-release; v0.1.7 stays the latest.
- **Owner confirmed v0.1.7 on the phone:** connecting with the fingerprint unlock for SSH "works fine and is solid".

### README demo GIF (2026-10-08, merged for v0.1.7)

- The README opens with `docs/media/or2-demo.gif` (840 x 740, 31 s at 20 fps, 3.2 MiB): the real app on the
  phone, driven by `ReadmeDemoDeviceTest` through an invented world (`DemoWorld`: hosts `atlas` and `build-box`,
  herdr agents, tmux, shells; no network, keys or real hosts), filmed with `UiAutomation` screenshots and rendered
  by the new `cargo xtask demo-gif` into a card with the six scene captions and touch marks. How to re-record:
  build.md, "README demo". The test is skipped in suite runs (one more skip); `demo-gif --check` joins the gates.
- Only the device-test app was used, and its demo hosts, key and settings were removed after each run; the daily
  app was not touched. `screenrecord` does not work on this OxygenOS 16 build: it runs in its own SELinux domain
  (`screen_record`), cannot write to `/data/local/tmp` or `/sdcard`, and when piped waits forever for
  `hwservicemanager`. The shell cannot signal it, so the two processes left by the first attempts stay until the
  phone restarts (idle, logging a wait line a second).
- Checks: xtask tests and Clippy, formatting, `demo-gif --check`, the device-test builds and their lint. The GIF
  was looked through frame by frame: no status bar, notification or real data in it.
- A full-screen video of the same tour was also filmed by the phone's own recorder (the test's
  `-e or2.demo.frames 0`, cut by `elapsedRealtime`; build.md, "README demo"); the owner chose the GIF for GitHub,
  so the video is not checked in.

### Published v0.1.6 / installed on Android (2026-10-05)

- Owner accepted picker polish (“good job”) and explicitly requested publication plus installation of
  the latest build. Rendering/scrolling and Mac/Arch colour QA were already accepted. **Published
  [v0.1.6](releases/v0.1.6.md) / versionCode 12 / FFI API 23**, with no additional feature changes.
- Fresh release gates pass: **1,080 Rust** (sshd/tmux/mosh/herdr required; no ignored/failures),
  formatting, Clippy with warnings denied and both generators; **675 JVM** (no failures/errors/skips),
  Android builds including signed release and both lints, all 183 Gradle tasks rerun. Linux `dist
  --expect-version 0.1.6` passes for both static/stripped architectures; x86_64 executes, aarch64 format-only.
- Release APK is signed by the existing key, version/metadata verified. SHA-256:
  `31c3266ba0503f17d55dd508dbcb0e6e578a9d678e5d2a238069e89ce609b38f`.
  The code-11 APK is preserved for rollback. Tag `v0.1.6` points to `f50ea84`; [release run 37294065194](https://github.com/code-akram/or2/actions/runs/37294065194)
  passes (Linux/macOS/build/publication). All six public assets downloaded and verified: four binary
  checksums/formats, APK checksum/signature matching the local build and existing signing identity.
  Public latest-release installer matches source and installs `or2-pair 0.1.6` into a temporary directory
  with matching checksum; no existing host install or authorization changes.
- Full device attempt recorded **179 cases: 176 passed, one About/license-screen failure, two
  notification-permission skips**, then timed out while ADB cleanup was unresponsive. Logcat records a
  Home gesture entering the launcher during the license test before the missing-Compose-hierarchy failure.
  Packaged 0.1.6/API 23 and the terminal/picker tests passed. The local
  test launcher was stopped, not the ADB server/bridge; no security or host changes. This first attempt
  is not a passing full gate; the interrupted case and related coverage passed on the restored route below.
- **ADB route incident resolved:** the phone was connected on the owner's Mac, but this agent runs on
  Arch and the SSH-forwarded endpoint had disappeared. A default check automatically started an empty
  runner-local ADB daemon on 5037; its device list was not the Mac's phone. Owner restored a loopback-only
  SSH forward on **5038**. Every subsequent ADB call explicitly supplies endpoint/port/serial, with no
  Mac ADB restart or SSH authorization/security change. The empty runner daemon was not stopped.
- **Restored-route hardware rerun passes: `OK (29 tests)`**, 66 s, no failures/skips. About/license list
  (including the interrupted case), packaged **0.1.6/API 23**, and both picker/Home suites pass. This
  focused rerun is not another full 179-case run. Daily app version/install/update timestamps stayed
  unchanged during tests; only separate device-test packages were installed and subsequently removed.
- **Public release installed in place at 14:25 phone-local time:** **0.1.6 / versionCode 12 / FFI API 23**.
  The prior installed code-11 APK matches its preserved backup; signing identity matches. Installed APK
  SHA-256 matches the local build and public download above; first-install time is unchanged. No daily-app
  uninstall/data clear, host/theme/authorization or security changes. Successful cold launch, app remains
  running, no fresh AndroidRuntime errors. No real-host biometric/terminal interaction was driven after
  the update; owner feature acceptance is on the preceding QA builds. Release and requested installation
  are complete; mobile-data/Doze acceptance remains owner-deferred. Evidence: `/tmp/or2-release-v0.1.6/`.

### Host picker capsule polish (2026-10-05, prerelease QA history)

- Owner requested lowercase `shell` / `dirs` and the shell pill to match the left tab capsule's height.
  The mismatch was the generic pill's 36 dp minimum versus the segmented track's 32 dp. The picker
  now explicitly uses the 32 dp height; shared buttons, shell actions and tabs are otherwise unchanged.
- Hardware regression fails before the fix (32 dp versus 36 dp), then passes with matching height,
  top/bottom alignment and lowercase labels. **22 picker/Home device tests pass**, zero failures/skips;
  **675 JVM**, debug/device-test/signed release builds and both lints pass. Daily app metadata stayed
  unchanged during the separate-app tests. Rust/FFI are unchanged; no fresh Rust gate was needed.
- Signed **0.1.5 / versionCode 11 / FFI API 23** installed in place at **13:23 phone-local time**.
  Prior installed code-10 APK matches its preserved backup; same signing certificate, unchanged
  first-install time, no uninstall/data clear. Installed/build SHA-256 match:
  `6ccc803439c9dcc1203c986088415f1cb968afb9e546cfb0a5fe38ee79e7b024`.
  Cold launch succeeds, no fresh AndroidRuntime errors. App is ready for the owner's visual check;
  no real host/biometric interaction was driven. No tag/public release.

### True-colour capability fix / owner QA (2026-10-05, prerelease QA history)

- **Owner accepted rendering and scrolling:** “rendering and scrolling is solved.” The remaining
  colour papercut was diagnosed read-only before approval: Mac and Arch Pi use the same `arch-ice`
  theme (identical file checksum), Pi 1.0.2, herdr 0.9.3 and matching herdr theme settings. Arch's
  Pi under herdr has `COLORTERM=truecolor`; the Mac's plain or2 Mosh shell had only `TERM=xterm-256color`.
  Pi quantized `#0C2030` to palette index 17. A subprocess-only A/B on the same Mac connection
  reproduced dark blue versus the correct RGB dark teal, with no theme/config changes.
- Owner approved the minimal fix and installation: Mosh bootstrap supplies `COLORTERM=truecolor`
  to the server/child; SSH wraps interactive programs with the same environment, retaining a
  plain shell's login startup through `exec "${SHELL:-/bin/sh}" -l`. The shared Mosh argv plan,
  terminal palette, Kotlin UI, FFI API 23 and stored settings are unchanged; no host files are edited.
- Both new disposable SSH/Mosh tests fail on baseline and pass on the fix: plain/directory shells
  receive the capability and preserve exact RGB. Quoting/login-shell unit tests and existing
  lifecycle/cwd tests pass. The SSH fake fixture now recognizes the env-wrapped login shell;
  the initial all-features run exposed its obsolete shell-request assumption, and the full rerun passes.
- Fresh checks: **1,080 Rust**, formatting, Clippy with warnings denied and both generator checks;
  **675 JVM** (zero failures/errors/skips), Android debug/device-test/**signed release** builds and
  both lints pass. Focused packaged-native/terminal device suite: **13 passed, no failures/skips**;
  daily app version/update time stayed unchanged during it. This is a focused rerun, not another
  full device suite; the preceding full renderer suite remains 176 passed/two permission skips.
- **Owner-authorized signed update installed in place at 13:02 phone-local time (2026-10-05):**
  **0.1.5 / versionCode 10 / FFI API 23**. Certificate matches the previous signed candidate;
  prior installed APK checksum matches the preserved code-9 APK. Installed/build SHA-256 match:
  `e4ca50c21c396db3648d3bb05b7576826bee465755d84660ce3e81b1c2e71ca5`.
  First-install time unchanged, no uninstall/data clear, successful cold launch and no fresh
  AndroidRuntime errors; the daily app is open/running. No biometric unlock or real-host colour
  acceptance was driven after the update. Colour QA needs a **newly opened shell and fresh Pi
  invocation**: existing processes cannot inherit a changed environment. See [manual QA](qa-terminal.md).
  Host themes, authorization, bridge/tunnel and security settings are untouched. No tag/public
  release or deferred connectivity/Doze acceptance. **Owner accepted the colour fix:** “good, its solved.”
  Rendering, scrolling and Mac/Arch colour matching are now owner-accepted.

### Terminal work (2026-10-05, prerelease QA history)

- `main` is ahead of v0.1.5: launcher mark/splash, Tokyo Night terminal colours, batched glyphs,
  cached rows and coalescing-safe moved rows over FFI **API 23** are committed (`649c174` through
  `0c86c04`). Before v0.1.6, the public release was v0.1.5/API 22; the manual-QA candidate included API 23.
- Follow-up **`3457b7b`** is committed and pushed: fixes retained rows returning blank after HWUI
  discards an off-tree RenderNode's display list; keep immutable row Pictures and replay missing lists without
  reshaping cells. Native frame pulls/UniFFI decoding now run on a worker, with a main-thread
  controller permitting only one in-flight read and one pending frame. Detach retains pending/final
  frames, notifications coalesce, and moved-row bases stay ordered.
- All deltas reject sequence gaps, including cell-only/metadata-only frames; the broken-base latch
  clears only on a full snapshot. An unusable take retries the full request, which a retiring display
  may have consumed. The remount test waits for main's resync, not only the worker's mailbox take.
- New coverage: bounded-reader lifecycle/wakeup JVM tests; independent hardware pixel comparisons
  over scrolling/edit/cache eviction; native moved rows against an explicit native full snapshot and
  legacy drawing; a Rust full-snapshot oracle over mixed edits, regions, viewports, modes, colours,
  resizes and slow consumption. Details: contracts, "Scrolling correctness and worker frame ingress".
- Fresh checks pass: **1,076 Rust** tests (sshd/tmux/mosh/herdr required), formatting, Clippy with
  warnings denied, both generated checks; **675 JVM** tests (zero failures/errors/skips), all Android
  builds including **unsigned** release, and debug/device-test lint. Separate-app device report:
  **178 tests, 176 passed, two notification-permission skips, zero failures**. During that separate-app
  suite the daily app's version and install/update timestamps stayed unchanged; test apps were cleaned up.
- **Owner-authorized signed candidate installed in place (2026-10-05, 11:07 phone-local time):**
  **0.1.5 / versionCode 9 / FFI API 23**, from `3457b7b`. The signed release build and refreshed
  debug/device-test builds, 675 JVM tests and both lints pass. Existing APK preserved and checksum-
  verified; candidate and existing signing certificate SHA-256 match (`1418c4aa…cbe1a2a2`). Installed
  APK SHA-256 matches the built candidate:
  `4496cd61c20425214d9e88a4bae9c32c7ee88e2d88ece0ceca855616e96e5f83`.
  First-install time unchanged; no uninstall/data clear, host authorization or security-setting change.
  Launch reports `Status: ok`, the app remains foreground/running and no fresh AndroidRuntime errors
  were found. No biometric unlock or real-host terminal QA was driven in that installation cycle.
  Owner subsequently accepted rendering/scrolling; colour QA is tracked separately above.
- These candidates kept versionName/workspace version 0.1.5 and had **no tag or public release**.
  [Manual owner QA](qa-terminal.md)
  covers live output, scrolling, view lifecycle, resizing and existing features. Previous synthetic
  timing gains are not proof of real-host acceptance; connectivity/Doze acceptance remains owner-deferred.

### Previous published release

- **Released: [v0.1.5](releases/v0.1.5.md)** (2026-10-04, tag `v0.1.5`, commit `d27f725`, versionCode 8,
  FFI API 22): project directories and one-tap shells. The signed APK (SHA-256 `9f35619e…5dff3`) and `or2-pair`
  for Linux x86_64/aarch64 (static) and macOS Intel/Apple silicon are published. All downloaded checksums/formats,
  APK signature and public latest-release installer were verified. The installer ran only in a temporary
  directory, never against host authorization. Publication was explicitly requested by the owner.
- **At publication the owner's phone ran the public v0.1.5 APK**, installed in place at **18:40 phone-local time**,
  no uninstall/data clear. Dirs includes current live herdr cwd ahead of history; both tabs use owned watches.
  The signature matches the existing app; installed/build/public-download SHA-256 matches. First-install time
  is unchanged, hosts remain visible, launch passes and no fresh AndroidRuntime crash was found. Final full
  gates pass below. **Owner QA confirmed after publication (2026-10-04): “it works.”** This records acceptance
  of the directory fix, not a separately observed per-host biometric/`pwd`/close checklist; deferred connectivity
  acceptance stays untouched.
- **Previous LAN upgrade QA (2026-10-04, v0.1.4):** after biometric unlock the phone resumed the same herdr
  terminal with a healthy Mosh badge and fresh output, using its existing key without re-pairing. The owner
  confirmed Spaces works as intended. This was LAN only, not the deferred M3 acceptance; real key unlock and
  a live-host connection were not repeated during the v0.1.5 candidate's automated cycle.
- **Release workflow passed:** [run 37210428881](https://github.com/code-akram/or2/actions/runs/37210428881),
  Linux/macOS builds and publication all successful. Earlier releases: [v0.1.4](releases/v0.1.4.md) (2026-10-04),
  [v0.1.3](releases/v0.1.3.md) and [v0.1.2](releases/v0.1.2.md) (2026-10-03),
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
   - Scanning for SSH servers and the app lock are **deferred by the owner (2026-10-09)**; gestures, hardware
     shortcuts, the OSC 52 clipboard (v0.1.1) and recent directories (v0.1.5) have shipped (roadmap.md).
   - M4 built on `m4` (history sheet, Wake-on-LAN and wake probe, device settings, Approve/Deny, Live Update
     summary); deferred by the owner: dictation, push (ntfy) until the connectivity tests.
   - M5: web preview; Chat View and the diff viewer are deferred by the owner (2026-10-09).

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

**Recent directories: owner QA bug diagnosed/fixed (2026-10-04, revised candidate):**
- The original reader required complete JSON but read only 8 KiB of a Codex header. Real metadata on both
  hosts includes large instructions (about 19–23 KiB), silently losing project paths. Complete large Claude
  entries hit the same parser cap. Realistic regressions failed on the old code and pass on the fix.
- Allow complete Codex headers up to 64 KiB, an extra byte to detect overflow, and a combined 512 KiB Codex
  output budget (fewer than 64 files if large headers exhaust it). Claude lines may use the existing 256 KiB
  tail allowance. Full JSON validation, the five-second timeout, 1 MiB exec cap and unsafe-path rejection stay.
  Worst-case output is tested at 786,446 bytes; no transcript records are used as Codex metadata.
- Read-only diagnostics through the phone's existing Mac connection confirmed Mac-local metadata and 22.7 KiB
  headers. Refresh corrected the initially displayed stale Linux path; subsequent host switches retained
  separate paths. Its original source remains unproven, not assumed to be copied history or a cache race.
- Fresh-context `codex exec` diagnosis and patch reviews completed; the patch has no actionable findings.
  **1,064 Rust**, **651 JVM** (no failures/errors/skips), and **167 device tests** pass, plus formatting,
  Clippy, generated checks, all Android builds and both lints. New real-FFI and device UI regressions cover
  simultaneous distinct hosts/concurrent refreshes and switching between their cached paths.
- Revised v0.1.5/**versionCode 7**, still FFI API 21, installed in place at 14:50 phone-local time. Old and new
  installed APK checksums and matching certificates verified; first-install time unchanged. Test apps removed.
  Launch passes; the subsequent normal biometric reconnect resumed the live Mac herdr terminal over Mosh.
  Opening Arch awaits its separate key's fingerprint. The exact revised reader via a temporary read-only
  LocalHost diagnostic now returns the real Arch repository path; no raw history/prompts printed.
- Nonpublishing Linux/macOS artifact [run 37196896791](https://github.com/code-akram/or2/actions/runs/37196896791)
  passed on `50803fb`. All four downloaded checksums/architectures verified; native runner binaries executed,
  cross-architectures format-checked only, publishing skipped. No public release/tag or host authorization,
  bridge/tunnel or security-setting change. Real-project QA and the deferred connectivity acceptance remain open.

**Dirs now includes live herdr cwd (2026-10-04, revised versionCode 8 / FFI API 22):**
- Owner QA exposed a separate source gap: Dirs read histories only, despite the herdr tab showing live project
  cwd correctly. A new device regression fails on baseline precisely when switching from herdr's live Pi path
  to Dirs. The live Arch snapshot confirms additional agent/pane cwd missing from history.
- The picker now merges its current host's default/named Live views before history: picker agent order with
  pane fallback, then all pane cwd (including plain shells). Changes/removals follow existing watches without
  extra queries/polling/storage; history loading/failure cannot hide valid live rows. With watches disabled,
  discovery remains history-only. Raw paths are never sanitized or inferred from titles.
- Pure API 22 `merge_directory_paths` delegates literal validation/exact dedup/shared 20-path cap to Rust.
  Calls use bounded batches (at most 20 accepted + 32 candidates), with early rejection of definitely overlong
  strings; invalid/duplicate entries cannot crowd later valid paths out of the cap. Current-watch scoping
  returns actual owned Live metadata, never another host's or a retired/stopped/lagging buffered value.
- Fresh-context Codex design/critical patch/fix/scope reviews completed. Bulk copying was bounded and tested;
  multi-agent fallback order is tested. Final patch/scope reviews found no actionable issues.
- Pass: **1,067 Rust**, **661 JVM** (no failures/errors/skips), formatting, Clippy, both generated checks,
  all Android builds and both lints. **Full device rerun: `OK (169 tests)`**, 375 s in the separate test app.
  New cases cover live Pi/plain-shell paths, cwd changes/unavailable/return, ShellIn, cross-host live/history
  separation and packaged FFI validation. One existing composer visibility assertion failed in the initial
  full run, then passed in isolation and in the complete rerun; no unrelated composer change or skipped test.
- Signed v0.1.5/**versionCode 8 / FFI API 22** installed in place at **18:14 phone-local time**. Prior installed
  APK checksum matched preserved code 7; matching certificates and new installed/build checksum verified.
  First-install time unchanged, launch passed, no fresh AndroidRuntime crash; test apps removed only after use.
  No public release/tag or authorization, bridge/tunnel or security change. Post-update real-project QA on both
  hosts and normal biometric reconnect remain pending; deferred connectivity acceptance remains untouched.
- Nonpublishing artifact [run 37208934830](https://github.com/code-akram/or2/actions/runs/37208934830) passed on
  `1095b4c`. All four downloaded Linux/macOS checksums/architectures verified; native runner binaries executed,
  cross-architectures format-checked only; publishing skipped. Implementation and verification records pushed.

**Final v0.1.5 release cycle (2026-10-04, owner authorized):**
- Fresh-context `codex exec` critical review of the complete `v0.1.4..HEAD` release range found no actionable
  bugs. No host/device/credential access by the reviewer; automated checks do not prove manual acceptance.
- **1,067 Rust tests**, requiring sshd/tmux/mosh/herdr; formatting, Clippy with warnings denied and both generator
  checks pass. Gradle reran all tasks: **661 JVM tests** (no failures/errors/skips), all Android builds and lints.
- Full separate-device suite passes **`OK (169 tests)`**, 326 s. Daily app version/update time stayed unchanged
  during tests; the two separate test apps were removed. Permission-dependent notification coverage stays conditional.
- Linux `dist --expect-version 0.1.5` passes, both checksums/static stripped architectures verified, x86_64
  executed (0.1.5), aarch64 format-checked only. Final signed rebuild's SHA-256 is in the release notes; it differs
  from the earlier candidate APK despite unchanged versionCode/source. Signature/package/version checks pass.
- Installed final build in place at 18:40; installed SHA-256 verified on-device, first-install time unchanged,
  hosts visible, successful launch/no fresh AndroidRuntime crash. A large APK pull timed out partially; on-device
  SHA-256 completed instead, without changing the bridge/tunnel. No real-host authentication/authorization change.
- Tagged/pushed **`v0.1.5` at `d27f725`**; tag-triggered [run 37210428881](https://github.com/code-akram/or2/actions/runs/37210428881)
  built Linux/macOS and published successfully. Signed APK uploaded locally; all six public assets downloaded.
  Four host checksums/formats and APK hash/signature verified; public APK is byte-identical to the installed build.
  Linux x86_64/macOS arm64 executed on native runners, other architectures format-checked only. Public latest-release
  installer fetched/compared with source and run into a temporary directory, producing `or2-pair 0.1.5` with a
  matching checksum. No host installation or authorization change. Verification notes pushed; working tree clean.
