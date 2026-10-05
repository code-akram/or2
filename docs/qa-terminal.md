# Terminal QA regression checklist

Owner-accepted 2026-10-05 on the preceding **signed QA candidate**:
**versionName 0.1.5, versionCode 11, FFI API 23**. Release v0.1.6 is versionCode 12 with the same features. The installation verification and build
checksum are recorded in [status](status.md). The candidate includes the launcher/splash,
Tokyo Night terminal colours, batched glyph drawing, retained rows, moved-row FFI frames,
and off-main native frame decoding. Rendering/scrolling are owner-accepted; this candidate adds
correct true-colour capability signalling to SSH/Mosh launches without changing any theme.
VersionCode 11 additionally aligns the host picker's `shell` pill with its tab capsule and uses
lowercase `shell` / `dirs`; the colour fix was owner-accepted on versionCode 10.

**Owner acceptance (2026-10-05):** rendering/scrolling and the Mac/Arch colour fix are solved.
The following checks are retained for regression reference, not pending acceptance.

## Colour regression checks: matching Mac/Arch colours

1. Unlock/connect normally, then open a **new Mac shell** in or2. Reopening an existing terminal
   does not give its running processes a new environment.
2. Run `printf '%s\n' "$COLORTERM"`; expect **`truecolor`**. The same should hold in a new
   project shell opened from Dirs. Check SSH too if you normally use it, not only Mosh.
3. Start a fresh Pi invocation in that new shell, or resume your session there using your normal
   workflow. Keep the existing `arch-ice` theme; no theme or settings edits are needed.
4. Compare Pi's prompt/message backgrounds, borders and text with Arch. The user-message panel
   should be dark teal (`#0C2030`), not the conspicuous dark blue produced by palette index 17.
5. Check normal typing/output and a short scroll. Existing Pi/herdr/tmux processes may retain
   their original environment; restart only what you choose to refresh, not all agent panes.

The checks below are the earlier renderer checklist, retained for regression reference; the
owner has already confirmed rendering and scrolling are solved.

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
   agent's prompt), optionally run `printf 'plain \033[1mbold\033[0m CJK: 界 emoji: 😀 combining: é\n'` and
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
