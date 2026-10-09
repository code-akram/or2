//! Network paths. SSH reaches hosts only through [`Transport`] and mosh only through
//! [`DatagramTransport`].
//!
//! A transport turns an [`Endpoint`] into a byte stream. Its `Stream` bound is exactly the bound
//! `russh::client::connect_stream` requires, so any transport can carry an SSH connection.
//! [`DirectTcp`] is the one stream implementation so far (it resolves a name once, retrying a
//! `.local` one, and races what it resolved: see [`dial`]). [`race_with`] connects to a host's several
//! addresses over any transport, each with its own time limit. Jump hosts and the Android network binding are added as
//! further implementations or methods when those milestones need them.
//!
//! mosh needs datagrams, and needs to open a new socket to the same endpoint whenever the
//! network changes (that is its roaming), so a [`DatagramTransport`] opens one socket per
//! `bind` and the mosh code never creates a socket itself. [`DirectUdp`] uses OS sockets.
//!
//! Wake-on-LAN broadcasts one datagram and hears nothing back, through [`DatagramBroadcast`]
//! ([`DirectBroadcast`]), the only socket allowed to send to a broadcast address.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

#[path = "transport_dial.rs"]
mod dial;

pub use dial::{Connector, DialTiming, Resolver, SystemResolver, TcpConnector, dial};

/// A validated host name or IP literal and a nonzero TCP port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    host: String,
    port: NonZeroU16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    #[error("host must be nonempty, at most 255 bytes, and contain no whitespace or controls")]
    InvalidHost,
    #[error("port must be nonzero")]
    InvalidPort,
}

impl Endpoint {
    pub fn new(host: &str, port: u16) -> Result<Self, EndpointError> {
        let valid_host = !host.is_empty()
            && host.len() <= 255
            && !host.chars().any(|c| c.is_whitespace() || c.is_control());
        if !valid_host {
            return Err(EndpointError::InvalidHost);
        }
        let port = NonZeroU16::new(port).ok_or(EndpointError::InvalidPort)?;
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port.get()
    }
}

/// Opens byte streams to endpoints. Dropping the returned future cancels the attempt; callers
/// apply their own timeout. Errors are plain `io::Error`s so the session can classify them.
pub trait Transport: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    fn connect(&self, endpoint: &Endpoint)
    -> impl Future<Output = io::Result<Self::Stream>> + Send;

    /// The remote address `stream` actually reached, when the transport knows it. A host name
    /// can resolve to several addresses and to different ones over time, so this (not the
    /// name) is what later datagram traffic to the same host must be pinned to (mosh).
    /// `None` for a transport that carries no such address (a tunnel, a test pipe).
    fn peer_addr(&self, _stream: &Self::Stream) -> Option<SocketAddr> {
        None
    }
}

/// OS sockets: LAN, or a VPN app such as ZeroTier that routes through the OS.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectTcp;

impl Transport for DirectTcp {
    type Stream = TcpStream;

    /// Resolves the name once and races what it resolved ([`dial`]): a `.local` name is retried,
    /// an IPv6 link-local address without a scope is skipped, and no address can hold the others.
    async fn connect(&self, endpoint: &Endpoint) -> io::Result<TcpStream> {
        dial(
            &SystemResolver,
            &TcpConnector,
            endpoint,
            DialTiming::default(),
        )
        .await
        .map(|(stream, _)| stream)
    }

    fn peer_addr(&self, stream: &TcpStream) -> Option<SocketAddr> {
        stream.peer_addr().ok()
    }
}

/// Each further address starts this long after the previous one started, unless the previous
/// one failed first.
pub const RACE_STAGGER: Duration = Duration::from_millis(250);

/// How long one address may take, name resolution included, before the race counts it as
/// unanswered. Each address has its own allowance inside the host's overall connect timeout, so
/// one address that silently drops packets (an overlay IP with no route, a sleeping machine) can
/// never consume the whole budget while the others have already failed.
pub const ADDRESS_TIMEOUT: Duration = Duration::from_secs(6);

/// The timing of a [`race_with`].
#[derive(Debug, Clone, Copy)]
pub struct RaceTiming {
    /// See [`RACE_STAGGER`].
    pub stagger: Duration,
    /// See [`ADDRESS_TIMEOUT`].
    pub address_timeout: Duration,
}

impl Default for RaceTiming {
    fn default() -> Self {
        Self {
            stagger: RACE_STAGGER,
            address_timeout: ADDRESS_TIMEOUT,
        }
    }
}

/// The winner of a [`race_with`]: its position in the address list, its stream and the remote
/// address the transport says that stream reached ([`Transport::peer_addr`]).
#[derive(Debug)]
pub struct Raced<S> {
    pub index: usize,
    pub stream: S,
    pub peer: Option<SocketAddr>,
}

/// Every address failed. `errors[i]` is address `i`'s error.
#[derive(Debug)]
pub struct RaceFailure {
    pub errors: Vec<io::Error>,
}

/// An address's error in words a person can act on, without the address: `connection refused`,
/// `no route to the host`, `no answer within 6 s`, `name not resolved (mDNS) after 3 tries`.
/// Errors this module raised carry their own text; an operating-system error is named by its
/// kind.
pub fn describe_error(error: &io::Error) -> String {
    // What several addresses of one endpoint said (`2 addresses: connection refused, no answer
    // within 5 s`) is filed under one kind; the words are the description.
    if dial::is_summary(error) {
        return error.to_string();
    }
    match error.kind() {
        io::ErrorKind::ConnectionRefused => "connection refused".into(),
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => {
            "no route to the host".into()
        }
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => {
            "connection reset".into()
        }
        io::ErrorKind::TimedOut if error.raw_os_error().is_some() => "no answer".into(),
        _ if error.raw_os_error().is_none() && error.get_ref().is_some() => error.to_string(),
        kind => format!("{kind:?} ({error})"),
    }
}

impl fmt::Display for RaceFailure {
    /// `address 0: connection refused; address 1: no answer within 6 s`. Positions and
    /// outcomes only: no host names or addresses, so the text is safe in diagnostics (the app
    /// puts the names back for its own screen).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "address {index}: {}", describe_error(error))?;
        }
        Ok(())
    }
}

impl std::error::Error for RaceFailure {}

/// `6 s`, `5.5 s`.
pub fn seconds(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{} s", duration.as_secs())
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

/// What each address of a race has done so far, shared with whoever has to explain a race that
/// is still running when its time is up (the host's overall connect timeout).
#[derive(Debug, Clone, Default)]
pub struct RaceReport {
    state: Arc<Mutex<ReportState>>,
}

#[derive(Debug, Default)]
struct ReportState {
    addresses: Vec<Progress>,
    over: bool,
}

#[derive(Debug, Clone)]
enum Progress {
    Waiting,
    Trying(Instant),
    Failed(String),
}

impl RaceReport {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReportState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn begin(&self, count: usize) {
        *self.lock() = ReportState {
            addresses: vec![Progress::Waiting; count],
            over: false,
        };
    }

    fn started(&self, index: usize) {
        if let Some(entry) = self.lock().addresses.get_mut(index) {
            *entry = Progress::Trying(Instant::now());
        }
    }

    fn failed(&self, index: usize, error: &io::Error) {
        if let Some(entry) = self.lock().addresses.get_mut(index) {
            *entry = Progress::Failed(describe_error(error));
        }
    }

    fn finish(&self) {
        self.lock().over = true;
    }

    /// True while the race has started and not yet produced a winner or a failure.
    pub fn is_pending(&self) -> bool {
        let state = self.lock();
        !state.addresses.is_empty() && !state.over
    }

    /// Each address's outcome so far, in the words of [`RaceFailure`]: those that failed say
    /// why, those still running say for how long (`still trying after 20 s`).
    pub fn describe(&self) -> String {
        let state = self.lock();
        let mut out = String::new();
        for (index, progress) in state.addresses.iter().enumerate() {
            if index > 0 {
                out.push_str("; ");
            }
            let what = match progress {
                Progress::Waiting => "not tried yet".to_owned(),
                Progress::Trying(since) => {
                    format!("still trying after {}", seconds(since.elapsed()))
                }
                Progress::Failed(why) => why.clone(),
            };
            out.push_str(&format!("address {index}: {what}"));
        }
        out
    }
}

/// What a [`race_attempts`] caller hears about an attempt.
pub(crate) enum Step<'a> {
    Started,
    Failed(&'a io::Error),
}

/// The race itself, over any attempts: starts attempt 0 at once, each next one `stagger` after
/// the previous one started or immediately when it fails, gives every attempt `attempt_timeout`
/// (`TimedOut`, "no answer within ..."), and returns the first to succeed with its index.
/// Every other attempt is dropped (a loser never keeps a socket), and so are all of them if the
/// caller drops this future. When all fail, the errors in attempt order.
pub(crate) async fn race_attempts<S, F, Fut>(
    count: usize,
    stagger: Duration,
    attempt_timeout: Duration,
    mut on_step: impl FnMut(usize, Step<'_>),
    start: F,
) -> Result<(usize, S), Vec<io::Error>>
where
    F: Fn(usize) -> Fut,
    Fut: Future<Output = io::Result<S>> + Send + 'static,
    S: Send + 'static,
{
    // Dropping the set aborts every attempt still running.
    type Attempt<S> = (usize, io::Result<S>);
    let mut attempts: JoinSet<Attempt<S>> = JoinSet::new();
    let mut errors: Vec<Option<io::Error>> = (0..count).map(|_| None).collect();
    let mut next = 0;
    let mut next_at = Instant::now();
    loop {
        let starting = next < count;
        tokio::select! {
            () = sleep_until(next_at), if starting => {
                let index = next;
                let attempt = start(index);
                on_step(index, Step::Started);
                attempts.spawn(async move {
                    let result = match tokio::time::timeout(attempt_timeout, attempt).await {
                        Ok(result) => result,
                        Err(_) => Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("no answer within {}", seconds(attempt_timeout)),
                        )),
                    };
                    (index, result)
                });
                next += 1;
                next_at = Instant::now() + stagger;
            }
            finished = attempts.join_next(), if !attempts.is_empty() => {
                let Some(finished) = finished else { continue };
                let (index, result) = match finished {
                    Ok(finished) => finished,
                    // A panic in a transport is a bug, not an address failure: surface it.
                    // Attempts are never aborted individually.
                    Err(error) => std::panic::resume_unwind(error.into_panic()),
                };
                match result {
                    Ok(stream) => return Ok((index, stream)),
                    Err(error) => {
                        on_step(index, Step::Failed(&error));
                        errors[index] = Some(error);
                        // Only the most recently started attempt's failure brings the next
                        // one forward; an older failure leaves the schedule alone.
                        if index + 1 == next {
                            next_at = Instant::now();
                        }
                    }
                }
            }
            else => break,
        }
    }
    Err(errors
        .into_iter()
        .map(|error| error.unwrap_or_else(|| io::Error::other("not attempted")))
        .collect())
}

/// Connects to the first of `addresses` that answers. Address 0 starts at once; each next one
/// starts `timing.stagger` after the previous one started, or immediately when that previous
/// one fails. The first connection wins and every other attempt is dropped, so a loser never
/// keeps a socket. Dropping the future cancels all attempts. Each address has
/// `timing.address_timeout` to answer (its own failure, `no answer within 6 s`), so the race
/// always ends: the caller's own timeout is the overall bound, not the only one. `report`, if
/// given, follows every address as the race goes (reset at the start, finished when the race
/// ends).
pub async fn race_with<T: Transport>(
    transport: &Arc<T>,
    addresses: &[Endpoint],
    timing: RaceTiming,
    report: Option<&RaceReport>,
) -> Result<Raced<T::Stream>, RaceFailure> {
    if let Some(report) = report {
        report.begin(addresses.len());
    }
    let outcome = race_attempts(
        addresses.len(),
        timing.stagger,
        timing.address_timeout,
        |index, step| {
            if let Some(report) = report {
                match step {
                    Step::Started => report.started(index),
                    Step::Failed(error) => report.failed(index, error),
                }
            }
        },
        |index| {
            let transport = Arc::clone(transport);
            let endpoint = addresses[index].clone();
            async move {
                let stream = transport.connect(&endpoint).await?;
                let peer = transport.peer_addr(&stream);
                Ok((stream, peer))
            }
        },
    )
    .await;
    if let Some(report) = report {
        report.finish();
    }
    match outcome {
        Ok((index, (stream, peer))) => Ok(Raced {
            index,
            stream,
            peer,
        }),
        Err(errors) => Err(RaceFailure { errors }),
    }
}

#[cfg(test)]
#[path = "transport_race_tests.rs"]
mod race_tests;

/// A datagram socket connected to one peer: it sends only to that peer and delivers only what
/// that peer sent. Implementations never block.
pub trait DatagramSocket: Send + Sync + 'static {
    /// The local address datagrams leave from. Roaming shows up here as a changed port.
    fn local_addr(&self) -> io::Result<SocketAddr>;

    /// The peer's address; its family sizes the datagrams.
    fn peer_addr(&self) -> io::Result<SocketAddr>;

    /// Sends one datagram without waiting. A full socket buffer drops it, as the network may:
    /// the caller sees `WouldBlock` and carries on, since mosh resends state, not packets.
    fn try_send(&self, datagram: &[u8]) -> io::Result<usize>;

    /// Polls for the next datagram, returning its length. Cancel-safe: dropping the future that
    /// polls it loses nothing.
    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>>;
}

/// Opens datagram sockets to endpoints. Each `bind` returns a fresh socket from a fresh local
/// port (or, on Android, a socket bound to whichever network is current), which is how a mosh
/// session roams: the old socket keeps receiving whatever is still in flight to it while the
/// new one carries what is sent from now on.
pub trait DatagramTransport: Send + Sync + 'static {
    type Socket: DatagramSocket;

    /// Resolves `endpoint` and opens a socket connected to it. Dropping the returned future
    /// cancels the attempt; callers apply their own timeout.
    fn bind(&self, endpoint: &Endpoint) -> impl Future<Output = io::Result<Self::Socket>> + Send;
}

/// OS UDP sockets: LAN, or a VPN app such as ZeroTier that routes through the OS. Resolves the
/// host and uses its first address, as mosh does.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectUdp;

impl DatagramSocket for UdpSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::peer_addr(self)
    }

    fn try_send(&self, datagram: &[u8]) -> io::Result<usize> {
        UdpSocket::try_send(self, datagram)
    }

    fn poll_recv(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut read = ReadBuf::new(buf);
        UdpSocket::poll_recv(self, cx, &mut read).map_ok(|()| read.filled().len())
    }
}

impl DatagramTransport for DirectUdp {
    type Socket = UdpSocket;

    async fn bind(&self, endpoint: &Endpoint) -> io::Result<UdpSocket> {
        let peer = tokio::net::lookup_host((endpoint.host(), endpoint.port()))
            .await?
            .next()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "host resolved to no addresses")
            })?;
        let local = if peer.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let socket = UdpSocket::bind(local).await?;
        socket.connect(peer).await?;
        // `try_send` only attempts the send once the reactor has reported the socket writable;
        // wait for that here so the first datagram is not refused as `WouldBlock`.
        socket.writable().await?;
        Ok(socket)
    }
}

/// Sends datagrams to broadcast addresses (Wake-on-LAN's magic packet, see [`crate::wake`]). Kept
/// apart from [`DatagramTransport`]: a mosh socket is connected to one peer and must never be able
/// to broadcast, while this one only ever sends and never hears an answer.
pub trait DatagramBroadcast: Send + Sync + 'static {
    /// Sends `datagram` once to each of `targets`, from one fresh socket. One target that cannot be
    /// reached (no route to a subnet that is gone) does not keep the datagram from the others: the
    /// first error is returned only when no target took it.
    fn send_all(
        &self,
        datagram: &[u8],
        targets: &[SocketAddrV4],
    ) -> impl Future<Output = io::Result<()>> + Send;
}

/// OS UDP sockets with `SO_BROADCAST` set: the app's only socket that may send to a broadcast
/// address. It goes out on whichever interface the OS routes each target through (a VPN app such as
/// ZeroTier included), like [`DirectUdp`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectBroadcast;

impl DirectBroadcast {
    async fn open(&self) -> io::Result<UdpSocket> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
        socket.set_broadcast(true)?;
        Ok(socket)
    }
}

impl DatagramBroadcast for DirectBroadcast {
    async fn send_all(&self, datagram: &[u8], targets: &[SocketAddrV4]) -> io::Result<()> {
        let socket = self.open().await?;
        let mut first_error = None;
        let mut sent = false;
        for target in targets {
            match socket.send_to(datagram, target).await {
                Ok(_) => sent = true,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) if !sent => Err(error),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn direct_broadcast_sockets_may_broadcast_and_deliver_to_every_target() {
        assert!(DirectBroadcast.open().await.unwrap().broadcast().unwrap());

        // Loopback receivers only: a test never puts a broadcast on a real network.
        let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let second = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = |socket: &UdpSocket| match socket.local_addr().unwrap() {
            SocketAddr::V4(address) => address,
            SocketAddr::V6(_) => unreachable!(),
        };
        DirectBroadcast
            .send_all(b"wake", &[target(&first), target(&second)])
            .await
            .unwrap();
        for socket in [&first, &second] {
            let mut buf = [0u8; 16];
            let n = socket.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"wake");
        }
    }

    #[tokio::test]
    async fn direct_broadcast_fails_only_when_no_target_took_the_datagram() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let SocketAddr::V4(reachable) = receiver.local_addr().unwrap() else {
            unreachable!()
        };
        // Port 0 is not a destination: the OS refuses the send at once.
        let refused = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
        assert!(DirectBroadcast.send_all(b"x", &[refused]).await.is_err());
        DirectBroadcast
            .send_all(b"y", &[refused, reachable])
            .await
            .unwrap();
        let mut buf = [0u8; 4];
        let n = receiver.recv(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"y");
    }

    #[tokio::test]
    async fn direct_udp_sockets_exchange_datagrams_with_their_peer_only() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        let endpoint = Endpoint::new("127.0.0.1", port).unwrap();

        let socket = DirectUdp.bind(&endpoint).await.unwrap();
        assert_eq!(
            DatagramSocket::peer_addr(&socket).unwrap(),
            server.local_addr().unwrap()
        );
        DatagramSocket::try_send(&socket, b"ping").unwrap();
        let mut buf = [0u8; 16];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"ping");
        assert_eq!(from, DatagramSocket::local_addr(&socket).unwrap());

        // A stranger's datagram never reaches a connected socket; the peer's does.
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger.send_to(b"nope", from).await.unwrap();
        server.send_to(b"pong", from).await.unwrap();
        let mut buf = [0u8; 16];
        let n = std::future::poll_fn(|cx| DatagramSocket::poll_recv(&socket, cx, &mut buf))
            .await
            .unwrap();
        assert_eq!(&buf[..n], b"pong");
    }

    #[tokio::test]
    async fn every_bind_uses_a_fresh_local_port() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint::new("127.0.0.1", server.local_addr().unwrap().port()).unwrap();
        let first = DirectUdp.bind(&endpoint).await.unwrap();
        let second = DirectUdp.bind(&endpoint).await.unwrap();
        assert_ne!(
            DatagramSocket::local_addr(&first).unwrap().port(),
            DatagramSocket::local_addr(&second).unwrap().port()
        );
    }

    #[tokio::test]
    async fn direct_udp_reports_an_unresolvable_host_as_an_error() {
        let endpoint = Endpoint::new("host.invalid", 9).unwrap();
        assert!(DirectUdp.bind(&endpoint).await.is_err());
    }

    #[test]
    fn endpoint_accepts_names_and_ip_literals() {
        for host in ["example.org", "203.0.113.3", "fe80::1", "host-b"] {
            let endpoint = Endpoint::new(host, 2222).unwrap();
            assert_eq!(endpoint.host(), host);
            assert_eq!(endpoint.port(), 2222);
        }
    }

    #[test]
    fn endpoint_rejects_empty_spaced_control_or_long_hosts_and_port_zero() {
        let long = "a".repeat(256);
        for host in ["", "a b", "a\tb", "a\u{7}", " host", long.as_str()] {
            assert_eq!(
                Endpoint::new(host, 22),
                Err(EndpointError::InvalidHost),
                "{host:?}"
            );
        }
        assert!(Endpoint::new(&"a".repeat(255), 22).is_ok());
        assert_eq!(Endpoint::new("host", 0), Err(EndpointError::InvalidPort));
    }
}
