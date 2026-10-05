# Manual owner QA: terminal rendering candidate

Prepared 2026-10-05. This is an **unreleased signed candidate**, not a public release:
**versionName 0.1.5, versionCode 9, FFI API 23**. The installation verification and build
checksum are recorded in [status](status.md). The candidate includes the launcher/splash,
Tokyo Night terminal colours, batched glyph drawing, retained rows, moved-row FFI frames,
and off-main native frame decoding.

## Before testing

- Open the normal **or2** app, not the separate device-test app. Saved hosts, keys and settings
  should remain present; no uninstall, data clear or re-pairing is expected.
- Unlock your existing key with the normal biometric prompt and connect to an existing host.
  If an unchanged host unexpectedly asks for new trust or pairing, stop and report it rather
  than approving blindly.
- Use your usual LAN connection. Mobile-data handover and unplugged/Doze acceptance are still
  owner-deferred and are **not** required for this renderer QA.

## Checklist

1. **Live terminal output.** Open an existing herdr agent or tmux terminal, type a normal short
   message/command, and watch fresh output. Check for blank/stale/duplicated rows, missing bold
   text, flicker, colour changes and delayed input while output is arriving.
2. **Scrolling in both directions.** Scroll enough to move rows out of view and back, including
   repeated direction changes and a fling. For herdr/tmux, use their own history; for a plain
   SSH shell, use or2's local scrollback. Mosh's limited local history is unchanged. Return to
   the bottom and confirm fresh output appears correctly.
3. **Rich text.** Inspect your usual coloured/bold agent output. In a disposable shell (not an
   agent's prompt), optionally run `printf 'plain bold? CJK: 界 emoji: 😀 combining: é\n'` and
   `seq 1 200`; check glyph placement and that numbered lines remain in order when scrolling.
4. **View lifecycle.** Go Home, reopen the same terminal, switch to another open terminal and
   back, then briefly background/foreground the app. The current screen should return without
   blank rows or being replaced by another terminal's content.
5. **Size and overlays.** Show/hide the keyboard, rotate if you normally do, and pinch the font
   size up/down. Try selection and Copy, then clear the selection. Check clipping, cursor,
   decorations, selection alignment and correct redraw after resizing.
6. **Existing features.** Confirm the terminal switcher, Spaces (herdr), and Dirs still open the
   expected host/session/project. Compare scrolling/input responsiveness with your usual build.

## Reporting

Record which transport and target you used (SSH/Mosh; shell/tmux/herdr), the steps that exposed
an issue, and whether reopening or resizing repairs it. A screenshot or short recording helps;
avoid exposing private terminal content. Automated fixture checks passing is not manual owner
acceptance. Do not mark the deferred connectivity/Doze checklist complete from this QA.
