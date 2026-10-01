//! Network paths. SSH reaches hosts only through [`Transport`] and mosh only through
//! [`DatagramTransport`].
//!
//! A transport turns an [`Endpoint`] into a byte stream. Its `Stream` bound is exactly the bound
//! `russh::client::connect_stream` requires, so any transport can carry an SSH connection.
//! [`DirectTcp`] is the one stream implementation so far. [`race`] connects to a host's several
//! addresses over any transport. Jump hosts and the Android network binding are added as
//! further implementations or methods when those milestones need them.
//!
//! mosh needs datagrams, and needs to open a new socket to the same endpoint whenever the
//! network changes (that is its roaming), so a [`DatagramTransport`] opens one socket per
//! `bind` and the mosh code never creates a socket itself. [`DirectUdp`] uses OS sockets.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

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
/// Resolves the host and tries each address in order (tokio's behaviour).
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectTcp;

impl Transport for DirectTcp {
    type Stream = TcpStream;

    async fn connect(&self, endpoint: &Endpoint) -> io::Result<TcpStream> {
        let stream = TcpStream::connect((endpoint.host(), endpoint.port())).await?;
        // Interactive keystrokes are tiny writes; do not let Nagle delay them.
        stream.set_nodelay(true)?;
        Ok(stream)
    }

    fn peer_addr(&self, stream: &TcpStream) -> Option<SocketAddr> {
        stream.peer_addr().ok()
    }
}

/// Each further address starts this long after the previous one started, unless the previous
/// one failed first.
pub const RACE_STAGGER: Duration = Duration::from_millis(250);

/// The winner of a [`race`]: its position in the address list, its stream and the remote
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

impl fmt::Display for RaceFailure {
    /// `address 0: ConnectionRefused (Connection refused (os error 111)); address 1: ...`.
    /// Positions and error kinds only: no host names, so the text is safe in diagnostics.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "address {index}: {:?} ({error})", error.kind())?;
        }
        Ok(())
    }
}

impl std::error::Error for RaceFailure {}

/// Connects to the first of `addresses` that answers. Address 0 starts at once; each next one
/// starts `stagger` after the previous one started, or immediately when that previous one
/// fails. The first connection wins and every other attempt is dropped, so a loser never
/// keeps a socket. Dropping the future cancels all attempts. There is no timeout here: the
/// caller bounds the race.
pub async fn race<T: Transport>(
    transport: &Arc<T>,
    addresses: &[Endpoint],
    stagger: Duration,
) -> Result<Raced<T::Stream>, RaceFailure> {
    // Dropping the set aborts every attempt still running.
    type Attempt<S> = (usize, io::Result<(S, Option<SocketAddr>)>);
    let mut attempts: JoinSet<Attempt<T::Stream>> = JoinSet::new();
    let mut errors: Vec<Option<io::Error>> = addresses.iter().map(|_| None).collect();
    let mut next = 0;
    let mut next_at = Instant::now();
    loop {
        let starting = next < addresses.len();
        tokio::select! {
            () = sleep_until(next_at), if starting => {
                let transport = Arc::clone(transport);
                let endpoint = addresses[next].clone();
                let index = next;
                attempts.spawn(async move {
                    let connected = transport.connect(&endpoint).await.map(|stream| {
                        let peer = transport.peer_addr(&stream);
                        (stream, peer)
                    });
                    (index, connected)
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
                    Ok((stream, peer)) => return Ok(Raced { index, stream, peer }),
                    Err(error) => {
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
    Err(RaceFailure {
        errors: errors
            .into_iter()
            .map(|error| error.unwrap_or_else(|| io::Error::other("not attempted")))
            .collect(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for host in ["example.org", "10.147.17.3", "fe80::1", "host-b"] {
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
