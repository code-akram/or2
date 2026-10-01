//! Running commands and opening sockets on a host, independent of how the host is reached.
//!
//! herdr, tmux and mosh code talk to a [`RemoteHost`], never to SSH, so it is testable with
//! [`LocalHost`] (feature `test-support`). A [`RemoteCommand`] is a program, arguments and
//! environment assignments, never a shell string: sshd hands the rendered command to the user's
//! login shell (bash, zsh or fish), so [`RemoteCommand::render`] single-quotes every token in
//! the one form all three read identically.

use std::fmt;
use std::future::Future;
use std::ops::Deref;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use zeroize::Zeroize;

/// Each of stdout and stderr is capped; excess fails the exec.
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// Every exec is abandoned after this long.
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("the host connection is closed")]
    Closed,
    #[error("the command did not finish in time")]
    TimedOut,
    #[error("command output exceeded the 1 MiB cap")]
    OutputTooLarge,
    #[error("the command cannot be quoted for every login shell")]
    Unquotable,
    /// The host refused the channel, e.g. streamlocal forwarding is disabled.
    #[error("the host refused the request: {0}")]
    Rejected(String),
    #[error("i/o failed: {0}")]
    Io(String),
    /// The command ran and reported that it could not do its job (a nonzero or signalled
    /// exit); the message says what the caller was trying to do.
    #[error("{0}")]
    Failed(String),
}

/// Command output that is wiped when dropped. Any exec can carry a secret (`mosh-server new`
/// prints its session key on stdout), and the collector must wipe on every path: success
/// (when the caller drops the output), error, timeout, output cap and a cancelled future,
/// which all simply drop what was collected so far. Growth is done by hand
/// ([`SecretBytes::extend_capped`]) because a `Vec` reallocation would free the old block
/// without wiping it. `Debug` shows the length only.
///
/// This covers or2's own buffers. russh hands each packet's payload over in its own
/// zeroizing `CryptoVec`; the SSH transport's internal buffers are outside this type.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `data` unless the total would pass `cap` (`OutputTooLarge`, with nothing
    /// appended).
    pub fn extend_capped(&mut self, data: &[u8], cap: usize) -> Result<(), RemoteError> {
        let needed = self.0.len() + data.len();
        if needed > cap {
            return Err(RemoteError::OutputTooLarge);
        }
        if self.0.capacity() < needed {
            let capacity = needed.max(self.0.capacity() * 2).max(256).min(cap);
            let mut bigger = Self(Vec::with_capacity(capacity));
            bigger.0.extend_from_slice(&self.0);
            // `bigger` now holds the old block; dropping it wipes it.
            std::mem::swap(self, &mut bigger);
        }
        self.0.extend_from_slice(data);
        Ok(())
    }
}

impl From<Vec<u8>> for SecretBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for SecretBytes {
    fn from(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl Deref for SecretBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for SecretBytes {
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.0 == other.as_slice()
    }
}

impl PartialEq<&[u8]> for SecretBytes {
    fn eq(&self, other: &&[u8]) -> bool {
        self.0 == *other
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes(<{} bytes>)", self.0.len())
    }
}

impl Zeroize for SecretBytes {
    fn zeroize(&mut self) {
        #[cfg(test)]
        wipe_log::record(&self.0);
        self.0.zeroize();
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// Test hook: remembers which buffers were wiped, so a test can check that a secret it fed
/// through an exec path was wiped on that path (only buffers holding [`MARKER_PREFIX`] are
/// kept).
#[cfg(test)]
pub(crate) mod wipe_log {
    use std::sync::Mutex;

    pub(crate) const MARKER_PREFIX: &[u8] = b"or2-secret-";
    static WIPED: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

    pub(super) fn record(bytes: &[u8]) {
        if bytes
            .windows(MARKER_PREFIX.len())
            .any(|window| window == MARKER_PREFIX)
        {
            WIPED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(bytes.to_vec());
        }
    }

    /// How many wiped buffers contained `marker`.
    pub(crate) fn wiped(marker: &[u8]) -> usize {
        WIPED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter(|bytes| bytes.windows(marker.len()).any(|window| window == marker))
            .count()
    }
}

/// A finished command. `status` is `None` when the process ended without an exit status
/// (killed by a signal). The streams are [`SecretBytes`]: wiped when dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: Option<u32>,
    pub stdout: SecretBytes,
    pub stderr: SecretBytes,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

pub trait RemoteHost: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// Runs an already rendered command line, exactly as sshd would hand it to the login
    /// shell, without a PTY; collects stdout, stderr and the exit status. Output is capped at
    /// [`OUTPUT_CAP`] per stream and the run at [`EXEC_TIMEOUT`]. Callers use [`exec`] or
    /// [`exec_script`]; this is the one primitive an implementation provides.
    ///
    /// [`exec`]: RemoteHost::exec
    /// [`exec_script`]: RemoteHost::exec_script
    fn exec_rendered(
        &self,
        line: &str,
    ) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send;

    /// Renders `command` ([`RemoteCommand::render`]) and runs it with [`exec_rendered`].
    ///
    /// [`exec_rendered`]: RemoteHost::exec_rendered
    fn exec(
        &self,
        command: &RemoteCommand,
    ) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send {
        async move { self.exec_rendered(&command.render()?).await }
    }

    /// Runs a fixed script as `sh -c '<script>'` ([`render_script`], so newlines are fine and
    /// `'` and `\` are not) with [`exec_rendered`]. Never pass untrusted text.
    ///
    /// [`exec_rendered`]: RemoteHost::exec_rendered
    fn exec_script(
        &self,
        script: &str,
    ) -> impl Future<Output = Result<ExecOutput, RemoteError>> + Send {
        async move { self.exec_rendered(&render_script(script)?).await }
    }

    /// Opens a byte stream to a Unix socket on the host (OpenSSH direct-streamlocal). A socket
    /// that is missing or refuses the connection is [`RemoteError::Io`]; `Rejected` is only for
    /// a host that refuses the channel itself (streamlocal forwarding disabled). The herdr
    /// watch of a session its listing calls running reports both as `Failed` (OpenSSH answers
    /// a forbidden open like a dead socket, so the message names the host's forwarding policy
    /// as a likely cause).
    fn open_unix(
        &self,
        path: &str,
    ) -> impl Future<Output = Result<Self::Stream, RemoteError>> + Send;
}

/// A program, its arguments and environment assignments. Untrusted values (session names, pane
/// ids) are only ever separate arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl RemoteCommand {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    /// The program followed by its arguments, for a caller that hands them to another program
    /// as its own argument vector (mosh-server's command). `None` for a command with
    /// environment assignments, which an argument vector cannot carry: dropping them would make
    /// the command run differently than over [`render`](Self::render), without a sign.
    pub fn argv(&self) -> Option<Vec<String>> {
        self.env.is_empty().then(|| {
            std::iter::once(self.program.clone())
                .chain(self.args.iter().cloned())
                .collect()
        })
    }

    /// `'program' 'arg' …`, or `env 'K=V' … 'program' 'arg' …` with assignments. Every token is
    /// single-quoted and an embedded `'` written as `'\''`, which bash, zsh, fish and sh read
    /// the same. Fails with [`RemoteError::Unquotable`] for a token containing a backslash or
    /// a control character, an environment name that is not `[A-Za-z_][A-Za-z0-9_]*`, or a
    /// program that is empty, starts with `-` or contains `=` (`env` would misread it).
    pub fn render(&self) -> Result<String, RemoteError> {
        let program_ok = !self.program.is_empty()
            && !self.program.starts_with('-')
            && !self.program.contains('=');
        if !program_ok {
            return Err(RemoteError::Unquotable);
        }
        let mut out = String::new();
        if !self.env.is_empty() {
            out.push_str("env");
            for (name, value) in &self.env {
                let mut chars = name.chars();
                let name_ok = chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !name_ok {
                    return Err(RemoteError::Unquotable);
                }
                out.push(' ');
                quote(&format!("{name}={value}"), &mut out)?;
            }
            out.push(' ');
        }
        quote(&self.program, &mut out)?;
        for arg in &self.args {
            out.push(' ');
            quote(arg, &mut out)?;
        }
        Ok(out)
    }
}

fn quote(token: &str, out: &mut String) -> Result<(), RemoteError> {
    if token.chars().any(|c| c == '\\' || c.is_control()) {
        return Err(RemoteError::Unquotable);
    }
    out.push('\'');
    for c in token.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    Ok(())
}

/// Renders a fixed script as `sh -c '<script>'`. Scripts are compile-time constants and must
/// contain no `'` or `\`, so no escaping is needed; anything else is
/// [`RemoteError::Unquotable`]. Never pass untrusted text here.
pub fn render_script(script: &str) -> Result<String, RemoteError> {
    if script.contains(['\'', '\\']) {
        return Err(RemoteError::Unquotable);
    }
    Ok(format!("sh -c '{script}'"))
}

#[cfg(any(test, feature = "test-support"))]
pub use local::LocalHost;

#[cfg(any(test, feature = "test-support"))]
mod local {
    use std::process::Stdio;

    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;
    use tokio::process::Command;

    use super::*;

    /// A [`RemoteHost`] over local processes and Unix sockets, for tests. `exec_rendered` runs
    /// the *rendered* line through `/bin/sh -c`, so quoting is exercised exactly as on a real
    /// host. Production code never uses it.
    #[derive(Debug, Clone)]
    pub struct LocalHost {
        timeout: Duration,
    }

    impl Default for LocalHost {
        fn default() -> Self {
            Self {
                timeout: EXEC_TIMEOUT,
            }
        }
    }

    impl LocalHost {
        pub fn new() -> Self {
            Self::default()
        }

        /// Overrides the exec timeout so timeout tests stay fast.
        pub fn with_timeout(timeout: Duration) -> Self {
            Self { timeout }
        }
    }

    fn io_error(error: std::io::Error) -> RemoteError {
        RemoteError::Io(error.to_string())
    }

    async fn read_capped(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, RemoteError> {
        let mut out = Vec::new();
        reader
            .take(OUTPUT_CAP as u64 + 1)
            .read_to_end(&mut out)
            .await
            .map_err(io_error)?;
        if out.len() > OUTPUT_CAP {
            return Err(RemoteError::OutputTooLarge);
        }
        Ok(out)
    }

    impl RemoteHost for LocalHost {
        type Stream = UnixStream;

        async fn exec_rendered(&self, rendered: &str) -> Result<ExecOutput, RemoteError> {
            let mut child = Command::new("/bin/sh")
                .arg("-c")
                .arg(rendered)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(io_error)?;
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");
            // Dropping the joined futures (cap exceeded, timeout) kills the child.
            let run = async {
                let (stdout, stderr, status) =
                    tokio::try_join!(read_capped(stdout), read_capped(stderr), async {
                        child.wait().await.map_err(io_error)
                    },)?;
                Ok(ExecOutput {
                    status: status.code().and_then(|code| u32::try_from(code).ok()),
                    stdout: stdout.into(),
                    stderr: stderr.into(),
                })
            };
            tokio::time::timeout(self.timeout, run)
                .await
                .map_err(|_| RemoteError::TimedOut)?
        }

        async fn open_unix(&self, path: &str) -> Result<UnixStream, RemoteError> {
            UnixStream::connect(path).await.map_err(io_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::*;

    fn printf(args: &[&str]) -> RemoteCommand {
        RemoteCommand::new("printf")
            .arg("[%s]")
            .args(args.iter().copied())
    }

    const NASTY: &[&str] = &[
        "plain",
        "a b",
        "it's",
        "'",
        "''",
        "$HOME",
        "${x}",
        "$(id)",
        "`id`",
        ";",
        "a;b && c | d > e",
        "\"double\"",
        "*",
        "~",
        "!x",
        "#c",
        "(x) {a,b}",
        "-n",
        "--",
        "%s",
        "",
        "é界😀",
    ];

    #[test]
    fn rendering_single_quotes_every_token() {
        let command = RemoteCommand::new("/usr/bin/tmux")
            .arg("-u")
            .arg("new-session")
            .arg("-s")
            .arg("my 'work' $x");
        assert_eq!(
            command.render().unwrap(),
            r#"'/usr/bin/tmux' '-u' 'new-session' '-s' 'my '\''work'\'' $x'"#
        );
        let with_env = RemoteCommand::new("mosh-server")
            .env("LANG", "C.UTF-8")
            .env("X", "a b")
            .arg("new");
        assert_eq!(
            with_env.render().unwrap(),
            "env 'LANG=C.UTF-8' 'X=a b' 'mosh-server' 'new'"
        );
        assert_eq!(RemoteCommand::new("true").render().unwrap(), "'true'");
    }

    #[test]
    fn rendering_rejects_what_shells_disagree_on() {
        for bad in ["a\\b", "a\nb", "a\tb", "a\u{7}b", "a\0b", "\\"] {
            assert_eq!(
                RemoteCommand::new("x").arg(bad).render(),
                Err(RemoteError::Unquotable),
                "{bad:?}"
            );
            assert_eq!(
                RemoteCommand::new("x").env("A", bad).render(),
                Err(RemoteError::Unquotable),
                "{bad:?}"
            );
        }
        for bad in ["", "-i", "a=b", "bad\\prog"] {
            assert_eq!(
                RemoteCommand::new(bad).render(),
                Err(RemoteError::Unquotable),
                "{bad:?}"
            );
        }
        for bad in ["", "1A", "A B", "A=B", "A-B", "É"] {
            assert_eq!(
                RemoteCommand::new("x").env(bad, "v").render(),
                Err(RemoteError::Unquotable),
                "{bad:?}"
            );
        }
        assert!(RemoteCommand::new("x").env("_a1", "v").render().is_ok());
    }

    #[test]
    fn scripts_render_as_sh_dash_c_and_reject_quotes_and_backslashes() {
        assert_eq!(
            render_script("command -v tmux; echo \"$HOME\"").unwrap(),
            "sh -c 'command -v tmux; echo \"$HOME\"'"
        );
        assert_eq!(render_script("it's"), Err(RemoteError::Unquotable));
        assert_eq!(render_script("a\\b"), Err(RemoteError::Unquotable));
    }

    fn find_in_path(program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }

    /// Runs `rendered` with `shell -c`, skipping the user's startup files (`zsh -f`, `fish
    /// --no-config`): they are machine state and may print.
    fn run_in_shell(shell: &Path, rendered: &str) -> Vec<u8> {
        let mut command = StdCommand::new(shell);
        match shell.file_name().and_then(|name| name.to_str()) {
            Some("zsh") => command.arg("-f"),
            Some("fish") => command.arg("--no-config"),
            _ => &mut command,
        };
        let output = command.arg("-c").arg(rendered).output().unwrap();
        assert!(
            output.status.success(),
            "{} -c {rendered}: {}",
            shell.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[test]
    fn argv_is_the_program_and_its_arguments_and_refuses_an_environment() {
        assert_eq!(
            printf(&["a b", "c"]).argv(),
            Some(vec![
                "printf".into(),
                "[%s]".into(),
                "a b".into(),
                "c".into()
            ])
        );
        assert_eq!(RemoteCommand::new("sh").env("A", "1").argv(), None);
    }

    #[test]
    fn secret_bytes_cap_grow_compare_and_hide_their_content() {
        let mut bytes = SecretBytes::new();
        assert!(bytes.is_empty());
        for chunk in [&b"abc"[..], &[b'x'; 600], b"end"] {
            bytes.extend_capped(chunk, 700).unwrap();
        }
        assert_eq!(bytes.len(), 606);
        assert_eq!(&bytes[..3], b"abc");
        assert_eq!(&bytes[603..], b"end");
        // Over the cap: refused and nothing appended.
        assert_eq!(
            bytes.extend_capped(&[0; 95], 700),
            Err(RemoteError::OutputTooLarge)
        );
        assert_eq!(bytes.len(), 606);
        bytes.extend_capped(&[0; 94], 700).unwrap();
        assert_eq!(bytes.len(), 700);
        assert_eq!(SecretBytes::from(&b"ab"[..]), b"ab");
        assert_eq!(SecretBytes::from(b"ab".to_vec()), &b"ab"[..]);
        let debug = format!("{:?}", SecretBytes::from(&b"or2-secret-debug"[..]));
        assert!(!debug.contains("or2-secret"), "{debug}");
        assert!(debug.contains("16 bytes"), "{debug}");
    }

    #[test]
    fn secret_bytes_are_wiped_when_dropped_even_through_clones_and_growth() {
        use super::wipe_log::wiped;
        let secret = b"or2-secret-drop";
        drop(SecretBytes::from(&secret[..]));
        assert_eq!(wiped(secret), 1);
        let original = SecretBytes::from(&secret[..]);
        let copy = original.clone();
        drop(original);
        assert_eq!(wiped(secret), 2);
        drop(copy);
        assert_eq!(wiped(secret), 3);
        // Growing past the capacity wipes the block that is given up, not just the last one.
        let mut grown = SecretBytes::new();
        grown.extend_capped(b"or2-secret-grow", 4096).unwrap();
        grown.extend_capped(&[b'.'; 1000], 4096).unwrap();
        assert_eq!(
            wiped(b"or2-secret-grow"),
            1,
            "the first block, wiped on growth"
        );
        drop(grown);
        assert_eq!(wiped(b"or2-secret-grow"), 2, "and the final one on drop");
    }

    #[test]
    fn rendered_commands_round_trip_argv_through_every_available_shell() {
        let expected: String = NASTY.iter().map(|arg| format!("[{arg}]")).collect();
        let rendered = printf(NASTY).render().unwrap();
        let env_command = RemoteCommand::new("sh")
            .env("A", "x y $z")
            .env("B", "it's `b`")
            .arg("-c")
            .arg("printf \"[%s][%s]\" \"$A\" \"$B\"");
        let env_rendered = env_command.render().unwrap();

        let mut shells = vec![PathBuf::from("/bin/sh")];
        let mut found = vec!["sh".to_owned()];
        // With OR2_REQUIRE_SHELLS set (CI), a missing shell fails instead of skipping, so the
        // cross-shell quoting claim is never verified vacuously.
        let required = std::env::var_os("OR2_REQUIRE_SHELLS").is_some();
        for name in ["bash", "zsh", "fish"] {
            match find_in_path(name) {
                Some(path) => {
                    shells.push(path);
                    found.push(name.to_owned());
                }
                None => assert!(!required, "OR2_REQUIRE_SHELLS is set but {name} is missing"),
            }
        }
        eprintln!("quoting round-trip shells: {}", found.join(", "));
        for shell in shells {
            let out = run_in_shell(&shell, &rendered);
            assert_eq!(
                String::from_utf8(out).unwrap(),
                expected,
                "{}",
                shell.display()
            );
            let out = run_in_shell(&shell, &env_rendered);
            assert_eq!(
                String::from_utf8(out).unwrap(),
                "[x y $z][it's `b`]",
                "{}",
                shell.display()
            );
        }
    }

    #[tokio::test]
    async fn local_host_runs_the_rendered_command_and_reports_status_and_streams() {
        let host = LocalHost::new();
        let out = host.exec(&printf(NASTY)).await.unwrap();
        let expected: String = NASTY.iter().map(|arg| format!("[{arg}]")).collect();
        assert_eq!(String::from_utf8(out.stdout.to_vec()).unwrap(), expected);
        assert!(out.success() && out.stderr.is_empty());

        let failing = RemoteCommand::new("sh")
            .arg("-c")
            .arg("echo out; echo err >&2; exit 3");
        let out = host.exec(&failing).await.unwrap();
        assert_eq!(out.status, Some(3));
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert!(!out.success());

        let killed = RemoteCommand::new("sh").arg("-c").arg("kill -9 $$");
        assert_eq!(host.exec(&killed).await.unwrap().status, None);

        let missing = host
            .exec(&RemoteCommand::new("or2-no-such-program"))
            .await
            .unwrap();
        assert_eq!(missing.status, Some(127));

        assert_eq!(
            host.exec(&RemoteCommand::new("x").arg("a\\b")).await,
            Err(RemoteError::Unquotable),
            "nothing runs when rendering fails"
        );
    }

    #[tokio::test]
    async fn local_host_exec_runs_fixed_scripts() {
        let host = LocalHost::new();
        let script = render_script("echo \"$((1 + 2))\"").unwrap();
        let out = host.exec_rendered(&script).await.unwrap();
        assert_eq!(out.stdout, b"3\n");
        // A multi-line script runs through the trait, no `LocalHost` inherent method needed.
        let out = host
            .exec_script("a=1\nb=2\necho \"$((a + b))\"")
            .await
            .unwrap();
        assert_eq!(out.stdout, b"3\n");
        assert_eq!(
            host.exec_script("echo 'quoted'").await,
            Err(RemoteError::Unquotable)
        );
    }

    #[tokio::test]
    async fn local_host_enforces_the_output_cap_per_stream() {
        let host = LocalHost::new();
        let emit = |bytes: usize, fd: &str| {
            RemoteCommand::new("sh")
                .arg("-c")
                .arg(format!("head -c {bytes} /dev/zero{fd}"))
        };
        let out = host.exec(&emit(OUTPUT_CAP, "")).await.unwrap();
        assert_eq!(out.stdout.len(), OUTPUT_CAP);
        assert_eq!(
            host.exec(&emit(OUTPUT_CAP + 1, "")).await,
            Err(RemoteError::OutputTooLarge)
        );
        assert_eq!(
            host.exec(&emit(OUTPUT_CAP + 1, " >&2")).await,
            Err(RemoteError::OutputTooLarge)
        );
    }

    #[tokio::test]
    async fn local_host_times_out_and_stops_the_command() {
        let host = LocalHost::with_timeout(Duration::from_millis(200));
        let started = std::time::Instant::now();
        let result = host.exec(&RemoteCommand::new("sleep").arg("30")).await;
        assert_eq!(result, Err(RemoteError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn local_host_opens_unix_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(&buf).await.unwrap();
        });
        let host = LocalHost::new();
        let mut stream = host.open_unix(path.to_str().unwrap()).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        stream.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");
        server.await.unwrap();

        let missing = dir.path().join("missing.sock");
        assert!(matches!(
            host.open_unix(missing.to_str().unwrap()).await,
            Err(RemoteError::Io(_))
        ));
    }
}
