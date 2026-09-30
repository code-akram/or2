//! Real SSH sessions over Transport. Network futures run on a process-wide runtime; the
//! driver and callbacks stay on one dedicated thread (also the terminal's owner in M1).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use russh::client;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tokio::io::AsyncWriteExt;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, sleep_until};

use crate::session::{
    CloseReason, Command, ConnectRequest, HostKeyPrompt, SessionDriver, SessionFailure,
    SessionHandle, SessionObserver, SessionState, channel,
};
use crate::term::TerminalSize;
use crate::terminal::TerminalEngine;
use crate::transport::{DirectTcp, Transport};
use crate::trust::{HostKey, HostKeyVerdict, verify};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("or2-network")
            .build()
            .expect("create SSH runtime")
    })
}

pub fn connect(request: ConnectRequest, observer: Arc<dyn SessionObserver>) -> SessionHandle {
    start(request, observer, CONNECT_TIMEOUT)
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

enum Event {
    HostKey(HostKeyPrompt, oneshot::Sender<bool>),
    Authenticating,
    Connected,
    Output(Vec<u8>),
    TransportEnded(SessionFailure),
    Closed(CloseReason),
}

enum Write {
    Bytes(Vec<u8>),
    Resize(TerminalSize),
}

struct Client {
    trusted: Vec<HostKey>,
    events: mpsc::Sender<Event>,
    checked: bool,
}

#[derive(Debug, thiserror::Error)]
enum ClientError {
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    #[error("host certificates are unsupported")]
    Certificate,
}

impl client::Handler for Client {
    type Error = ClientError;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Err(ClientError::Certificate);
        };
        let presented = HostKey::from_public_key(key.clone());
        if verify(&presented, &self.trusted) == HostKeyVerdict::Trusted {
            self.checked = true;
            return Ok(true);
        }
        // Rekey must not silently change the peer identity or prompt from Connected.
        if self.checked {
            return Ok(false);
        }
        self.checked = true;
        let (reply, decision) = oneshot::channel();
        self.events
            .send(Event::HostKey(
                HostKeyPrompt {
                    presented: presented.clone(),
                    previously_trusted: self.trusted.clone(),
                },
                reply,
            ))
            .await
            .map_err(|_| russh::Error::Disconnect)?;
        let approved = decision.await.unwrap_or(false);
        if approved {
            self.trusted.push(presented);
        }
        Ok(approved)
    }
}

async fn drive(request: ConnectRequest, driver: &mut SessionDriver, timeout: Duration) {
    let (events, mut incoming) = mpsc::channel(32);
    let (writes, outgoing) = mpsc::unbounded_channel();
    let (size, latest_size) = watch::channel(request.size);
    let replies = writes.clone();
    let mut terminal = match TerminalEngine::new(request.size, move |bytes| {
        let _ = replies.send(Write::Bytes(bytes.to_vec()));
    }) {
        Ok(terminal) => terminal,
        Err(error) => {
            driver.close(CloseReason::Failed(internal(error)));
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
    let mut connected = false;
    let outcome: Result<CloseReason, SessionFailure> = async { loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
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
                    connected = true;
                    driver.transition(SessionState::Connected).map_err(internal)?;
                    publish(driver, &mut terminal)?;
                }
                Event::Output(bytes) => {
                    terminal.write(&bytes);
                    if connected { publish(driver, &mut terminal)?; }
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
                Command::Resize(new_size) => {
                    size.send_replace(new_size);
                    terminal.resize(new_size).map_err(internal)?;
                    if connected {
                        let _ = writes.send(Write::Resize(new_size));
                        publish(driver, &mut terminal)?;
                    }
                }
                Command::Text(text) => { let _ = writes.send(Write::Bytes(crate::input::text_bytes(&text))); }
                Command::Key(key) => {
                    let bytes = terminal.encode_key(&key).map_err(internal)?;
                    let _ = writes.send(Write::Bytes(bytes));
                }
                Command::Scroll(scroll) => {
                    let bytes = terminal.scroll(scroll).map_err(internal)?;
                    if !bytes.is_empty() { let _ = writes.send(Write::Bytes(bytes)); }
                    publish(driver, &mut terminal)?;
                }
                Command::FullFrame => {
                    terminal.request_full_frame();
                    publish(driver, &mut terminal)?;
                }
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
    let (ssh_stream, pipe) = tokio::io::duplex(65536);
    let (mut read, mut write) = tokio::io::split(stream);
    let (mut pipe_read, mut pipe_write) = tokio::io::split(pipe);
    let relay = async {
        tokio::try_join!(
            async {
                let result = tokio::io::copy(&mut read, &mut pipe_write).await;
                let _ = pipe_write.shutdown().await;
                let failure = match &result {
                    Ok(_) => lost(),
                    Err(error) => SessionFailure::ConnectionLost(format!(
                        "SSH transport ({:?}): {error}",
                        error.kind()
                    )),
                };
                let _ = events.send(Event::TransportEnded(failure)).await;
                result
            },
            tokio::io::copy(&mut pipe_read, &mut write),
        )
    };
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
            Ok(_) => lost(),
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
    let config = client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    };
    let mut handle = client::connect_stream(
        Arc::new(config),
        stream,
        Client {
            trusted: request.trusted_host_keys,
            events: events.clone(),
            checked: false,
        },
    )
    .await
    .map_err(|error| match error {
        ClientError::Certificate => {
            SessionFailure::UnsupportedHostKey("host certificates are unsupported".into())
        }
        ClientError::Ssh(
            error @ russh::Error::NoCommonAlgo {
                kind: russh::AlgorithmKind::Key,
                ..
            },
        ) => SessionFailure::UnsupportedHostKey(format!("SSH host key: {error}")),
        ClientError::Ssh(
            error @ (russh::Error::IO(_) | russh::Error::Disconnect | russh::Error::HUP),
        ) => connection_error(error),
        ClientError::Ssh(error) => SessionFailure::Protocol(format!("SSH handshake: {error}")),
    })?;
    let result = tokio::select! {
        result = authenticated_shell(&mut handle, request.username, request.key, events, writes, size) => result,
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
    handle: &mut client::Handle<Client>,
    username: String,
    key: crate::keys::ClientKey,
    events: &mpsc::Sender<Event>,
    writes: &mut mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
) -> Result<CloseReason, SessionFailure> {
    let _ = events.send(Event::Authenticating).await;
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(connection_error)?
        .flatten();
    let auth = handle
        .authenticate_publickey(
            username,
            PrivateKeyWithHashAlg::new(Arc::new(key.private_key().clone()), hash),
        )
        .await
        .map_err(connection_error)?;
    if !auth.success() {
        return Err(SessionFailure::AuthenticationRejected);
    }
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
    let _ = events.send(Event::Connected).await;
    let mut exit_status = None;
    let mut exit_signal = false;
    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = events.send(Event::Output(data.to_vec())).await;
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(russh::ChannelMsg::ExitSignal { .. }) => exit_signal = true,
                Some(russh::ChannelMsg::Close) | None => return if exit_status.is_some() || exit_signal {
                    Ok(CloseReason::RemoteExited { exit_status })
                } else { Err(lost()) },
                // EOF can precede exit-status. Keep reading until close.
                _ => {}
            },
            Some(write) = writes.recv() => match write {
                Write::Bytes(bytes) => channel.data(bytes.as_slice()).await.map_err(connection_error)?,
                Write::Resize(size) => channel.window_change(u32::from(size.columns()), u32::from(size.rows()), 0, 0)
                    .await.map_err(connection_error)?,
            },
        }
    }
}

async fn request_accepted(
    channel: &mut russh::Channel<client::Msg>,
    events: &mpsc::Sender<Event>,
) -> Result<(), SessionFailure> {
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Success) => return Ok(()),
            Some(russh::ChannelMsg::Failure) => return Err(SessionFailure::ShellRejected),
            Some(
                russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. },
            ) => {
                let _ = events.send(Event::Output(data.to_vec())).await;
            }
            Some(russh::ChannelMsg::Close) | None => return Err(lost()),
            _ => {}
        }
    }
}

fn lost() -> SessionFailure {
    SessionFailure::ConnectionLost(
        "SSH transport EOF or channel closed without exit status/signal".into(),
    )
}

fn connection_error(error: russh::Error) -> SessionFailure {
    SessionFailure::ConnectionLost(match error {
        russh::Error::IO(error) => format!("SSH transport ({:?}): {error}", error.kind()),
        error => format!("SSH: {error}"),
    })
}

fn internal(error: impl std::fmt::Display) -> SessionFailure {
    SessionFailure::Internal(error.to_string())
}

fn publish(
    driver: &mut SessionDriver,
    terminal: &mut TerminalEngine,
) -> Result<(), SessionFailure> {
    let frame = terminal.frame().map_err(internal)?;
    driver.publish(frame).map_err(internal)
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
