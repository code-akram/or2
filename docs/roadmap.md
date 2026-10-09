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

**Shipped from this list:**
- **Gestures** (v0.1.1): one-finger swipe for the next/previous tmux window or herdr tab, two fingers sideways
  for panes and vertically for sessions/workspaces; pinch zooms.
- **Hardware keyboard shortcuts** (v0.1.1): Ctrl+Shift+1..9 switch terminals, W closes, V pastes, C copies,
  Enter opens the composer, / shows the Gestures & shortcuts sheet.
- **Remote clipboard** (v0.1.1): OSC 52 (and OSC 1337 Copy) writes reach the Android clipboard, on by default
  with a Settings switch, size-capped and rate-limited; nothing is ever read back.
- **Recent directories / one-tap shells** (v0.1.5, FFI API 22): the picker's **Dirs** tab merges the live herdr
  agent/pane cwd ahead of bounded Claude Code/Codex histories and opens a new shell there in one tap. A
  tmux-in-directory option is later.

**Deferred by the owner (2026-10-09):**
- **Scan for SSH servers** in "Add host": mDNS (`_ssh._tcp`) and the current subnet's port 22, through
  `Transport`, with a short time limit; it fills the manual form. Local network only (not ZeroTier or remote
  hosts), so it mostly helps with a Mac on the LAN. Small to medium.
- **App lock on resume:** an optional biometric prompt when returning to or2, separate from key unlock (which
  asks only when a key is decrypted to connect). Off by default, with a short grace period for quick app
  switches. Small.

**M4 (already planned, refined):** local agent notifications from herdr events (shipped v0.1.1) and image
paste via SFTP (shipped v0.1.2). Built 2026-10-09 (FFI API 24, contracts "M4"): the history sheet, Wake-on-LAN with
the TCP wake probe, keep-screen-on and reopen-last-terminal settings, Approve/Deny of Claude Code permission prompts
from the notification, and the agent summary as a Live Update. **Deferred by the owner (2026-10-09):** voice
dictation, Chat View, the diff viewer, and push through a self-hosted ntfy (UnifiedPush) until the mobile-data and
Wi-Fi handover tests show whether or2 stays connected in the background.

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
- **File browser / sharing** over SFTP; **themes** (the terminal font, JetBrains Mono under the OFL with DejaVu
  Sans Mono for symbols, is bundled since 2026-10-09); **FIDO2/YubiKey** keys.
- **Usage view** for agents (limits and context left), from what herdr and the agents' own
  status lines expose.

## Research-backed priorities (October 2026)

From the [Moshi study](research/2026-10-moshi-study/README.md): reliability is where Moshi
keeps paying (half of its connection items are fixes), Chat View is its biggest bet, and
multiplexer navigation is second. or2 should support few agents well and never add a cloud.

**Next (small, after M3 lands):**
1. Shipped v0.1.1: local notifications from herdr events: exactly one per Blocked/Done edge
   (`state_change_seq`), none when the pane is on screen, opt-in per host, tap opens the pane.
2. Shipped v0.1.1: wheel-aware scrolling: a swipe in tmux (mouse on) or herdr scrolls the pane, not shell
   history; a scroll-to-bottom button; tap links (including wrapped URLs).
3. Shipped for Reply (v0.1.2, v0.1.3): herdr's `agent_session` ties a notification's Reply and the inbox's
   Enable Reply to the exact agent; Chat View will build on it.
4. From the reference-app pass: recent directories / one-tap shells (v0.1.5), gestures, hardware shortcuts
   and the OSC 52 clipboard (v0.1.1) shipped; scan for SSH servers and the optional app lock are deferred
   by the owner (2026-10-09).

**M4:** image paste over SFTP (shipped v0.1.2; `russh-sftp`, Apache-2.0, OpenSSH-compatible: it reads SFTP handles as UTF-8, so a
server with binary handles fails the upload; EXIF-stripped, downscaled, uploaded to
a private cache dir, path inserted without Enter); notification actions (reply through
`submit_text` shipped v0.1.2; approve/deny with confirmation still open) and an Android 16 Live Update summary; a history
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
