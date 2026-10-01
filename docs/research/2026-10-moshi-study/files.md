# Files, image paste, diff and web preview for or2

## Findings

**What Moshi ships.** All four features are Pro-only (`64-research-inputs.md`). Moshi uses russh 0.57 plus russh-sftp 2.0, and react-native-webview for the browser preview.
- **Files:** v3.12.4 says attachments "upload faster… by reusing the active SSH connection". or2 already has that, with one connection per host.
- **Images:** v3.7.0 delivers pasted images "through the Moshi hook when installed", with a toast and a fix for double-tap timeouts.
- **Diff and preview:** both depend on its cloud-paired hook daemon. Moshi's sidebar also shows branch, ahead/behind and dirty state.
- **Preview extras:** the Safari-like chrome came in v3.1.0 and Docker/OrbStack dev-server discovery in v3.6.5. v3.0.0 added the "Open localhost web services from your paired host in-app" feature, and v3.15.0 made Preview and Diff toolbar buttons.
- **or2 has no daemon**, so it must do all of this over plain SSH exec, SFTP and direct-tcpip.

**russh-sftp** ([crates.io](https://crates.io/crates/russh-sftp), [repo](https://github.com/AspectUnk/russh-sftp))
- It is Apache-2.0, which is GPL-3.0-compatible, and has no GMS involvement.
- It is actively maintained. Versions run 2.4.0 (2026-08-03), 3.0.0 (2026-09-08) and 3.0.1 (2026-09-28), with commits as recent as today.
- 3.0.1 has only `russh ^0.63.2` as a dev-dependency. The `SftpSession::new` constructor takes any `AsyncRead + AsyncWrite + Unpin + Send + 'static` stream, so russh 0.63.3 is compatible. Feed it `channel.into_stream()` from a session channel with `request_subsystem("sftp")`.
- It covers open, create, read_dir, mkdir, rename, remove, stat, `expand-path`, `statvfs` and `fsync`.
- Its throughput is weak. 3.0.0 pipelines reads, but writes were reported as one in flight, about 256 KiB per RTT ([issue report](https://github.com/ahmadhajji/findssh/issues/12)). Uploads of 1–2 MB images are fine. Large file uploads need the `limits@openssh.com` chunk size or a different approach.
- Writing our own minimal SFTP v3 client would be M effort for no gain. The wire format is simple, but the edge cases (extended attributes, OpenSSH extensions, packet-size caps) are where bugs live.
- **Image upload alternative:** an exec that streams stdin into `cat > file` needs no subsystem. It also works where sshd disables SFTP. It would need a stdin-streaming exec, which `RemoteHost::exec` doesn't offer today (the contracts show output only, capped at 1 MiB).

**Claude Code accepts image file paths in the prompt.** Ctrl+V clipboard paste is unreliable over SSH, but dragging in or typing a path works. Formats are JPEG, PNG, GIF and WebP, up to 5 MB ([guide](https://smartscope.blog/en/generative-ai/claude/claude-code-image-guide/)). **Codex** handles path-in-prompt unreliably and prefers `-i` or Ctrl+V ([guide](https://codex.danielvaughan.com/2026/03/28/codex-cli-image-workflows/)). Treat Claude Code as the primary target and verify Codex on the device.

**Android Photo Picker** needs no permission. `PickVisualMedia` falls back to `ACTION_OPEN_DOCUMENT` without GMS ([docs](https://developer.android.com/training/data-storage/shared/photo-picker)). That makes it F-Droid-clean.

**WebView preview**
- russh has `Handle::channel_open_direct_tcpip` ([docs](https://docs.rs/russh/0.63.3/russh/client/struct.Handle.html)), so no new SSH capability is needed.
- WebView's `shouldInterceptRequest` cannot see POST bodies or WebSockets, which kills Vite/Next HMR. A real loopback listener is therefore required.
- `androidx.webkit` is Apache-2.0 and AOSP, not GMS. The WebView engine is a system component.

## Recommended approach

### Image paste (S/M, high impact)
1. Entry points are a composer button, a Photo Picker multi-select, and an `ACTION_SEND image/*` share target. Add Compose `contentReceiver` for keyboard image insert and clipboard-image paste.
2. Locally, decode, apply EXIF orientation, **strip EXIF/GPS** (privacy), downscale to at most 1568 px on the long edge, and re-encode to JPEG/WebP of 1 MB or less. This keeps uploads fast and under Claude's 5 MB limit.
3. Upload to `$XDG_CACHE_HOME/or2/paste/<yyyymmdd>-<rand>.jpg`, mode 0600, directory 0700. Write to a `.part` file, then rename. Use a space-free name so no shell quoting is needed.
4. Insert the absolute path plus a trailing space into the prompt without pressing Enter. Use bracketed paste when libghostty reports mode 2004, so Claude Code turns it into an image chip.
5. Show progress and a toast, with an inline "Retry/Cancel".
6. Prune files older than 24 h at connect, over exec.
7. Add a per-host setting to place files under the pane's `cwd/.or2-paste/` instead. Do this only if a sandboxed agent can't read the cache directory. Verify on the device whether Claude Code prompts for paths outside the cwd.

### Files (M, medium impact)
- Add an `or2_core::sftp` module over the existing `Arc<SshHost>`: `upload`, `download`, `list`, `remove`, and a size check after upload.
- Expose it through the FFI as a streaming handle with a progress listener.
- Use a Compose file browser starting at the pane `cwd`. Download via the Storage Access Framework (`ACTION_CREATE_DOCUMENT`) and upload via `ACTION_OPEN_DOCUMENT`.
- Respect the "session channels are limited by the server" contract by keeping at most one SFTP channel per host.

### Diff viewer (M/L, high impact)
- **Data, in Rust:**
  - Run `git -C <cwd> status --porcelain=v2 -z --branch` for the file list, branch and ahead/behind.
  - Run `git diff --numstat -z HEAD` for per-file stats.
  - Run `git diff --no-color --no-ext-diff -U3 HEAD -- <path>` lazily per file. This avoids the 1 MiB exec cap, and oversized files show "too large".
  - Cover untracked files with `git diff --no-index /dev/null <f>`.
  - Pass `RemoteCommand` args as an argv, never a shell string.
  - Parse the output into hunk and line records and return them over UniFFI.
  - Word-level intra-line highlighting uses the `similar` crate (Apache-2.0).
  - I'd write the roughly 150-line parser ourselves rather than add a dependency.
- **UI:** a native Compose `LazyColumn` with unified view, collapsible files and monospaced gutters. Add side-by-side on tablets.
- **Syntax highlighting** is optional and later. Use [`dev.snipme:highlights`](https://github.com/SnipMeDev/Highlights) (Apache-2.0, Kotlin, no GMS), or skip it.
- There is no maintained Compose diff widget worth adopting.
- Show an ahead/behind badge, like Moshi's.

### Web preview (M/L, medium-high impact)
- **Loopback proxy:**
  - A Rust listener on `127.0.0.1` binds an ephemeral port. Try the same port as the remote first, so absolute `localhost:3000` URLs and CORS still work.
  - Each accepted connection opens a `direct-tcpip` channel to `127.0.0.1:<port>` and then `::1`, since Node's `localhost` often binds IPv6 only.
  - The listener closes with the sheet.
- **Discovery:** exec `ss -ltnH` on Linux or `lsof -nP -iTCP -sTCP:LISTEN` on macOS. Rank by whether the process cwd is under the pane's workspace, and let the user pick or type a port.
- **WebView hardening:**
  - Add a `network_security_config` allowing cleartext only to `127.0.0.1`.
  - Disable Safe Browsing via the manifest meta-data, since it phones Google.
  - Block file/content access and external navigation (open them in the browser).
  - Add a minimal URL bar with reload and back buttons.

## Phased plan
- **P1 (M4):** SFTP module plus image paste. Needs russh-sftp 3.0.1 and the pipeline above.
- **P2:** diff viewer v1, covering list, unified view and lazy per-file loading.
- **P3:** web preview with the loopback proxy, discovery, and a hardened WebView.
- **P4:** the file browser, diff side-by-side and highlighting, and optional "Preview/Diff" toolbar buttons.

## Risks
- **Loopback exposure:** any local app can reach the forwarded port while the sheet is open. Mitigate with an ephemeral port, a short lifetime, and a one-time token if we later put an HTTP-aware shim in front.
- **sshd limits:** `AllowTcpForwarding no` or `DisableForwarding yes` breaks preview, and `Subsystem sftp` may be absent. Surface a clear error, as the contracts already do for streamlocal, and fall back to exec `cat` for uploads.
- **russh-sftp 3.0** is three weeks old. Pin `=3.0.1` and test it against OpenSSH and macOS sftp-server. 2.4.0 is the fallback.
- **Large diffs** hit the 1 MiB exec cap, so keep loading lazy per file.
- **Architecture invariants:** a listening `TcpListener` is not a `Transport` connect, but put it in one `forward` module and note the exception in the contracts.
- **Agent behaviour:** whether an agent reads an inserted path as an image varies. It is verified for Claude Code and unverified for Codex.

## Sources
- [russh-sftp on crates.io](https://crates.io/crates/russh-sftp)
- [russh-sftp repo](https://github.com/AspectUnk/russh-sftp)
- [`SftpSession` docs](https://docs.rs/russh-sftp/latest/russh_sftp/client/struct.SftpSession.html)
- [russh `Handle` docs](https://docs.rs/russh/0.63.3/russh/client/struct.Handle.html)
- [Claude Code image guide](https://smartscope.blog/en/generative-ai/claude/claude-code-image-guide/)
- [Codex CLI image guide](https://codex.danielvaughan.com/2026/03/28/codex-cli-image-workflows/)
- [Android Photo Picker](https://developer.android.com/training/data-storage/shared/photo-picker)
- [SFTP write-pipelining report](https://github.com/ahmadhajji/findssh/issues/12)
- [SnipMeDev/Highlights](https://github.com/SnipMeDev/Highlights)
- Local: `/home/akram/code/or2/.amp/in/moshi/63-whatsnew-all.txt`, `60-licenses.txt`, `64-research-inputs.md`, and `/home/akram/code/or2/docs/design.md` and `contracts.md`. `docs/roadmap.md` does not exist; the milestones are in `design.md`.