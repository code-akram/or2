## Chat View for or2: findings and recommendation

### Findings

**1. herdr already gives the pane-to-transcript mapping, and or2 drops it.** Schema 0.9.3 (`core/or2-core/src/herdr/schema.json`) puts `agent_session {source, agent, kind: "id"|"path", value}` on `PaneInfo` and `AgentInfo` (`kind` is an open enum). It is omitted when herdr has no session reference. The Claude, Codex, Copilot, Devin, Droid, Qoder, Cursor, Grok and OpenCode integrations report the session **id** from their hooks on session start ([integrations](https://herdr.dev/docs/integrations/), [socket API](https://herdr.dev/docs/socket-api/)). `pane.report_agent_session` also accepts `agent_session_path`. The `HerdrView` projection in `contracts.md` (pane and agent fields) has no `agent_session`, so adding it is the first change.

**2. herdr read and write calls (schema).**
- `agent.read` takes `{target, source, lines?, format: text|ansi, strip_ansi}`. `pane.read` takes `{pane_id, source, lines?, format, strip_ansi}`. `source` is `visible | recent | recent_unwrapped | detection`.
- `PaneReadResult` returns `text`, `revision` and `truncated`. There is no diff or cursor API, so every read is a full re-read.
- `agent.explain {target}` returns the detection rule, manifest and evidence. It is a debugging aid, not content.
- Input: `pane.send_text {pane_id, text}`, `pane.send_keys {keys[]}`, `pane.send_input {text?, keys?}`, and `agent.prompt {target, text, wait?}`. `agent.prompt` submits and optionally waits in one request.
- `pane.process_info` returns argv, cwd and pid of the foreground process. That is useful when `agent_session` is missing.

**3. Claude Code transcripts.** The file is `~/.claude/projects/<cwd-encoded>/<session-id>.jsonl`. Each line carries `type`, `uuid`, `parentUuid`, `timestamp`, `sessionId`, `cwd`, `gitBranch` and `version`. `message.content[]` holds `text`, `thinking`, `tool_use` and `tool_result` blocks ([format notes](https://claude-dev.tools/docs/jsonl-format)).
- Hooks receive `transcript_path`, and the docs state the file is written **asynchronously and may lag** the in-memory conversation ([hooks](https://code.claude.com/docs/en/hooks)).
- Old files are deleted after 30 days by default. `CLAUDE_CONFIG_DIR` relocates the directory ([where six CLIs store logs](https://allaboutcoding.ghinda.com/where-ai-coding-clis-store-session-logs/)).
- Entry types evolve with Claude Code versions, so the parser must tolerate unknown types.

**4. Codex transcripts.** The file is `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`, with `{timestamp, type, payload}` per line.
- `type` is one of `session_meta` (line 1), `turn_context`, `response_item` (`message`, `function_call`, `function_call_output`, linked by `call_id`) or `event_msg` (`agent_message`, `task_started`, `task_complete`, `token_count`, ...).
- There is no parent pointer, so causality comes from file order. The format is undocumented and the real wire format differs from the source comments ([reverse-engineering notes](https://dev.to/milkoor/reverse-engineering-codex-cli-rollout-traces-3b9b)).
- Files can reach GBs from compaction history ([openai/codex#24948](https://github.com/openai/codex/issues/24948)), so never read a whole file.

**5. Other agents are heterogeneous.**
- Amp writes one JSON file per thread (`~/.local/share/amp/threads/T-*.json`).
- Cursor and OpenCode use SQLite.
- Pi writes JSONL.
- I could not verify the paths for Droid, Copilot, Qoder or Devin.

Moshi's own text says Chat View "uses the agent's TUI running on your host", and its hook daemon feeds context and inbox data. I did not verify how it builds the chat.

**6. Parsing TUI text** (herdr's own manifests do this for status). It is brittle, loses structure and has no tool-card data. Use it only for pending prompts and for agents with no transcript parser.

### Recommended architecture

Rust builds a normalised `ChatEvent` stream per pane (`User`, `AssistantText`, `Thinking`, `ToolCall{id, name, input}`, `ToolResult{id, output}`, `Status`, `Unknown{raw}`). Kotlin renders it. Kotlin never sees JSONL.

1. **Resolve the session (S).** Project `agent_session` into `HerdrPane` and `HerdrAgent`. Then run these over the host's exec channel:
   - Claude: `ls ~/.claude/projects/*/<id>.jsonl`. A glob avoids reimplementing cwd encoding. Probe `CLAUDE_CONFIG_DIR` and `~/.config/claude` as well.
   - Codex: `find ${CODEX_HOME:-~/.codex}/sessions -name 'rollout-*-<id>.jsonl'`.
   - With no `agent_session`, use `pane.process_info` for `--resume <id>` in argv. Last resort is the newest-mtime file whose cwd matches. The UI labels this "guessed".
   - `/clear` and resume change the id, and herdr's hook updates `agent_session`. The watch already sees that as an invalidation, so re-resolve then.
2. **Backfill (S).** Fetch the last ~512 KB with `tail -c`, drop the partial first line, and for Claude walk the `parentUuid` chain from the last line to get the active branch. Page older history on scroll-up.
3. **Follow (M).** Open one long-lived exec `tail -c +<offset> -F <file>` per open chat and keep the byte offset.
   - On reconnect, `stat` the file and resume from the offset. If the file shrank or the inode changed, reset.
   - Only the final unterminated line is ever partial, so hold it until its newline arrives.
   - Fall back to polling `stat` and `tail` every 1 to 2 s where `tail -F` misbehaves.
   - The BSD `tail` and `stat` flags differ on the Mac host, so write fixtures for both.
4. **Dedupe and merge (M).**
   - Claude: key on `uuid`. Merge assistant lines that share `message.id`, as I recall streamed blocks arriving on separate lines (verify against fixtures). Drop `isSidechain`, meta entries and the slash-command pseudo-user wrappers.
   - Codex: treat `response_item` as truth and use `event_msg` only for command and patch results and for `task_complete`. Verify the duplication between the two on real files.
5. **Liveness (S).**
   - Use the existing herdr watch for `working`, `blocked` and `idle`, and for the `state_change_seq` bump.
   - While `working`, show a spinner plus tool cards as they land, since transcript lines arrive one block at a time.
   - Skip token-level streaming. A pane.read-diff ghost bubble is fragile and not worth v1.
6. **Blocked prompts (M).**
   - A `tool_use` with no `tool_result` while status is `blocked` becomes an approval card (command or diff).
   - Parse `pane.read visible` for the option list. Buttons send `pane.send_keys` (digit, Enter or Esc).
   - Where parsing fails, show "answer in terminal".
7. **Replies (S).** Composer text goes into the pane the same way the current composer does, using the proven `submit_text` sequence: bracketed paste or text, a 100 ms pause, then a separate Enter. That sequence was needed because TUIs treat text plus Enter in one burst as a paste.
   - If or2 holds a terminal channel on that pane, reuse `Session.submit_text`.
   - Otherwise send `pane.send_text`, then `pane.send_keys ["enter"]` about 100 ms later. Test whether `agent.prompt` and `pane.send_input{text,keys}` pace correctly, and whether `pane.send_text` honours bracketed paste for multi-line input. Fall back to opening a terminal channel if not.
   - Confirm delivery when the matching `User` line appears in the transcript. If `working` has not begun within ~3 s, check `pane.read visible` for text still sitting in the input box and offer retry.
8. **FFI (S).** `ChatWatch`, modelled on `HerdrWatch`, delivers `{reset, events[]}` deltas through a notify-once mailbox.
9. **UI (M-L).** A `LazyColumn` with tool cards, a markdown renderer, a diff-stat card and a jump-to-terminal button.
   - Markdown: [mikepenz multiplatform-markdown-renderer](https://github.com/mikepenz/multiplatform-markdown-renderer) is Apache-2.0 and pure Kotlin from Maven Central, so it is GPLv3-compatible and F-Droid-buildable. It has `retainState` for updates. Alternative: `pulldown-cmark` (MIT) in Rust emitting spans.
   - Any Kotlin cache of messages goes in no-backup storage, off by default.

**Scope and effort.** Claude Code plus Codex, read-only, with composer reply is **L overall, high impact**. It is the biggest remaining differentiator versus Moshi, and it needs no host daemon. Do `agent_session` projection and a one-command transcript locator first (S); that proves the mapping on both hosts. Do approvals second. Other agents come last, one parser each. Amp, OpenCode and Cursor need JSON or SQLite reads over exec (`sqlite3` may be absent), so treat them as separate work items.

**Parsers.** Write them in-house with `serde_json` (MIT/Apache) and `Value` fallbacks. [majiayu000/agent-sessions](https://github.com/majiayu000/agent-sessions) (MIT, Rust, Claude and Codex, tolerates a partial tail line) is a design reference. Vendoring needs a `THIRD_PARTY_NOTICES.md` entry with its source, a pinned commit, licence and copyright line.

### Rejected
- Driving `claude -p` or the Agent SDK, or `codex app-server`: these start a separate process, not the live TUI in the pane.
- Running a host daemon: this violates the design constraints.

### Risks
- **Format drift.** Mitigate with tolerant parsing, an `Unknown` raw card, and fixtures captured from the owner's hosts. Take the fixtures read-only and sanitise them before committing, since transcripts contain secrets.
- **Lag.** Transcript writes are asynchronous, so chat can trail the TUI by about a second.
- **Mapping misses.** `agent_session` is absent when herdr's hook was not installed (`integration.list` shows this), and herdr must be recent enough to report it.
- **SSH channel limits.** OpenSSH `MaxSessions` defaults to 10 session channels per connection. Terminals plus exec plus tail share it, so keep it to one tail per open chat.
- **Retention and size.** Claude may have deleted old files. Codex files can be huge, so every read is bounded.
- **Reply races.** A reply sent while the agent is working may queue or be interpreted differently by each agent.
- **Privacy.** The chat cache holds sensitive data.

### Sources
- https://herdr.dev/docs/socket-api/
- https://herdr.dev/docs/integrations/
- https://github.com/herdrdev/herdr/blob/master/docs/versions/0.8.0/website/src/content/docs/integrations.mdx
- https://code.claude.com/docs/en/hooks
- https://claude-dev.tools/docs/jsonl-format
- https://dev.to/milkoor/reverse-engineering-codex-cli-rollout-traces-3b9b
- https://allaboutcoding.ghinda.com/where-ai-coding-clis-store-session-logs/
- https://github.com/openai/codex/issues/24948
- https://github.com/majiayu000/agent-sessions
- https://github.com/mikepenz/multiplatform-markdown-renderer
- Local: `core/or2-core/src/herdr/schema.json`, `docs/design.md`, `docs/contracts.md`, `.amp/in/moshi/63-whatsnew-all.txt`, `.amp/in/moshi/07-chatmode.txt`