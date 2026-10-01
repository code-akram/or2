//! Real SSH over [`Transport`](crate::transport::Transport). Network futures run on a
//! process-wide runtime; each driver and its callbacks stay on one dedicated thread, which
//! also owns the terminal it drives.
//!
//! * [`connect_host`] (M2): one connection per host carrying terminals, exec queries and
//!   streamlocal channels. See [`connection`].
//! * [`connect`] (M1 path): one connection, one shell. Removed when lane B lands.
//!
//! Both build on [`client`] (host-key handler, relay, authentication) and [`pump`] (the
//! terminal pump shared by an M1 shell and a terminal channel on a host).

mod client;
mod connection;
mod pump;
mod terminal_session;

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use russh::client::{self as russh_client, Handle};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep_until};

use crate::host::{HostConnectRequest, HostHandle, HostObserver};
use crate::session::{
    CloseReason, Command, ConnectRequest, SessionDriver, SessionFailure, SessionHandle,
    SessionObserver, SessionState, channel,
};
use crate::term::TerminalSize;
use crate::transport::{DirectTcp, Transport};
use client::{Client, HostKeyRequest, TransportEnd, authenticate, handshake_failure, relay};
use pump::{
    Event, TerminalPump, Write, connection_error, internal, pump_channel, request_accepted,
};

pub use connection::HostOptions;
#[cfg(any(test, feature = "test-support"))]
pub use connection::{SshRemote, connect_tapped};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("or2-network")
            .build()
            .expect("create SSH runtime")
    })
}

/// M1 path: removed when lane B lands.
pub fn connect(request: ConnectRequest, observer: Arc<dyn SessionObserver>) -> SessionHandle {
    start(request, observer, CONNECT_TIMEOUT)
}

/// Connects to a host over TCP: races its addresses, verifies the host key, authenticates and
/// serves terminals, queries and herdr watches until closed. Returns at once; the state
/// changes and the final `Closed` arrive through `observer` on a Rust-owned thread.
pub fn connect_host(request: HostConnectRequest, observer: Arc<dyn HostObserver>) -> HostHandle {
    connect_host_with(
        Arc::new(DirectTcp),
        request,
        observer,
        HostOptions::default(),
    )
}

/// [`connect_host`] over any transport and with explicit timings, for tests.
pub fn connect_host_with<T: Transport>(
    transport: Arc<T>,
    request: HostConnectRequest,
    observer: Arc<dyn HostObserver>,
    options: HostOptions,
) -> HostHandle {
    connection::start(transport, request, observer, options)
}

fn start(
    request: ConnectRequest,
    observer: Arc<dyn SessionObserver>,
    timeout: Duration,
) -> SessionHandle {
    let (handle, mut driver) = channel(observer);
    // Initialize before spawning so runtime initialization is never done in a callback.
    let runtime = runtime();
    std::thread::Builder::new()
        .name("or2-session".into())
        .spawn(move || runtime.block_on(drive(request, &mut driver, timeout)))
        .expect("create session thread");
    handle
}

impl From<HostKeyRequest> for Event {
    fn from(request: HostKeyRequest) -> Self {
        Event::HostKey(request.prompt, request.reply)
    }
}

impl From<TransportEnd> for Event {
    fn from(end: TransportEnd) -> Self {
        Event::TransportEnded(end.0)
    }
}

async fn drive(request: ConnectRequest, driver: &mut SessionDriver, timeout: Duration) {
    let (events, mut incoming) = mpsc::channel(32);
    let (mut pump, outgoing, latest_size) = match TerminalPump::new(request.size) {
        Ok(parts) => parts,
        Err(failure) => {
            driver.close(CloseReason::Failed(failure));
            return;
        }
    };
    let (shutdown, stop) = watch::channel(false);
    let mut network = tokio::spawn(async move {
        let reason = network(request, events.clone(), outgoing, latest_size, stop).await;
        let _ = events.send(Event::Closed(reason)).await;
    });
    let mut decision = None;
    let mut deadline = Instant::now() + timeout;
    let mut remaining = timeout;
    let mut timing = true;
    let mut pending_event = None;
    let outcome: Result<CloseReason, SessionFailure> = async { loop {
        tokio::select! {
            Some(event) = async {
                if pending_event.is_some() { pending_event.take() } else { incoming.recv().await }
            } => match event {
                Event::HostKey(prompt, reply) => {
                    remaining = deadline.saturating_duration_since(Instant::now());
                    timing = false; // Human confirmation is bounded by the server, not us.
                    decision = Some(reply);
                    driver.transition(SessionState::AwaitingHostKey(prompt)).map_err(internal)?;
                }
                Event::Authenticating => {
                    driver.transition(SessionState::Authenticating).map_err(internal)?;
                }
                Event::Connected => {
                    timing = false;
                    pump.connect(driver)?;
                }
                Event::Output(bytes) => {
                    pump.output(driver, bytes, &mut incoming, &mut pending_event)?;
                }
                Event::TransportEnded(failure) => {
                    if decision.is_some() { break Ok(CloseReason::Failed(failure)); }
                }
                Event::Closed(reason) => break Ok(reason),
            },
            command = driver.next_command() => match command {
                Command::Disconnect => break Ok(CloseReason::Disconnected),
                Command::RejectHostKey => break Ok(CloseReason::Failed(SessionFailure::HostKeyRejected)),
                Command::ApproveHostKey { .. } => {
                    if let Some(reply) = decision.take() {
                        let _ = reply.send(true);
                        deadline = Instant::now() + remaining;
                        timing = true;
                    }
                }
                command => pump.command(driver, command)?,
            },
            () = sleep_until(deadline), if timing => break Ok(CloseReason::Failed(SessionFailure::TimedOut)),
        }
    }}.await;
    let reason = outcome.unwrap_or_else(CloseReason::Failed);
    if reason == CloseReason::Disconnected {
        shutdown.send_replace(true);
        // Let russh flush a best-effort disconnect, but never wait indefinitely for a peer.
        let _ = tokio::time::timeout(Duration::from_millis(250), &mut network).await;
    }
    network.abort();
    if !network.is_finished() {
        let _ = network.await;
    }
    driver.close(reason);
    // Cancellation drops the relay's actual transport, including during a host-key prompt.
}

async fn network(
    request: ConnectRequest,
    events: mpsc::Sender<Event>,
    mut writes: mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
    stop: watch::Receiver<bool>,
) -> CloseReason {
    let stream = match DirectTcp.connect(&request.endpoint).await {
        Ok(stream) => stream,
        Err(error) => {
            return CloseReason::Failed(SessionFailure::Unreachable(format!(
                "TCP connection failed ({:?}): {error}",
                error.kind()
            )));
        }
    };
    // russh awaits check_server_key inside its reader. Relay the transport through a bounded
    // pipe so EOF is still observable while that callback waits for human confirmation.
    let (ssh_stream, relay) = relay(stream, events.clone());
    tokio::pin!(relay);
    let result = tokio::select! {
        biased;
        result = shell(request, &events, &mut writes, size, ssh_stream, stop) => {
            if matches!(result, Ok(CloseReason::Disconnected)) {
                // russh flushed into the duplex pipe; also drain that pipe to the transport.
                let _ = tokio::time::timeout(Duration::from_millis(75), &mut relay).await;
            }
            result
        },
        result = &mut relay => Err(match result {
            Ok(_) => pump::lost(),
            Err(error) => connection_error(russh::Error::IO(error)),
        }),
    };
    match result {
        Ok(reason) => reason,
        Err(failure) => CloseReason::Failed(failure),
    }
}

async fn shell(
    request: ConnectRequest,
    events: &mpsc::Sender<Event>,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
    stream: tokio::io::DuplexStream,
    mut stop: watch::Receiver<bool>,
) -> Result<CloseReason, SessionFailure> {
    let mut handle = russh_client::connect_stream(
        client::config(),
        stream,
        Client::new(request.trusted_host_keys.clone(), events.clone()),
    )
    .await
    .map_err(handshake_failure)?;
    let result = tokio::select! {
        result = authenticated_shell(&mut handle, request, events, writes, size) => result,
        _ = stop.changed() => {
            let _ = handle.disconnect(russh::Disconnect::ByApplication, "session closed", "").await;
            let _ = tokio::time::timeout(Duration::from_millis(150), handle).await;
            return Ok(CloseReason::Disconnected);
        }
    };
    if matches!(result, Err(SessionFailure::ConnectionLost(_)))
        && handle.is_closed()
        && let Err(error) = handle.await
    {
        return Err(SessionFailure::ConnectionLost(format!("SSH: {error}")));
    }
    result
}

async fn authenticated_shell(
    handle: &mut Handle<Client<Event>>,
    request: ConnectRequest,
    events: &mpsc::Sender<Event>,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
) -> Result<CloseReason, SessionFailure> {
    let _ = events.send(Event::Authenticating).await;
    authenticate(handle, request.username.clone(), &request.key).await?;
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(connection_error)?;
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
    request_accepted(&mut channel, events).await?;
    channel
        .request_shell(true)
        .await
        .map_err(connection_error)?;
    request_accepted(&mut channel, events).await?;
    // Resizes during authentication/PTY setup still win before Connected.
    let latest = *size.borrow();
    if latest != initial {
        channel
            .window_change(u32::from(latest.columns()), u32::from(latest.rows()), 0, 0)
            .await
            .map_err(connection_error)?;
    }
    #[cfg(test)]
    tests::before_connected(request.endpoint.port(), size.clone()).await;
    let _ = events.send(Event::Connected).await;
    // Dropping the whole connection closes the channel, so M1 never asks to shut it down.
    pump_channel(channel, events, writes, std::future::pending()).await
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
