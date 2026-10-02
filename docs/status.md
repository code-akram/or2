# Status and next steps

Where or2 stands, and what comes next. Read this first when picking the work up; the details are in
[design](design.md) (checklists), [contracts](contracts.md) (what the code must do) and
[roadmap](roadmap.md). Updated 2026-10-02.

## Where it stands

**Released:** [v0.1.1](releases/v0.1.1.md) on 2026-10-02, after [v0.1.0](releases/v0.1.0.md) the same day.
Each contains the signed APK and `or2-pair` for Linux x86_64/aarch64 (static) and macOS Intel/Apple silicon.
v0.1.0 covers M1 to M3 and Easy pair v2; v0.1.1 adds instant opens (SSH first, mosh swapped in behind),
agent notifications, wheel-aware scrolling, tap links and OSC 52, gestures and shortcuts, the redesigned
terminal top and Home cards, and `or2-pair` on a rail. The notes list what is still untested.

**Verified on the owner's phone** (OnePlus 10 Pro, Android 16, Wi-Fi, plugged in):
- M2 acceptance.
- M3: background survival, airplane-mode roaming, process-kill reattach, and the re-check timings.
- Easy pair v2 against a host reached over its public address: paired, saved with its host key
  trusted, and connected.
- Easy pair v2 with a Mac on the LAN, using the macOS `or2-pair` from the one-liner: paired smoothly and
  connected with no host-key prompt.
- The v0.1.0 release APK: installed on the phone in place of the debug build (SHA-256 checked); it ran the
  Mac pairing above.
  A debug build cannot be installed over it (a different signing key): try new builds as the
  `.devicetest` app or as signed release builds, or uninstall first (losing the hosts and keys).
- v0.1.1: installed over v0.1.0 (same key, hosts kept); instant opens and the switch to mosh on the Mac
  (UDP verified both ways with captures; an earlier failure was a stale per-connection verdict, now
  rechecked after five minutes); the new top bar, arrow pad and picker.
- The device suite passes (137, with the two notification-posting tests skipped: the phone refuses the
  permission to a test build). It runs as the separate `io.github.code_akram.or2.devicetest` app and never
  touches the daily app.

**Not yet verified:**
- **The M3 tests the owner deferred:** mobile data, Wi-Fi to mobile handover, and unplugged (Doze)
  background runs. These cover v0 acceptance step 3.
- **A posted agent notification on the phone,** end to end (tests cover it up to posting).
- **The v0.1.1 Home cards** (card versus `>_` button), seen in the gallery and the device suite only.

## Next, in order

1. **The owner's QA of v0.1.1** and any patch it needs (v0.1.2).
2. **The deferred M3 acceptance,** when the owner approves it: mobile data, handover, Doze.
3. **The rest of the roadmap's "Next" items:** scanning for SSH servers, recent directories, app lock, and
   projecting herdr's `agent_session` into the inbox.
4. **M4:** image paste over SFTP, notification actions, history sheet, ntfy, dictation (BYOK or
   on-device), and Wake-on-LAN with a TCP wake probe and keep-screen-on. **M5:** Chat View, diff viewer,
   web preview.

## Releases

- One release stream, with tags `vX.Y.Z`. The Cargo workspace version, the Android `versionName` and
  the tag must agree; `dist --expect-version` and an xtask test check this.
- Iterations ship as patch releases (v0.1.1, v0.1.2, …). A minor version is the owner's call.
- To cut a release, follow [build](build.md):
  1. Bump the versions.
  2. Write `docs/releases/vX.Y.Z.md` and the CHANGELOG entry.
  3. Build and sign the APK locally (the key lives outside the repo, in the owner's
     `~/.config/or2/`).
  4. Put its SHA-256 in the notes.
  5. Push `main` and the tag. CI builds the `or2-pair` binaries and creates the release.
  6. Upload the APK with `gh release upload`.
  7. Test the one-liner.

## Open decisions for the owner

- An HTTPS transport in Rust, or a documented exception, for future web features.
- The scope of Chat View: which agents it supports first.
- Running `tmux set mouse on` on hosts, with consent.
- Dictation languages.
- Whether to rewrite git history to remove real host details pushed before the scrub (recommended: no).
- Cleaning up old worker worktrees under `.claude/worktrees/`: about 100 GB, kept until the owner
  decides.

## How the work is run

- The lead plans and integrates; Opus workers implement lanes in their own git worktrees.
- At important checkpoints, an external Codex review and an internal Fable review run in parallel,
  and every finding gets a fix and a test.
- Device runs use only the `.devicetest` app, through the ADB tunnel.
- [AGENTS.md](../AGENTS.md) has the repository rules.
