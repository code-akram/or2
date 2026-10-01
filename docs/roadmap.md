# or2 roadmap notes

Product direction beyond the milestone checklists in [design](design.md). Sources: the owner's
own use, and a study (2026-10-01) of the Moshi Android app, the owner's reference for polish
(settings, help, "Discover" tour and Pro paywall; no screenshots or account data are kept in the
repository).

## How Moshi is built, and where or2 deliberately differs

| Moshi | or2 |
|---|---|
| `moshi-hook` daemon on every host (brew/script install, paired with a device token through Moshi's cloud) feeds the inbox, usage, context detection, diff and web preview | **No daemon.** herdr already exposes agent state over its socket; or2 reads it over the SSH connection (streamlocal). Anything that needs more uses plain exec. |
| Push notifications and iOS Live Activities through its servers | **No FCM, no vendor cloud.** Local notifications from live herdr events while the foreground service runs; UnifiedPush/ntfy only as an opt-in later. |
| Paid "Pro" tier (around ₹10,100/year): multiplexer sessions, auto-attach, sidebar, image paste, diff, web preview, mosh/ET, file sharing, dictation, themes, fonts; free tier capped at 2 connections | **GPL, free, F-Droid-clean.** Everything is in. |
| Credentials can sync through a user-chosen folder | Keys are Keystore-bound and never leave the phone; a future export, if any, is encrypted and explicit. |

What or2 already matches: tmux and herdr session pickers, auto-attach after reconnect, mosh,
a herdr agent inbox, composer/quick replies, multi-address hosts, compact UI, and (soon) QR pairing.

## Ideas worth taking, by priority

**Next (small, high value):**
- **Scan for SSH servers** in "Add host": mDNS (`_ssh._tcp`) and the current subnet's port 22,
  through `Transport`, with a short time limit; it fills the manual form.
- **Recent directories / one-tap shells:** on connect, list recent working directories from the
  host's Claude Code and Codex histories (exec, no daemon) and open a shell or a tmux session
  there in one tap.
- **Gestures:** swipe for the next tmux window or herdr tab, two-finger swipe for panes,
  two-finger vertical swipe for sessions/workspaces; pinch already zooms.
- **Hardware keyboard shortcuts:** show shortcuts, switch session 1–9, close session, paste,
  open composer.
- **Remote clipboard:** honour OSC 52 from the host into the Android clipboard (with a per-host
  opt-in), so copying inside tmux/herdr reaches the phone.
- **App lock on resume** (optional biometric when returning to or2), separate from key unlock.

**M4 (already planned, refined):** local agent notifications from herdr events; image paste via
SFTP; voice dictation with an on-device or bring-your-own-key engine (no Google services);
Wake-on-LAN, TCP wake probe and keep-screen-on (see design M4 backlog).

**M5 candidates (bigger):**
- **Chat view** of an agent pane: render the agent's conversation natively from herdr's
  `agent.read`/`pane.read` instead of the TUI, with the composer as the reply box. Moshi treats
  this as a headline feature.
- **Scrollback in mosh:** mosh only syncs the screen, so or2 should page history from the
  target instead (herdr `pane.read` or `tmux capture-pane`) when the user scrolls up in a mosh
  terminal.
- **Diff viewer** (`git diff` over exec, rendered natively) and **web preview** (an SSH port
  forward to a WebView).
- **zellij** sessions alongside tmux and herdr; **Eternal Terminal** as a transport (evaluate
  against mosh first).
- **File browser / sharing** over SFTP; **themes and bundled fonts** (JetBrains Mono under the
  OFL); **FIDO2/YubiKey** keys.
- **Usage view** for agents (limits and context left), from what herdr and the agents' own
  status lines expose.

## Research-backed priorities (October 2026)

From the [Moshi study](research/2026-10-moshi-study/README.md): reliability is where Moshi
keeps paying (half of its connection items are fixes), Chat View is its biggest bet, and
multiplexer navigation is second. or2 should support few agents well and never add a cloud.

**Next (small, after M3 lands):**
1. Local notifications from herdr events: exactly one per Blocked/Done edge
   (`state_change_seq`), none when the pane is on screen, opt-in per host, tap opens the pane.
2. Wheel-aware scrolling: a swipe in tmux (mouse on) or herdr scrolls the pane, not shell
   history; a scroll-to-bottom button; tap links (including wrapped URLs).
3. Project herdr's `agent_session` into the inbox model (unlocks Chat View and precise tap
   routing).
4. From the reference-app pass: scan for SSH servers, recent directories / one-tap shells,
   gestures, hardware shortcuts, OSC 52 clipboard, optional app lock.

**M4:** image paste over SFTP (`russh-sftp`, Apache-2.0; EXIF-stripped, downscaled, uploaded to
a private cache dir, path inserted without Enter); notification actions (reply through
`submit_text`, approve/deny with confirmation) and an Android 16 Live Update summary; a history
sheet that pages tmux/herdr history under mosh; an ntfy hook snippet as opt-in remote push
(metadata-only payload by default); bring-your-own-key dictation; Wake-on-LAN, TCP wake probe,
keep-screen-on.

**M5:** Chat View v1 for Claude Code then Codex (transcripts read over exec, located from
herdr's `agent_session`), approval cards, diff viewer (git over exec), web preview (SSH
`direct-tcpip` into a WebView), UnifiedPush connector.

**Later / maybe:** on-device dictation (sherpa-onnx + Moonshine/Parakeet; F-Droid needs
from-source ONNX builds), zellij (≥ 0.44), tmux control mode, file browser, FIDO2/YubiKey,
Eternal Terminal only if UDP is blocked somewhere the owner works.

**Decisions needed** (see the synthesis's open questions): whether a Rust HTTPS transport is
acceptable (BYOK dictation, UnifiedPush) or an explicit exception; Chat View agent scope;
whether or2 may run `tmux set mouse on` with consent; dictation languages.
