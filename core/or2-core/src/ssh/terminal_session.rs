//! One terminal session on a host connection: a PTY channel running the login shell, tmux or
//! herdr (contracts.md, "Terminal sessions on a host").
//!
//! Exactly M1's session semantics on a channel instead of a whole connection. The caller's
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
use crate::herdr::{self, HerdrError};
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
    let mut network = tokio::spawn(async move {
        let reason = channel_task(host, target, &events, outgoing, latest_size, stop).await;
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

/// The command line that runs `target` on a PTY, or `None` for the login shell. A missing
/// program is `NotInstalled`; a failed pane focus is `CommandFailed`.
async fn program(
    host: &SshHost,
    target: &TerminalTarget,
) -> Result<Option<String>, SessionFailure> {
    let command = match target {
        TerminalTarget::Shell => return Ok(None),
        TerminalTarget::Tmux { session_name } => {
            let path = host
                .capabilities()
                .await
                .map_err(remote_failure)?
                .tmux
                .as_deref()
                .ok_or_else(|| not_installed("tmux"))?;
            tmux::attach_command(path, session_name)
        }
        TerminalTarget::Herdr { session, pane_id } => {
            let path = host
                .capabilities()
                .await
                .map_err(remote_failure)?
                .herdr
                .as_deref()
                .ok_or_else(|| not_installed("herdr"))?;
            if let Some(pane_id) = pane_id {
                herdr::focus_pane(host, path, session.as_deref(), pane_id)
                    .await
                    .map_err(herdr_failure)?;
            }
            let command = RemoteCommand::new(path);
            match session {
                Some(name) => command.args(["--session", name]),
                None => command,
            }
        }
    };
    command.render().map(Some).map_err(internal)
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

fn not_installed(program: &str) -> SessionFailure {
    SessionFailure::NotInstalled {
        program: program.into(),
    }
}

fn remote_failure(error: RemoteError) -> SessionFailure {
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

/// Opens the PTY channel, starts the program and pumps it. Returns why it ended; `Disconnected`
/// only when asked to stop, after closing the channel.
async fn channel_task(
    host: Arc<SshHost>,
    target: TerminalTarget,
    events: &mpsc::Sender<Event>,
    mut writes: mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
    stop: watch::Receiver<bool>,
) -> CloseReason {
    // Everything before the channel exists (probe, pane focus) is abandoned on stop.
    let command = tokio::select! {
        command = program(&host, &target) => match command {
            Ok(command) => command,
            Err(failure) => return CloseReason::Failed(failure),
        },
        () = stopped(stop.clone()) => return CloseReason::Disconnected,
    };
    // The server may never answer the channel open, `pty-req` or the program request, and
    // keepalives alone do not end that: bound the whole setup like an exec.
    let limit = host.exec_timeout();
    let mut channel = tokio::select! {
        channel = timeout(limit, host.open_channel()) => match channel {
            Ok(Ok(channel)) => channel,
            Ok(Err(error)) => return CloseReason::Failed(open_failure(error)),
            Err(_) => return CloseReason::Failed(SessionFailure::TimedOut),
        },
        () = stopped(stop.clone()) => return CloseReason::Disconnected,
    };
    // From here on a channel exists: every exit closes it.
    let started = tokio::select! {
        started = timeout(limit, start_program(&mut channel, command.as_deref(), events, &size)) => {
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

/// `pty-req` at the current size, then the shell or the command, then any resize that
/// happened meanwhile (latest wins, so the PTY is at the right size before `Connected`).
async fn start_program(
    channel: &mut russh::Channel<russh::client::Msg>,
    command: Option<&str>,
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
    match command {
        None => channel.request_shell(true).await,
        Some(line) => channel.exec(true, line).await,
    }
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
