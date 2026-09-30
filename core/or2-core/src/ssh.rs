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
    TransportEnded,
    Closed(CloseReason),
}

enum Write {
    Bytes(Vec<u8>),
    Resize(TerminalSize),
}

struct Client {
    trusted: Vec<HostKey>,
    events: mpsc::Sender<Event>,
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
            return Ok(true);
        }
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
    let network = tokio::spawn(async move {
        let reason = network(request, events.clone(), outgoing, latest_size).await;
        let _ = events.send(Event::Closed(reason)).await;
    });
    let mut decision = None;
    let mut deadline = Instant::now() + timeout;
    let mut remaining = timeout;
    let mut timing = true;
    let mut connected = false;
    let reason = loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
                Event::HostKey(prompt, reply) => {
                    remaining = deadline.saturating_duration_since(Instant::now());
                    timing = false; // Human confirmation is bounded by the server, not us.
                    decision = Some(reply);
                    driver.transition(SessionState::AwaitingHostKey(prompt)).unwrap();
                }
                Event::Authenticating => {
                    timing = false;
                    driver.transition(SessionState::Authenticating).unwrap();
                }
                Event::Connected => {
                    connected = true;
                    driver.transition(SessionState::Connected).unwrap();
                }
                Event::Output(_bytes) => {
                    // Terminal adapter consumes these bytes in milestone A2.
                }
                Event::TransportEnded => {
                    if decision.is_some() { break CloseReason::Failed(lost()); }
                }
                Event::Closed(reason) => break reason,
            },
            command = driver.next_command() => match command {
                Command::Disconnect => break CloseReason::Disconnected,
                Command::RejectHostKey => break CloseReason::Failed(SessionFailure::HostKeyRejected),
                Command::ApproveHostKey { .. } => {
                    if let Some(reply) = decision.take() {
                        let _ = reply.send(true);
                        deadline = Instant::now() + remaining;
                        timing = true;
                    }
                }
                Command::Resize(new_size) => {
                    size.send_replace(new_size);
                    if connected { let _ = writes.send(Write::Resize(new_size)); }
                }
                Command::Text(text) => { let _ = writes.send(Write::Bytes(crate::input::text_bytes(&text))); }
                Command::Key(_) | Command::Scroll(_) | Command::FullFrame => {}
            },
            () = sleep_until(deadline), if timing => break CloseReason::Failed(SessionFailure::TimedOut),
        }
    };
    network.abort();
    let _ = network.await;
    driver.close(reason);
    // Cancellation drops the relay's actual transport, including during a host-key prompt.
}

async fn network(
    request: ConnectRequest,
    events: mpsc::Sender<Event>,
    mut writes: mpsc::UnboundedReceiver<Write>,
    size: watch::Receiver<TerminalSize>,
) -> CloseReason {
    let stream = match DirectTcp.connect(&request.endpoint).await {
        Ok(stream) => stream,
        Err(_) => {
            return CloseReason::Failed(SessionFailure::Unreachable(
                "TCP connection failed".into(),
            ));
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
                let _ = events.send(Event::TransportEnded).await;
                result
            },
            tokio::io::copy(&mut pipe_read, &mut write),
        )
    };
    let result = tokio::select! {
        biased;
        result = shell(request, &events, &mut writes, size, ssh_stream) => result,
        _ = relay => Err(lost()),
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
        },
    )
    .await
    .map_err(|error| match error {
        ClientError::Certificate => {
            SessionFailure::UnsupportedHostKey("host certificates are unsupported".into())
        }
        ClientError::Ssh(russh::Error::IO(_) | russh::Error::Disconnect) => lost(),
        _ => SessionFailure::Protocol("SSH handshake failed".into()),
    })?;
    let _ = events.send(Event::Authenticating).await;
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|_| lost())?
        .flatten();
    let auth = handle
        .authenticate_publickey(
            request.username,
            PrivateKeyWithHashAlg::new(Arc::new(request.key.private_key().clone()), hash),
        )
        .await
        .map_err(|_| lost())?;
    if !auth.success() {
        return Err(SessionFailure::AuthenticationRejected);
    }
    let mut channel = handle.channel_open_session().await.map_err(|_| lost())?;
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
        .map_err(|_| lost())?;
    request_accepted(&mut channel, events).await?;
    channel.request_shell(true).await.map_err(|_| lost())?;
    request_accepted(&mut channel, events).await?;
    // Resizes during authentication/PTY setup still win before Connected.
    let latest = *size.borrow();
    if latest != initial {
        channel
            .window_change(u32::from(latest.columns()), u32::from(latest.rows()), 0, 0)
            .await
            .map_err(|_| lost())?;
    }
    let _ = events.send(Event::Connected).await;
    let mut exit_status = None;
    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. }) => {
                    let _ = events.send(Event::Output(data.to_vec())).await;
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status: status }) => exit_status = Some(status),
                Some(russh::ChannelMsg::Close) | None => return match exit_status {
                    Some(status) => Ok(CloseReason::RemoteExited { exit_status: Some(status) }),
                    None => Err(lost()),
                },
                // EOF can precede exit-status. Keep reading until close.
                _ => {}
            },
            Some(write) = writes.recv() => match write {
                Write::Bytes(bytes) => channel.data(bytes.as_slice()).await.map_err(|_| lost())?,
                Write::Resize(size) => channel.window_change(u32::from(size.columns()), u32::from(size.rows()), 0, 0)
                    .await.map_err(|_| lost())?,
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
    SessionFailure::ConnectionLost("SSH connection ended".into())
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
