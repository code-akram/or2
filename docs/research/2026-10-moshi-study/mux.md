# Multiplexers and transports: zellij, Eternal Terminal, scrolling in mosh

## Findings

**Scrolling gap in or2 today.** `Terminal::scroll` (`core/or2-core/src/terminal.rs:434`) scrolls libghostty's local viewport on the primary screen. On the alternate screen it sends arrow keys. There is no mouse-wheel encoding anywhere in `core/or2-core/src`. tmux, herdr and zellij all run on the alternate screen, so a swipe becomes Up/Down keypresses. Those reach the shell or agent as command history, not as pane scrolling. This matters most under mosh, where libghostty holds almost no history (design.md, M0 findings).

**What Moshi does.** Its drag gesture is forwarded as mouse-wheel events when tmux has `mouse on`. Full history is reached through tmux copy mode, with a scroll-to-bottom button for tmux (3.2.0) and herdr. Its zellij page gives no scrolling detail. It admits that mosh restores only the visible screen ([Moshi scrolling](https://getmoshi.app/docs/scrolling)).

**herdr schema** (`core/or2-core/src/herdr/schema.json`, herdr 0.9.3, protocol 22):
- `pane.read {pane_id, source: visible|recent|recent_unwrapped|detection, format: text|ansi, lines?: u32, strip_ansi}` returns `{text, truncated, revision}`.
- `pane.scroll {pane_id, offset_from_bottom}` scrolls a pane over the API.
- `PaneInfo.scroll {offset_from_bottom, max_offset_from_bottom, viewport_rows}` gives scroll state.
- The `pane.scroll_changed` event exists, but `herdr/watch.rs:77` currently ignores it.
- `pane.edit_scrollback` and `pane.selection.read` also exist.
- or2 has no wrapper for any of these yet.

**tmux** (from my knowledge of tmux's CLI; not re-fetched). `capture-pane -p -e -J -S -<N> -t <pane>` dumps history with escapes. `copy-mode -e -t <pane>` plus `send-keys -X` drives copy mode. `display-message -p '#{pane_id} #{pane_in_mode} #{scroll_position} #{history_size} #{alternate_on}'` reports state.

**zellij** ([programmatic control](https://zellij.dev/documentation/programmatic-control.html), [CLI actions](https://zellij.dev/documentation/cli-actions.html), [subscribe](https://zellij.dev/documentation/zellij-subscribe.html)):
- Latest is 0.45.1 (Aug 2026). The licence is MIT. or2 would only exec its CLI, so nothing is linked.
- Version 0.44.0 (Mar 2026) added the JSON CLI: `action list-panes --json -a`, `list-tabs --json -a`, `dump-screen --pane-id X --full --ansi`, `subscribe --pane-id X --format json [--scrollback]`, and `action go-to-tab`. A global `--session NAME` targets a session from outside.
- Before 0.44, every upgrade orphaned running sessions. From 0.44 on, the client/server protocol is forward-compatible ([release notes](https://zellij.dev/news/remote-sessions-windows-cli/)).
- Session listing is `zellij list-sessions`. The default output is human-readable (`name [Created …] (EXITED - attach to resurrect)`). Use `--short --no-formatting` and verify with `--help`. Exited sessions are listed too and `attach` resurrects them.
- Attach with `zellij attach -c <name>`.
- No agent-state API exists. A WASM plugin API does, but it would mean installing something on the host, which breaks or2's "no host daemon" rule.
- Moshi's zellij support is thin: a session picker plus a tab shortcut row. It has no tree navigation ([Moshi zellij](https://getmoshi.app/docs/zellij)).

**Eternal Terminal** ([how it works](https://eternalterminal.dev/howitworks/), [repo](https://github.com/MisterTea/EternalTerminal)):
- **Licence and release.** Apache-2.0, which is GPLv3-compatible. The latest release is 7.0.0 (Jul 2024), so activity is slow.
- **Handshake.** SSH runs `etterminal` and returns an id and passkey. The client then connects to `etserver` on TCP 2022 and the SSH leg closes.
- **Wire format.** Protobuf, protocol v6. Encryption is libsodium secretbox (XSalsa20-Poly1305).
- **Recovery.** BackedReader/BackedWriter keep sequence numbers and replay the lost bytes on reconnect.
- **Server availability.** It needs the `etserver` daemon on the host, which is packaged for Arch AUR and Homebrew. It is TCP only, with no UDP.
- **Rust clients.** [AArnott/eternal-terminal-client](https://github.com/AArnott/eternal-terminal-client) is MIT, speaks protocol v6, and is Rust. It is a CLI binary with no library API. It lacks forwarding and agent support, and it has 1 star. [tphakala/et-go](https://github.com/tphakala/et-go) is Go, Apache-2.0 and an MVP. No reusable Rust crate exists.
- **Moshi's ET.** Pro-only and "experimental" ([connections](https://getmoshi.app/docs/connections)).

**ET vs mosh for or2.**

| | mosh | ET |
|---|---|---|
| Transport | UDP | TCP, so it works where UDP is blocked |
| Scrollback | Screen diffs only, so no history | Raw byte stream, so libghostty keeps full scrollback and the or2 scroll path works unchanged |
| Predictive echo | Yes | No |
| Lossy links | Handles loss well | TCP head-of-line blocking |
| Host requirement | `mosh-server` is spawned per session | A running `etserver` daemon and an open port 2022 |

or2's own hosts run mosh over ZeroTier, which carries UDP, so ET's main selling point does not apply today.

## Recommended approach (priority order)

1. **Wheel-aware scroll routing (S, high).**
   - Read the mouse-tracking modes (1000/1002/1003 with SGR 1006) and alternate scroll (1007) from libghostty. Check whether the pinned libghostty `8953a74` exposes a mouse encoder.
   - When the app tracks the mouse, send SGR wheel events. With 1007 only, send arrows. Otherwise scroll the local viewport.
   - Add an opt-in "enable tmux mouse for this session" action that runs `tmux set -t =<s> mouse on`. Never apply it silently.
   - Add the scroll-to-bottom button. For herdr, drive it from `PaneInfo.scroll` and subscribe to `pane.scroll_changed`, using `pane.scroll offset_from_bottom=0` to jump. For tmux, use `pane_in_mode` and `send-keys -X cancel`.

2. **History sheet over an exec channel (M, high).**
   - It works under both mosh and SSH, and whether or not mouse is on.
   - The sources are tmux `capture-pane -p -e -J -S -N`, herdr `pane.read source=recent_unwrapped format=ansi lines=N` (check `truncated`), and zellij `dump-screen --full --ansi`.
   - Render the result in a read-only libghostty instance. Add search, copy, and "jump to live".
   - Resolve the pane id with `display-message` for tmux, the existing herdr view for herdr, and `list-panes --json` for zellij.
   - Re-wrapping unwrapped text to phone width gives better UX than the host's pane width. This is a snapshot, not live. Moshi has no equivalent.

3. **Drive native copy mode (S–M, medium).** On the first upward drag, exec `tmux copy-mode -e -t %id`, then send PageUp or wheel events through the PTY. This gives tmux's own search and selection, but it is lower priority than item 2.

4. **zellij parity (M, medium).**
   - Add a `Zellij` probe (path and `--version`) and a `TerminalTarget::Zellij{session}` using `attach -c`.
   - Add a session picker from `list-sessions --short --no-formatting`.
   - For 0.44 and later, build a tab/pane tree from `--session X action list-tabs --json -a` and `list-panes --json -a`, with `go-to-tab` swipe switching. Below 0.44, offer list and attach only.
   - Poll for agents via `list-panes --command` and show `dump-screen` tails as previews.
   - Skip the plugin API.

5. **ET (L, low for now).**
   - Defer it. It adds a `Transport::Tcp` session type, a new `et` module in `or2-core`, a prost protobuf definition, `crypto_secretbox` (RustCrypto, MIT/Apache) and a port of the MIT Rust client. All of these are GPL- and F-Droid-clean.
   - The cost is roughly 2k lines, plus host setup (the `etserver` service and TCP 2022).
   - If you do it, port from the MIT client and record it in `THIRD_PARTY_NOTICES.md`, and read the C++ source for exact framing.
   - Revisit if you hit UDP-blocked networks.

## Risks

- **Unverified libghostty support.** I have not checked that the pinned libghostty exposes mouse encoding or an alternate-scroll mode query.
- **tmux changes your config.** Setting `mouse on` also changes click and selection behaviour. Keep it opt-in.
- **Latency.** The `copy-mode` and `pane.scroll` exec calls cost one round trip each. Coalesce them, and use wheel events for continuous drags.
- **Fragile zellij parsing.** Human-readable `list-sessions` output is brittle, so gate on the version. The socket directory may differ under non-interactive SSH when `XDG_RUNTIME_DIR` is unset, notably on macOS. Test this against the Mac host.
- **ET protocol version.** Master's `ET.proto` shows a challenge/auth handshake, while the clients I read advertise protocol v6 against a 7.0.0 server. Confirm the current version before building anything.
- **herdr mouse behaviour.** I assumed herdr handles the wheel itself. The docs I fetched don't say, so test it.

## Sources

- [Moshi scrolling](https://getmoshi.app/docs/scrolling)
- [Moshi zellij](https://getmoshi.app/docs/zellij)
- [Moshi connections](https://getmoshi.app/docs/connections)
- [Zellij programmatic control](https://zellij.dev/documentation/programmatic-control.html)
- [Zellij CLI actions](https://zellij.dev/documentation/cli-actions.html)
- [Zellij subscribe](https://zellij.dev/documentation/zellij-subscribe.html)
- [Zellij 0.44 news](https://zellij.dev/news/remote-sessions-windows-cli/)
- [Zellij releases](https://github.com/zellij-org/zellij/releases)
- [ET repo](https://github.com/MisterTea/EternalTerminal)
- [ET how it works](https://eternalterminal.dev/howitworks/)
- [AArnott Rust ET client](https://github.com/AArnott/eternal-terminal-client)
- [Rust ET client announcement](https://blog.nerdbank.net/2026/07/20/introducing-eternal-terminal-client-a-rust-powered-eternal-terminal-client-built-for-windows-and-everywhere-else/)
- [et-go](https://github.com/tphakala/et-go)
- [herdr docs](https://herdr.dev/docs/socket-api)
- Local: `core/or2-core/src/terminal.rs`, `core/or2-core/src/herdr/schema.json`, `docs/design.md`