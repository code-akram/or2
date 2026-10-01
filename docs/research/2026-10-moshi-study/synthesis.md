# or2 roadmap proposal: M4, M5 and later

Note: `docs/roadmap.md` does not exist. Milestones live in `design.md`, where M4 is "quick replies, opt-in notifications, image paste, voice". Wake-on-LAN, the TCP wake probe, keep-screen-on and Easy pair are not in `design.md`, `contracts.md` or `ui.md`. I take them as planned because the task says so, and I leave them out of the backlog.

## 1. Executive summary

**What Moshi teaches** (`63-whatsnew-all.txt`, 254 items over 46 releases, counts ±3):
- **Reliability is the fragile area.** Connection reliability is 24 items, half of them fixes. Mosh/ET recovery after backgrounding shows up in about 19 releases. M3-B should be treated as a long tail, not a one-off.
- **The order was mechanism, then agent awareness, then chrome.** Chat View is the largest bet at 54 items, 44 of them PRO. Multiplexer navigation is second at 40.
- **Per-agent adapters cost a lot.** Moshi added about 14 agents and paid 8 Chat View fixes. or2 should support few agents well.
- **The session switcher was rewritten twice** (v3.6.3, v3.12.0). Keep or2's Inbox simple.

**Where or2 should NOT follow:**
- **Cloud relay and device tokens.** Moshi pushes through its own cloud via a `moshi-hook` daemon. That conflicts with "no vendor cloud" and "no host daemon".
- **Analytics and cloud-paired hook daemons.** Moshi's diff and preview depend on one. Use SSH exec, SFTP and direct-tcpip instead.
- **The paywall.** About 32% of Moshi items are PRO. or2 is GPL-3.0-or-later, so everything is free.
- **iOS-only features.** Live Activities, Dynamic Island, iCloud and Watch do not transfer.
- **Low-value breadth.** ET, tablet layout and sync are weak fits for a two-host, one-owner tool.

**Where or2 should follow:**
- Never lose the session.
- Agent triage from the phone.
- Fast multiplexer navigation and working scroll.
- Typing efficiency.
- Seeing results (images, diff, preview).

## 2. Prioritised backlog

Effort: S, M or L. Every library below is GPL-3.0-compatible. Only Apache-2.0, MIT and CC-BY-4.0 are named. Dependency licences come from the reports, not from re-checking.

### Next (fold into M4; finish M3-B first)

| Item | User value | Effort | Depends on | Risks | Licence notes |
|---|---|---|---|---|---|
| M3-B acceptance hardening | Session survives background, kill and network switch | M | M3-B | OxygenOS kills the service. Test the garbled-on-reuse bug Moshi fixed in v3.14.2. | none |
| Local notifications (`NotifyPolicy`) | Notify on edges into Blocked or Done, de-duplicated by `(host,pane,state_change_seq)` | S | M3-B service and `HerdrWatch` | Dies if the service is killed. Show "last seen" age. | `androidx.core` Apache-2.0 |
| Wheel-aware scroll routing | Swipe in tmux or herdr scrolls the pane, not shell history | S | Check that pinned libghostty `8953a74` exposes mouse modes | Not yet verified. Today a swipe on the alt screen sends arrows (`terminal.rs:434`). | none |
| Scroll-to-bottom button | Matches Moshi v3.2.0 | S | Wheel routing; herdr `PaneInfo.scroll` | `pane.scroll_changed` is currently ignored (`watch.rs:77`) | none |
| Tap links and multi-line link wrap | Cheap, frequent | S | none | none | none |
| Project `agent_session` into `HerdrPane` and `HerdrAgent` | Unlocks Chat View and tap routing | S | none | Absent when the herdr hook is not installed | none |
| Tap routing `or2://host/<id>/pane/<pane>` | Notification opens the right pane | S | M3-B reattach | none | none |

### M4 (as in design.md, resequenced)

| Item | User value | Effort | Depends on | Risks | Licence notes |
|---|---|---|---|---|---|
| SFTP module plus image paste | Send screenshots to Claude Code | M | russh-sftp pinned `=3.0.1`, fallback 2.4.0 | 3.0.1 is about 3 weeks old. Writes are slow but fine for 1 MB images. Codex path handling is unverified. | russh-sftp Apache-2.0 |
| Notification actions (RemoteInput Reply, Open) | Answer from the shade | M | Local notifications | Lock-screen sends. Require unlock; never queue silently. | Apache-2.0 |
| Android 16 Live Update | One ongoing summary ("2 blocked") | S-M | Local notifications | Promotion may be refused. Fall back to an ordinary ongoing notification. | none |
| History sheet over exec | Real scrollback under mosh, with search and copy | M | Resolved pane id | Snapshot only. Zellij and macOS socket paths need testing. | none |
| tmux "mouse on" opt-in action | Wheel works in tmux | S | Wheel routing | Changes click and selection behaviour. Never apply silently. | none |
| ntfy hook snippet (remote push phase 1) | Alerts while the app is dead | S | none | Prompt text is visible to the ntfy server. Default payload is metadata only; recommend self-hosting. | ntfy app Apache-2.0/GPLv2, no code linked |
| BYOK dictation | Voice into the composer | S-M | Settings, Keystore-wrapped key | Needs an HTTPS path (see questions). Off by default, no pre-filled URL. | OpenAI-shaped API; no code |

### M5

| Item | User value | Effort | Depends on | Risks | Licence notes |
|---|---|---|---|---|---|
| Chat View v1 | Moshi's biggest differentiator, with no cloud | L | `agent_session`, exec tail | Format drift, lag of about 1 s, sensitive cache | `serde_json`; markdown via mikepenz renderer (Apache-2.0) or `pulldown-cmark` (MIT) |
| Approval cards (Chat View step) | Approve or deny from the phone | M | Chat View | Option parsing is brittle. Fall back to "answer in terminal". | none |
| Diff viewer v1 | See what the agent changed | M-L | git over exec | The 1 MiB exec cap means per-file lazy loading | `similar` Apache-2.0, optional |
| Web preview | Open dev servers | M-L | `direct-tcpip`, loopback listener | `AllowTcpForwarding no`. Any local app can reach the port while open. Disable Safe Browsing. | `androidx.webkit` Apache-2.0 |
| UnifiedPush connector (remote push phase 2) | or2-owned notification with RemoteInput | M | ntfy phase 1 | Distributor must survive OxygenOS | connector 3.3.5 and Tink, Apache-2.0. Exclude `embedded_fcm_distributor` (pulls Play Services). |

### Later/Maybe

| Item | Effort | Notes |
|---|---|---|
| On-device dictation (sherpa-onnx, Moonshine small streaming, Silero VAD) | L | F-Droid must build sherpa-onnx and ONNX Runtime from source, because the crate's `build.rs` downloads prebuilt binaries. Download models on user action only. Add attribution for CC-BY weights. Unmeasured on 8 Gen 1. Possible NonFreeNet or NonFreeAsset flags. |
| Zellij parity | M | Needs zellij 0.44 or later (MIT, exec only). Below that, offer list and attach only. |
| tmux control mode, file browser, side-by-side diff, syntax highlighting | M | `dev.snipme:highlights` Apache-2.0 |
| YubiKey/FIDO2 keys, Chat View for agents beyond Claude and Codex | M each | One parser each. |
| ET (about 2k lines, Apache-2.0 upstream, port the MIT Rust client) | L | Revisit only if UDP is blocked on a network you use. |
| Embedded ZeroTier, Cloudflare Access | L | Already "later" in design.md. |

**Not planned:** Moshi's cloud push, tablet layout, sync, Apple-only features.

## 3. Sequencing and acceptance criteria

### Milestone A: finish M3, plus the "Next" bundle (small, mostly S)

1. **M3-B and v0 step 3.**
   - Accepted when the phone is backgrounded for 10 minutes over mobile data and returns to the same pane with no re-typing.
   - Also run force-kill, Wi-Fi to mobile data, and a saved connection that fails to restore.
   - Reused sessions must not arrive garbled until a resize.
2. **Local notifications.**
   - A Blocked or Done edge on a host that is off by default fires exactly one notification per `state_change_seq`.
   - Nothing fires when the pane is on screen.
   - The first-connect opt-in is shown.
   - Tapping opens the right pane via the deep link.
3. **Wheel routing and scroll-to-bottom.**
   - In tmux with `mouse on` and in herdr, a swipe scrolls the pane, not shell history.
   - In plain shell, a swipe scrolls the local viewport.
   - The scroll-to-bottom button appears when scrolled up in herdr and in tmux.
4. **Tap links and `agent_session` projection.**
   - A URL on screen opens on tap.
   - `HerdrView` carries `agent_session` for Claude and Codex panes on both hosts.

### Milestone B: M4 proper

1. **Image paste.**
   - A picked image is EXIF-stripped, downscaled to at most 1568 px and 1 MB or less, and uploaded to `$XDG_CACHE_HOME/or2/paste/` with mode 0600 inside a 0700 directory.
   - The path is inserted without pressing Enter, and Claude Code shows an image chip.
   - Files older than 24 hours are pruned at connect.
   - Verified on both the Linux and macOS hosts.
   - A clear error shows when `Subsystem sftp` is disabled.
2. **Notification actions and Live Update.**
   - Reply sends through the proven `submit_text` sequence.
   - Approve and Deny require confirmation.
   - If the connection is down, the notification shows "Open to unlock" and sends nothing silently.
   - If promotion is refused, the plain ongoing notification is used.
3. **History sheet.**
   - It shows at least N lines from tmux or herdr under mosh, with search, copy and "jump to live".
4. **ntfy snippet.**
   - Settings shows a copyable hook snippet.
   - A hook POST to a self-hosted topic produces a notification whose tap opens the pane.
   - The default payload carries no prompt text.
5. **BYOK dictation.**
   - It is disabled until the user enters a URL and key.
   - The key is Keystore-wrapped and never logged.
   - A consent screen is shown.
   - Result text lands in the composer, not the PTY.
   - A word-replacement map applies.

M5 starts with a design pass for Chat View. First step: a one-command transcript locator for Claude Code, then Codex, with fixtures taken read-only from your hosts and sanitised before commit.

## 4. Risks (cross-cutting)

- **The Transport rule has no answer for HTTP.** AGENTS.md says everything network goes through `Transport` and Kotlin never speaks a wire protocol. Third-party HTTPS for BYOK dictation, a preview loopback `TcpListener`, and any UnifiedPush HTTP path all need either a Rust HTTPS transport or a documented exception in `design.md`. Decide before M4.
- **SSH channel budget.** OpenSSH `MaxSessions` defaults to 10. Terminals, exec, SFTP and chat tails share one connection, so cap it at one SFTP channel per host and one tail per open chat.
- **Local notifications are only "live while connected".** Remote push exists to cover the gap, and the ntfy app has the same OxygenOS problem.
- **F-Droid review.** NonFreeNet or NonFreeAsset flags are possible for BYOK and model downloads, and need a maintainer's answer.
- **Not verified in the reports:**
  - the 8 Gen 1 speech RTF;
  - whether libghostty `8953a74` exposes mouse modes;
  - Codex image-path behaviour;
  - whether herdr handles the mouse wheel itself;
  - UnifiedPush registration details.
- **Moshi's Chat View mechanism.** Moshi's internals are not confirmed; its own text says only that it "uses the agent's TUI running on your host".

## 5. Open questions for the owner

1. **Transport rule exception.** Is a Rust HTTPS transport acceptable, or must BYOK dictation reach only your own host over `direct-tcpip`?
2. **Which milestone owns "Next"?** Fold the Next bundle into M4 as proposed, or ship it as a separate M3.5?
3. **Remote push.** Is ntfy-app-only (no connector library, no RemoteInput) enough, or do you want the UnifiedPush connector in M5?
4. **Agent scope.** Claude Code and Codex only for Chat View? Do you use other agents (Amp, Cursor, OpenCode)? Those use JSON or SQLite and are separate work.
5. **Privacy.** Is an optional, no-backup, off-by-default Chat View cache acceptable, or should it be memory-only?
6. **Dictation.** Is English-only streaming (Moonshine) enough, or do you need multilingual (Parakeet, 640 MB)? Is a from-source ONNX build for F-Droid acceptable?
7. **tmux config.** May or2 run `tmux set mouse on` with explicit consent, or must scroll work with no host-side config change?
8. **Image paste location.** Is a cache directory outside the working directory fine? That needs a device check of whether Claude Code prompts for paths outside the cwd.
9. **Scope drops.** Confirm ET, Zellij and tablet layout stay in "Later/Maybe", given you run two hosts over ZeroTier with UDP.
10. **Wake items.** Wake-on-LAN, the TCP wake probe, keep-screen-on and Easy pair are not in the current docs. Which milestone owns them?

## Sources

- Local, read: `docs/design.md`, `docs/contracts.md` section headings, `docs/ui.md`.
- Moshi inputs: `.amp/in/moshi/63-whatsnew-all.txt`, `.amp/in/moshi/64-research-inputs.md`, `.amp/in/moshi/60-licenses.txt`.
- URLs cited in the research reports:
  - https://herdr.dev/docs/socket-api/
  - https://herdr.dev/docs/integrations/
  - https://herdr.dev/docs/plugins/
  - https://code.claude.com/docs/en/hooks
  - https://crates.io/crates/russh-sftp
  - https://docs.rs/russh/0.63.3/russh/client/struct.Handle.html
  - https://developer.android.com/develop/ui/views/notifications/live-update
  - https://developer.android.com/training/data-storage/shared/photo-picker
  - https://docs.ntfy.sh/publish/
  - https://central.sonatype.com/artifact/org.unifiedpush.android/connector
  - https://unifiedpush.org/developers/implementations/
  - https://dontkillmyapp.com/oneplus
  - https://github.com/k2-fsa/sherpa-onnx
  - https://github.com/moonshine-ai/moonshine
  - https://github.com/davamix/ondevice-streaming-asr-bench
  - https://f-droid.org/en/docs/Inclusion_Policy/
  - https://github.com/mikepenz/multiplatform-markdown-renderer
  - https://zellij.dev/documentation/programmatic-control.html
  - https://eternalterminal.dev/howitworks/
  - https://getmoshi.app/docs/scrolling