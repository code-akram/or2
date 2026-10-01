# Moshi feature timeline analysis (v3.0.0 to v3.15.3) and or2 gap matrix

## Method and caveat
I parsed the per-version files in `wn/`, which carry Moshi's own NEW/ENHANCED/FIXED tags. That gave 254 items across 46 releases: 102 NEW, 98 ENHANCED, 54 FIXED. I assigned each item to an area by keyword rules plus manual overrides. Boundaries are fuzzy, for example a "Mosh over Herdr theme" item could sit in either of two areas. Treat the counts as ±3. The notes are curated, so FIXED is understated. 82 items (32%) are tagged PRO.

## Investment by area

| Area | Items | NEW / ENH / FIX | Reading |
|---|---|---|---|
| Chat View (agent chat UI) | 54 | 20 / 26 / 8 | The biggest bet. Present in 25 of 46 releases since v3.6.0. 44 of its 54 items are PRO. |
| Multiplexer nav and Sidebar | 40 | 21 / 16 / 3 | tmux, herdr and Zellij navigation: Jump To, swipe-to-switch, deep links, Git status. |
| Input, toolbar, gestures | 29 | 14 / 5 / 10 | Heavy ongoing polish. |
| Connection reliability (Mosh/ET/SSH) | 24 | 4 / 8 / 12 | Half of all items are fixes. This is the most fragile area. |
| Notifications, hooks, Live Update | 22 | 8 / 11 / 3 | |
| Themes, fonts, terminal fidelity | 19 | 7 / 5 / 7 | |
| Platform, onboarding, sync, licensing | 17 | 5 / 9 / 3 | |
| Home, Switcher, Inbox/Agents | 16 | 4 / 9 / 3 | The session switcher was rewritten at v3.6.3 and again at v3.12.0. |
| Web preview, diff, files, images | 14 | 8 / 3 / 3 | |
| Dictation | 10 | 8 / 2 / 0 | |
| Keys and auth | 6 | 3 / 2 / 1 | YubiKey, key library. |

Fixes cluster in two places. Connection reliability has 12 of its 24 items as fixes, and Chat View has 8 fixes, several of them reconnect or state bugs. Mosh/ET recovery after backgrounding appears in about 19 releases. The recurring failure modes are:
- Restoring state after the app is killed or minimized.
- A garbled screen on session reuse.
- Network switches.
- ET and Mosh restore failures.

## What users value (inferred)
1. **Never losing the session.** This is the repeated reliability work and the reason Mosh, ET, auto-attach and "reopen terminal on launch" exist.
2. **Agent triage from the phone.** Inbox, push, Live Activities, usage windows, and answering questions or plans in Chat View. Moshi's 14 supported chat agents show that users run many agents, not just Claude Code.
3. **Fast navigation across tmux, herdr and Zellij.** This has been the second-largest investment since v3.0.
4. **Typing efficiency on glass.** Zen ring, D-pad, chords, volume-button modifiers, dictation.
5. **Seeing results.** Diff, web preview, images.

The Pro paywall gates multiplexers, auto-attach, Mosh and ET, the sidebar, image paste, diff, preview and BYOK dictation. That tells you which of these people will pay for.

## Sequence in which capabilities appeared
- **v3.0 to v3.2 (utility base):** diff viewer, web preview, herdr support (3.1), unified push and a scroll-to-bottom button for tmux scrollback (3.2).
- **v3.2.2 to v3.5 (input and transports):** local Parakeet dictation (3.2.2), mouse mode (3.3), ET protocol (3.5).
- **v3.6 to v3.10 (Chat View era):**
  - Chat View appears at 3.6.0 and Codex at 3.6.1.
  - Session switcher rewrite at 3.6.3; image paste via hook at 3.7.
  - YubiKey at 3.8.
  - Answering questions in chat at 3.9.
  - Kitty images and Chat View on by default at 3.10 (default-on at 3.12.4).
- **v3.11 to v3.12 (hub):** usage bars, host web servers and simulators from Home (3.11). Home becomes the session switcher (3.12).
- **v3.13 to v3.15 (breadth):** Windows and WSL hosts, BYOK dictation, composer autocomplete (3.13). Hook install from Home, long-press menus (3.14). Zen mode, tablet UI, key library (3.15).

The pattern is mechanism first, then agent awareness, then chrome. Moshi added herdr at v3.1 and made it central at v3.3.

## Gap matrix (or2 status from `docs/design.md` and `docs/contracts.md`; no `docs/roadmap.md` exists)

| Area | Moshi capability | or2 status | Priority |
|---|---|---|---|
| Connection | Mosh | has (M3-A Rust and FFI done; app wiring is M3-B) | P0 |
| Connection | Roaming, background survival, auto-reattach | planned (M3-B service, network callback, "Resume" card) | P0 |
| Connection | ET protocol | missing | P3 (Mosh covers it; ET needs a server install) |
| Connection | Mosh wake probe | missing | P3 |
| Multiplexer | tmux picker | has | – |
| Multiplexer | herdr workspaces and tabs | has (focus, inbox) | – |
| Multiplexer | Swipe-to-switch panes | missing | P2 |
| Multiplexer | Sidebar with Git status | missing | P2 |
| Multiplexer | Zellij | missing | P3 |
| Multiplexer | tmux control mode | planned ("Later") | P2 |
| Agents | Inbox across hosts, blocked first | has | – |
| Agents | Quick reply (composer, `submit_text`) | has | – |
| Agents | Usage windows and reset alerts | missing | P3 |
| Agents | Chat View | planned ("Later") | P1 (see below) |
| Notifications | Local, from herdr events | planned (M4) | P0 |
| Notifications | Push from hooks | missing | P3 |
| Input | Toolbar, arrow pad, composer | has | – |
| Input | Gestures, Zen ring, volume modifiers, chords | partial | P2 |
| Input | Terminal link tap, multi-line link wrap | missing | P1 |
| Input | Mouse mode, right-click arming | missing (herdr is mouse-aware) | P1 |
| Voice | Dictation | planned (M4) | P2 |
| Content | Image paste | planned (M4, SFTP) | P1 |
| Content | Diff viewer, web preview | planned ("Later") | P3 |
| Terminal | Themes, fonts, custom palettes | partial (Catppuccin only; no font choice) | P2 |
| Terminal | Kitty graphics, OSC 52 clipboard | missing | P2 |
| Keys | YubiKey, FIDO2 keys | planned ("Later") | P3 |
| Keys | Key library, biometric per use | has | – |
| Platform | Easy Pair (QR), host scan | missing | P3 |
| Platform | Tablet layout, i18n, sync | missing | P4 |

## Recommended approach for or2
1. **Finish M3-B (effort M, highest impact).** Moshi spent roughly 12 fix items on exactly this. Test these cases:
   - Background, minimize, force-kill.
   - Reused sessions arriving garbled until resize. Moshi fixed this at v3.14.2; or2's full-frame and viewport handling should be checked for the same bug.
   - Network switches.
   - A saved connection that fails to restore.
2. **Local notifications from herdr events (effort S, high impact).** The service already owns the herdr watch. Notify on Blocked and Done. This delivers v0 step 2 without any host daemon. Do not copy Moshi's cloud device-token model, because it conflicts with the "no vendor cloud" rule. Optional later work is UnifiedPush or ntfy.
3. **Tap links and mouse mode (effort S to M, medium impact).** Link detection and tap handling are cheap. Mouse-mode passthrough is needed for herdr and tmux selection.
4. **Image paste over SFTP (effort M, high impact).** Add `russh-sftp` (Apache-2.0, GPL-3.0-compatible; Moshi uses it too) and reuse the active SSH connection. Paste the remote path into the agent prompt. Moshi found this reuse speeds uploads and cuts memory.
5. **Chat View later, in minimal form (effort L, highest differentiator).** Moshi's data shows it is the center of gravity. It reads the agent's output from the host, with no cloud. or2 has no host daemon, so any transcript source (for example reading agent log files over SSH) must be decided in a design pass before building. Build a simple "last agent reply as card, approve or deny, answer a question" view first. Avoid a per-agent adapter matrix: Moshi added about 14 agents over 12 releases and paid 8 fixes for it.
6. **Dictation (effort M).** Prefer an on-device engine with a permissive licence: whisper.cpp (MIT) or sherpa-onnx (Apache-2.0). Both are GPL-3.0-compatible. Check F-Droid reproducibility of bundled models, and whether any model weights carry their own licence terms. Moshi's `parakeet.cpp` is MIT, but model licences differ.
7. **Do not build yet:** ET, Zellij, tablet layout, iCloud-style sync, Apple Watch. They are low value for a two-host, one-owner tool.

## Risks
- Chat View scope creep: it is 21% of Moshi's items, and most of that is per-agent parsing that breaks when agent TUIs change.
- Reconnect work will keep finding bugs on OxygenOS because of aggressive process killing. Budget for repeated phone acceptance runs.
- Moshi is iOS-first (Live Activities, Dynamic Island, iCloud, Watch), so part of its timeline does not transfer to Android.
- The keyword classification has some fuzzy boundaries (see caveat above).
- Dictation, models and any bundled assets need a licence check before F-Droid submission. All the libraries named above, including russh-sftp, whisper.cpp and sherpa-onnx, have not been re-verified here beyond Moshi's `60-licenses.txt` entries (russh-sftp Apache-2.0, parakeet.cpp MIT, whisper.rn MIT).

## Sources
- `/home/akram/code/or2/.amp/in/moshi/63-whatsnew-all.txt`
- `/home/akram/code/or2/.amp/in/moshi/wn/*.txt` (NEW/ENHANCED/FIXED tags and PRO flags)
- `/home/akram/code/or2/.amp/in/moshi/64-research-inputs.md`
- `/home/akram/code/or2/.amp/in/moshi/60-licenses.txt`
- `/home/akram/code/or2/docs/design.md`
- `/home/akram/code/or2/docs/contracts.md` (M3 section)
- `/home/akram/code/or2/docs/ui.md`

No web sources were used.