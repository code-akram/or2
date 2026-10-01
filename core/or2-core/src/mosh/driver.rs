//! The mosh session driver: one dedicated thread that owns the terminal and the protocol state
//! and moves datagrams, with the standard session lifecycle (`Connecting`, `Connected`,
//! `Closed`) and the same frame, input, resize, scroll and disconnect behaviour as the SSH
//! driver.
//!
//! mosh has no handshake of its own (the SSH bootstrap agreed the key), so the session becomes
//! `Connected` when the first datagram from the server authenticates, and fails with `TimedOut`
//! if none does within [`CONNECT_TIMEOUT`]. After that it never gives up on its own: surviving a
//! dead network is mosh's point. It ends when the server announces the end of the session
//! (`RemoteExited`), the user disconnects, or something internal breaks.

use std::future::{Future, poll_fn};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, sleep, sleep_until, timeout};

use crate::input::text_bytes;
use crate::session::{
    CloseReason, Command, SessionDriver, SessionFailure, SessionHandle, SessionObserver,
    SessionState, channel,
};
use crate::transport::{DatagramTransport, DirectUdp, Endpoint, EndpointError};

use super::bootstrap::MoshParams;
use super::ghostty::GhosttyScreen;
use super::link::{Link, is_too_large};
use super::ssp::crypto::RECEIVE_MTU;
use super::ssp::session::{Fault, LinkHealth, Session};

/// How long to wait for the server's first datagram before failing the session.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a disconnect waits for the server to acknowledge the shutdown before giving up.
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);
/// How often [`HealthObserver::link_health`] is called.
const HEALTH_INTERVAL: Duration = Duration::from_secs(1);
/// How long to wait before trying to open another socket after one failed.
const REBIND_RETRY: Duration = Duration::from_secs(1);
/// How long opening another socket may take, name resolution included, before it is given up
/// and retried. It runs beside the rest of the session, so this only bounds how long a rotation
/// can be stuck, not how long the session is unresponsive.
const REBIND_TIMEOUT: Duration = Duration::from_secs(3);
/// How many datagrams are taken in one go before frames and commands get a turn.
const MAX_DATAGRAMS_PER_TURN: usize = 64;

/// Receives the link's health about once a second, from the driver thread. This is the hook M3
/// uses to show "no contact for N seconds"; it must return quickly.
pub trait HealthObserver: Send + Sync {
    fn link_health(&self, health: LinkHealth);
}

/// Controls the network side of a running session. Cheap to clone and callable from any thread.
#[derive(Clone)]
pub struct LinkControl {
    roam: Arc<Notify>,
}

impl LinkControl {
    /// The network changed: open a new socket now and send from it, instead of waiting ten
    /// seconds without answers to notice. The server follows the first datagram that
    /// authenticates from the new source.
    pub fn roam(&self) {
        self.roam.notify_one();
    }
}

/// Starts a mosh session over OS UDP sockets. Validates the address synchronously and returns
/// at once; everything else arrives through `observer`. `peer` is the IP address the SSH
/// connection that ran the bootstrap actually reached (`HostHandle::peer_addr`, not a name
/// looked up again); the server listens on it at UDP port `params.port`. Every socket of the
/// session, roaming included, must reach exactly that address.
pub fn start(
    params: MoshParams,
    peer: IpAddr,
    observer: Arc<dyn SessionObserver>,
) -> Result<SessionHandle, EndpointError> {
    Ok(start_with(DirectUdp, params, peer, observer, None)?.0)
}

/// [`start`] over any [`DatagramTransport`], with a health hook, returning the network control
/// next to the session handle.
pub fn start_with<T: DatagramTransport>(
    transport: T,
    params: MoshParams,
    peer: IpAddr,
    observer: Arc<dyn SessionObserver>,
    health: Option<Arc<dyn HealthObserver>>,
) -> Result<(SessionHandle, LinkControl), EndpointError> {
    spawn(transport, params, peer, observer, health, CONNECT_TIMEOUT)
}

fn spawn<T: DatagramTransport>(
    transport: T,
    params: MoshParams,
    peer: IpAddr,
    observer: Arc<dyn SessionObserver>,
    health: Option<Arc<dyn HealthObserver>>,
    connect_timeout: Duration,
) -> Result<(SessionHandle, LinkControl), EndpointError> {
    // Only to validate the port; the link builds its own endpoint from the pinned address.
    Endpoint::new(&peer.to_string(), params.port)?;
    let peer = SocketAddr::new(peer, params.port);
    let (handle, mut driver) = channel(observer);
    let control = LinkControl {
        roam: Arc::new(Notify::new()),
    };
    // Initialize before spawning so runtime initialization is never done in a callback.
    let runtime = crate::ssh::runtime();
    let roam = control.roam.clone();
    std::thread::Builder::new()
        .name("or2-mosh".into())
        .spawn(move || {
            runtime.block_on(async move {
                let reason = match run(
                    transport,
                    peer,
                    params,
                    health,
                    roam,
                    connect_timeout,
                    &mut driver,
                )
                .await
                {
                    Ok(reason) => reason,
                    Err(failure) => CloseReason::Failed(failure),
                };
                driver.close(reason);
            })
        })
        .expect("create mosh session thread");
    Ok((handle, control))
}

fn internal(error: impl std::fmt::Display) -> SessionFailure {
    SessionFailure::Internal(error.to_string())
}

async fn run<T: DatagramTransport>(
    transport: T,
    peer: SocketAddr,
    params: MoshParams,
    health: Option<Arc<dyn HealthObserver>>,
    roam: Arc<Notify>,
    connect_timeout: Duration,
    driver: &mut SessionDriver,
) -> Result<CloseReason, SessionFailure> {
    let key = params.key.to_base64_key().map_err(internal)?;
    let screen = GhosttyScreen::new(params.size).map_err(internal)?;
    // Opening the first socket resolves the host name, which can take as long as the resolver
    // does: a disconnect (or a dropped handle) must not wait for it. A resize is remembered.
    let mut size = params.size;
    let open = timeout(connect_timeout, Link::open(Arc::new(transport), peer));
    tokio::pin!(open);
    let mut link = loop {
        tokio::select! {
            opened = &mut open => match opened {
                Ok(Ok(link)) => break link,
                Ok(Err(error)) => {
                    return Err(SessionFailure::Unreachable(format!(
                        "UDP socket failed ({:?}): {error}",
                        error.kind()
                    )));
                }
                Err(_) => return Err(SessionFailure::TimedOut),
            },
            command = driver.next_command() => match command {
                Command::Disconnect => return Ok(CloseReason::Disconnected),
                Command::Resize(new) => size = new,
                // Nothing to send to yet, and host keys do not exist.
                _ => {}
            },
        }
    };
    let mut session = Session::new(&key, link.peer_is_ipv6(), screen);
    // The server starts at 80x24 whatever we want; say so in the first datagram.
    session
        .resize(size.rows(), size.columns())
        .map_err(internal)?;

    let mut buffer = [0u8; RECEIVE_MTU];
    let deadline = Instant::now() + connect_timeout;
    let mut connected = false;
    let mut published = u64::MAX;
    let mut next_rebind = Instant::now();
    let mut next_health = Instant::now() + HEALTH_INTERVAL;
    // The socket being opened for a rebind, and when to give up on it.
    let mut opening: Option<Opening<T>> = None;
    let mut opening_deadline = Instant::now();

    loop {
        let tick = session.tick().map_err(internal)?;
        send(&link, &mut session, &tick.datagrams);
        if tick.rebind && opening.is_none() && Instant::now() >= next_rebind {
            opening = Some(Box::pin(link.next_socket()));
            opening_deadline = Instant::now() + REBIND_TIMEOUT;
        }
        if let Some(observer) = &health
            && Instant::now() >= next_health
        {
            observer.link_health(session.link_health());
            next_health = Instant::now() + HEALTH_INTERVAL;
        }

        let mut wait = Duration::from_millis(session.wait_time_ms().max(1));
        if health.is_some() {
            wait = wait.min(HEALTH_INTERVAL);
        }
        tokio::select! {
            received = poll_fn(|cx| link.poll_recv(cx, &mut buffer)) => {
                let mut taken = 0;
                let mut next = Some(received);
                while let Some(result) = next.take() {
                    match result {
                        Ok(length) => {
                            match session.handle_datagram(&buffer[..length]) {
                                Ok(_) => {
                                    link.authenticated(std::time::Instant::now());
                                    if !connected {
                                        connected = true;
                                        driver
                                            .transition(SessionState::Connected)
                                            .map_err(internal)?;
                                    }
                                }
                                Err(Fault::Dropped(_)) => {}
                                Err(Fault::Screen(error)) => return Err(internal(error)),
                            }
                        }
                        // The network says no (a refused port, an unreachable route). mosh
                        // keeps trying, so this is not an end; just do not spin on it.
                        Err(_) => sleep(Duration::from_millis(20)).await,
                    }
                    taken += 1;
                    if taken < MAX_DATAGRAMS_PER_TURN {
                        next = link.try_recv(&mut buffer);
                    }
                }
                if connected {
                    publish_if_changed(driver, &mut session, &mut published)?;
                }
                if session.peer_shut_down() {
                    // Acknowledge the server's goodbye, then we are done.
                    let tick = session.tick().map_err(internal)?;
                    send(&link, &mut session, &tick.datagrams);
                    return Ok(CloseReason::RemoteExited { exit_status: None });
                }
            }
            command = driver.next_command() => {
                match command {
                    Command::Disconnect => {
                        goodbye(&mut link, &mut session, &mut buffer).await;
                        return Ok(CloseReason::Disconnected);
                    }
                    // mosh has no host key prompt: the SSH connection that ran the bootstrap
                    // made that decision.
                    Command::ApproveHostKey { .. } | Command::RejectHostKey => {}
                    Command::Resize(size) => {
                        session.resize(size.rows(), size.columns()).map_err(internal)?;
                        if connected {
                            publish(driver, &mut session)?;
                        }
                    }
                    Command::Text(text) => session.send_input(&text_bytes(&text)),
                    Command::Key(key) => {
                        let bytes = session.terminal().live().engine().encode_key(&key).map_err(internal)?;
                        session.send_input(&bytes);
                    }
                    Command::Scroll(scroll) => {
                        let bytes = session.terminal().live().engine().scroll(scroll).map_err(internal)?;
                        if !bytes.is_empty() {
                            session.send_input(&bytes);
                        }
                        publish(driver, &mut session)?;
                    }
                    Command::FullFrame => {
                        session.terminal().live().engine().request_full_frame();
                        publish(driver, &mut session)?;
                    }
                }
            }
            opened = poll_fn(|cx| match opening.as_mut() {
                Some(opening) => opening.as_mut().poll(cx),
                None => std::task::Poll::Pending,
            }), if opening.is_some() => {
                opening = None;
                match opened.and_then(|socket| link.adopt(socket)) {
                    Ok(()) => session.note_rebound(),
                    Err(_) => next_rebind = Instant::now() + REBIND_RETRY,
                }
            }
            () = sleep_until(opening_deadline), if opening.is_some() => {
                // A resolver that never answers: drop the attempt and try again shortly.
                opening = None;
                next_rebind = Instant::now() + REBIND_RETRY;
            }
            () = roam.notified() => session.request_rebind(),
            () = sleep(wait) => {}
            () = sleep_until(deadline), if !connected => {
                return Err(SessionFailure::TimedOut);
            }
        }
    }
}

/// A socket being opened for a rebind.
type Opening<T> =
    Pin<Box<dyn Future<Output = io::Result<<T as DatagramTransport>::Socket>> + Send>>;

/// Sends datagrams from the newest socket. A refused or dropped send costs nothing the protocol
/// does not already repair, since it resends state; only "too large" changes behaviour.
fn send<T: DatagramTransport>(
    link: &Link<T>,
    session: &mut Session<GhosttyScreen>,
    datagrams: &[Vec<u8>],
) {
    for datagram in datagrams {
        if let Err(error) = link.send(datagram)
            && is_too_large(&error)
        {
            session.datagram_too_large();
        }
    }
}

/// Tells the server the client is leaving, as mosh does, so its session ends instead of lingering
/// on the host. Bounded: a server that is already gone must not hold the disconnect.
async fn goodbye<T: DatagramTransport>(
    link: &mut Link<T>,
    session: &mut Session<GhosttyScreen>,
    buffer: &mut [u8],
) {
    session.shutdown();
    let deadline = Instant::now() + GOODBYE_TIMEOUT;
    while !session.finished() && Instant::now() < deadline {
        let Ok(tick) = session.tick() else { return };
        send(link, session, &tick.datagrams);
        let wait = Duration::from_millis(session.wait_time_ms().clamp(1, 50));
        tokio::select! {
            received = poll_fn(|cx| link.poll_recv(cx, buffer)) => {
                if let Ok(length) = received {
                    let _ = session.handle_datagram(&buffer[..length]);
                }
            }
            () = sleep(wait) => {}
            () = sleep_until(deadline) => {}
        }
    }
}

fn publish_if_changed(
    driver: &mut SessionDriver,
    session: &mut Session<GhosttyScreen>,
    published: &mut u64,
) -> Result<(), SessionFailure> {
    let latest = session.terminal().latest();
    if *published == latest {
        return Ok(());
    }
    *published = latest;
    publish(driver, session)
}

fn publish(
    driver: &mut SessionDriver,
    session: &mut Session<GhosttyScreen>,
) -> Result<(), SessionFailure> {
    let frame = session
        .terminal()
        .live()
        .engine()
        .frame()
        .map_err(internal)?;
    driver.publish(frame).map_err(internal)
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
