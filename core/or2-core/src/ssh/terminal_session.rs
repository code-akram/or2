//! One terminal session on a host connection: a PTY channel running the login shell, tmux or
//! herdr (contracts.md, "Terminal sessions on a host").
//!
//! The session lifecycle of every terminal, on a channel of the host's connection. The caller's
//! thread (`or2-terminal`) runs [`drive`], which owns the libghostty engine and the
//! [`SessionDriver`]; [`channel_task`] on the network runtime owns the SSH channel. They talk
//! through the shared [`TerminalPump`] / [`pump_channel`] event and write queues.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

use super::connection::{SshHost, closed_reason};
use super::pump::{
    CHANNEL_CLOSE_GRACE, Event, TerminalPump, Write, connection_error, internal, pump_channel,
    request_accepted,
};
use crate::herdr::HerdrError;
use crate::host::TerminalTarget;
use crate::remote::{RemoteCommand, RemoteError};
use crate::session::{CloseReason, Command, SessionDriver, SessionFailure};
use crate::term::TerminalSize;
use crate::tmux;

/// A channel that ended without an exit status is `ConnectionLost`; when the whole host went
/// down that is the host's failure, which the driver announces a moment later. Wait this long
/// for it so the session reports the specific cause.
const HOST_REASON_GRACE: Duration = Duration::from_millis(250);

/// Runs one terminal session to its close. `closing` carries the reason when the host closes.
pub(super) async fn drive(
    host: Arc<SshHost>,
    target: TerminalTarget,
    size: TerminalSize,
    mut driver: SessionDriver,
    mut closing: watch::Receiver<Option<CloseReason>>,
) {
    let (mut pump, outgoing, latest_size) = match TerminalPump::new(size) {
        Ok(parts) => parts,
        Err(failure) => {
            driver.close(CloseReason::Failed(failure));
            return;
        }
    };
    let (events, mut incoming) = mpsc::channel(32);
    let (shutdown, stop) = watch::channel(false);
    let client_id = driver.client_id().map(str::to_owned);
    // Dropped (released) when this returns, after the session's `Closed`.
    let _release = host.tmux_release(client_id.as_deref(), &closing);
    let mut network = tokio::spawn(async move {
        let reason = channel_task(
            host,
            target,
            client_id,
            &events,
            outgoing,
            latest_size,
            stop,
        )
        .await;
        let _ = events.send(Event::Closed(reason)).await;
    });
    let mut pending_event = None;
    let outcome: Result<CloseReason, SessionFailure> = async {
        loop {
            tokio::select! {
                Some(event) = async {
                    if pending_event.is_some() { pending_event.take() } else { incoming.recv().await }
                } => match event {
                    Event::Connected => pump.connect(&mut driver)?,
                    Event::Output(bytes) => {
                        pump.output(&mut driver, bytes, &mut incoming, &mut pending_event)?;
                    }
                    Event::Closed(reason) => break Ok(host_reason(reason, &mut closing).await),
                },
                () = pump.enter_pending() => pump.enter_due(&mut driver)?,
                command = driver.next_command() => match command {
                    Command::Disconnect => break Ok(CloseReason::Disconnected),
                    command => pump.command(&mut driver, command)?,
                },
                reason = closed_reason(&mut closing) => break Ok(reason),
            }
        }
    }
    .await;
    let reason = outcome.unwrap_or_else(CloseReason::Failed);
    // Whatever ended the session, close its channel (best effort, bounded) before the
    // network task goes: an orphaned channel would keep its program running on the host.
    shutdown.send_replace(true);
    let _ = timeout(CHANNEL_CLOSE_GRACE * 2, &mut network).await;
    network.abort();
    if !network.is_finished() {
        let _ = network.await;
    }
    driver.close(reason);
}

/// A channel lost without an exit status gets the host's own failure when the host closed too.
async fn host_reason(
    reason: CloseReason,
    closing: &mut watch::Receiver<Option<CloseReason>>,
) -> CloseReason {
    if matches!(
        reason,
        CloseReason::Failed(SessionFailure::ConnectionLost(_))
    ) && let Ok(host) = timeout(HOST_REASON_GRACE, closed_reason(closing)).await
    {
        return host;
    }
    reason
}

/// Resolves once the session thread asks the channel to shut down.
async fn stopped(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopping| *stopping).await;
}

/// What a terminal target needs before it can start: the command that runs it on a PTY (`None`
/// for the login shell), and the pane focus a herdr pane owes first.
pub(super) struct Planned {
    pub(super) command: Option<RemoteCommand>,
    pub(super) focus: Option<PaneFocus>,
}

/// A herdr pane to focus before the terminal on it shows anything, as a step of its own so the
/// caller can run it beside the channel open or the mosh bootstrap (they do not depend on it:
/// the herdr client follows herdr's focus) and still fail the terminal if it fails.
pub(super) struct PaneFocus {
    herdr: String,
    session: Option<String>,
    pane_id: String,
}

impl PaneFocus {
    /// Focuses the pane, joining the app's own focus if it is in flight and accepting one it
    /// acknowledged a moment ago (the connection's [`herdr::FocusGate`]).
    pub(super) async fn run(&self, host: &Arc<SshHost>) -> Result<(), SessionFailure> {
        host.focus_pane(&self.herdr, self.session.as_deref(), &self.pane_id, true)
            .await
            .map_err(herdr_failure)
    }
}

/// The command that runs `target` on a PTY, or `None` for the login shell, and the pane focus
/// it owes. A missing program is `NotInstalled`. Shared with the mosh terminal, which runs the
/// same command as mosh-server's.
pub(super) async fn plan(
    host: &SshHost,
    target: &TerminalTarget,
    client_id: Option<&str>,
) -> Result<Planned, SessionFailure> {
    let (command, focus) = match target {
        TerminalTarget::Shell => (None, None),
        TerminalTarget::ShellIn { path } => (Some(crate::directories::shell_command(path)), None),
        TerminalTarget::Tmux { session_name } => {
            let programs = host.programs().await.map_err(remote_failure)?;
            let path = programs
                .tmux
                .as_deref()
                .ok_or_else(|| not_installed("tmux"))?;
            // The client-recording step only for a tmux that takes it (2.6 and later, from the
            // program probe's `tmux -V`): an older tmux would reject the whole command list,
            // attach included. Without it the terminal's session moves do nothing.
            let client_id = client_id.filter(|_| programs.tmux_records_clients);
            (
                Some(tmux::attach_command(path, session_name, client_id)),
                None,
            )
        }
        TerminalTarget::Herdr { session, pane_id } => {
            let path = host
                .programs()
                .await
                .map_err(remote_failure)?
                .herdr
                .as_deref()
                .ok_or_else(|| not_installed("herdr"))?;
            let focus = pane_id.as_ref().map(|pane_id| PaneFocus {
                herdr: path.to_owned(),
                session: session.clone(),
                pane_id: pane_id.clone(),
            });
            let command = RemoteCommand::new(path);
            let command = match session {
                Some(name) => command.args(["--session", name]),
                None => command,
            };
            (Some(command), focus)
        }
    };
    Ok(Planned { command, focus })
}

/// The server refusing a session channel (OpenSSH's `MaxSessions`, 10 per connection by
/// default, counts every terminal and every exec in flight) leaves the connection healthy:
/// `ShellRejected`, never `ConnectionLost`.
fn open_failure(error: russh::Error) -> SessionFailure {
    match error {
        russh::Error::ChannelOpenFailure(_) => SessionFailure::ShellRejected,
        error => connection_error(error),
    }
}

pub(super) fn not_installed(program: &str) -> SessionFailure {
    SessionFailure::NotInstalled {
        program: program.into(),
    }
}

pub(super) fn remote_failure(error: RemoteError) -> SessionFailure {
    match error {
        RemoteError::Closed => SessionFailure::ConnectionLost("the host connection closed".into()),
        RemoteError::TimedOut => SessionFailure::TimedOut,
        error => SessionFailure::CommandFailed(error.to_string()),
    }
}

fn herdr_failure(error: HerdrError) -> SessionFailure {
    match error {
        HerdrError::Remote(error) => remote_failure(error),
        error => SessionFailure::CommandFailed(error.to_string()),
    }
}

/// Advertise the renderer's RGB support without depending on sshd's AcceptEnv policy.
/// A plain shell still runs the user's login startup files; target arguments stay quoted.
/// Kept on the SSH path: adding environment to the shared plan would break mosh's argv handoff.
fn ssh_command(command: Option<RemoteCommand>) -> Result<String, RemoteError> {
    command
        .unwrap_or_else(|| RemoteCommand::new("sh").args(["-c", "exec \"${SHELL:-/bin/sh}\" -l"]))
        .env("COLORTERM", "truecolor")
        .render()
}

/// Opens the PTY channel, starts the program and pumps it. Returns why it ended; `Disconnected`
/// only when asked to stop, after closing the channel.
pub(super) async fn channel_task(
    host: Arc<SshHost>,
    target: TerminalTarget,
    client_id: Option<String>,
    events: &mpsc::Sender<Event>,
    mut writes: mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
    stop: watch::Receiver<bool>,
) -> CloseReason {
    // Everything before the channel exists (the probe) is abandoned on stop.
    let planned = tokio::select! {
        planned = plan(&host, &target, client_id.as_deref()) => match planned {
            Ok(planned) => planned,
            Err(failure) => return CloseReason::Failed(failure),
        },
        () = stopped(stop.clone()) => return CloseReason::Disconnected,
    };
    let Planned { command, focus } = planned;
    let command = match ssh_command(command) {
        Ok(line) => line,
        Err(error) => return CloseReason::Failed(internal(error)),
    };
    // The server may never answer the channel open, `pty-req` or the program request, and
    // keepalives alone do not end that: bound the whole setup like an exec.
    let limit = host.exec_timeout();
    // The pane focus (herdr) and the channel open do not depend on each other, so they cost one
    // round trip together instead of two. A focus that fails closes the channel again.
    let focus_step = async {
        match &focus {
            Some(focus) => focus.run(&host).await,
            None => Ok(()),
        }
    };
    // The open belongs to the host connection (`SshHost::start_open`): whatever this task does
    // with it, a channel the server confirms after we gave up is closed by the connection, not
    // leaked on it (a raw channel does not close itself when dropped, and a leaked one counts
    // against the server's `MaxSessions`). Its result is kept outside the join, so a stop that
    // drops the join while the focus or the open is still pending still finds an answer that
    // arrived meanwhile.
    let mut pending = host.start_open();
    let opening = std::sync::Mutex::new(None);
    let mut open_step = std::pin::pin!(async {
        let opened = timeout(limit, pending.wait()).await;
        *opening.lock().unwrap() = Some(opened);
    });
    let joined = tokio::select! {
        (focused, ()) = async { tokio::join!(focus_step, &mut open_step) } => Some(focused),
        () = stopped(stop.clone()) => None,
    };
    let focused = match joined {
        Some(focused) => focused,
        None => {
            // The confirmation may be in flight to us: give it a moment so the channel is closed
            // before this session reports `Closed`. One that arrives later is closed by the
            // connection when `pending` goes (below).
            if opening.lock().unwrap().is_none() {
                let _ = timeout(CHANNEL_CLOSE_GRACE, open_step.as_mut()).await;
            }
            let accepted = opening.lock().unwrap().take();
            if let Some(Ok(Ok(channel))) = accepted {
                let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
            }
            return CloseReason::Disconnected;
        }
    };
    let opened = opening.lock().unwrap().take();
    let mut channel = match opened {
        Some(Ok(Ok(channel))) => channel,
        Some(Ok(Err(error))) => return CloseReason::Failed(open_failure(error)),
        Some(Err(_)) | None => return CloseReason::Failed(SessionFailure::TimedOut),
    };
    if let Err(failure) = focused {
        let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
        return CloseReason::Failed(failure);
    }
    // From here on a channel exists: every exit closes it, an abort of this task included (the
    // guard closes it when dropped; `close` and `into_inner` are the deliberate ways out).
    let started = tokio::select! {
        started = timeout(limit, start_program(&mut channel, &command, events, &size)) => {
            started.unwrap_or(Err(SessionFailure::TimedOut))
        }
        () = stopped(stop.clone()) => {
            let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
            return CloseReason::Disconnected;
        }
    };
    if let Err(failure) = started {
        let _ = timeout(CHANNEL_CLOSE_GRACE, channel.close()).await;
        return CloseReason::Failed(failure);
    }
    let _ = events.send(Event::Connected).await;
    match pump_channel(channel, events, &mut writes, stopped(stop)).await {
        Ok(reason) => reason,
        Err(failure) => CloseReason::Failed(failure),
    }
}

/// `pty-req` at the current size, then the command (a login-shell wrapper for plain shells),
/// then any resize meanwhile (latest wins, so the PTY is at the right size before `Connected`).
async fn start_program(
    channel: &mut russh::Channel<russh::client::Msg>,
    command: &str,
    events: &mpsc::Sender<Event>,
    size: &watch::Receiver<TerminalSize>,
) -> Result<(), SessionFailure> {
    let initial = *size.borrow();
    channel
        .request_pty(
            true,
            "xterm-256color",
            u32::from(initial.columns()),
            u32::from(initial.rows()),
            0,
            0,
            &[],
        )
        .await
        .map_err(connection_error)?;
    request_accepted(channel, events).await?;
    channel
        .exec(true, command)
        .await
        .map_err(connection_error)?;
    request_accepted(channel, events).await?;
    let latest = *size.borrow();
    if latest != initial {
        channel
            .window_change(u32::from(latest.columns()), u32::from(latest.rows()), 0, 0)
            .await
            .map_err(connection_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_login_shell_advertises_truecolor_and_remains_a_login_shell() {
        assert_eq!(
            ssh_command(None).unwrap(),
            "env 'COLORTERM=truecolor' 'sh' '-c' 'exec \"${SHELL:-/bin/sh}\" -l'"
        );
    }

    #[test]
    fn ssh_targets_keep_literal_arguments_and_gain_truecolor() {
        let command = RemoteCommand::new("/opt/bin/herdr").args(["--session", "it's"]);
        assert_eq!(
            ssh_command(Some(command)).unwrap(),
            "env 'COLORTERM=truecolor' '/opt/bin/herdr' '--session' 'it'\\''s'"
        );
    }
}
