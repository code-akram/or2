//! The session's sockets, opened and rebound only through a [`DatagramTransport`].
//!
//! This is where upstream mosh-rs kept its `Vec<UdpSocket>`; it is the half of its session that
//! touched the OS, moved out so the protocol code (`ssp::session`) can stay free of I/O.
//!
//! Packets go out the NEWEST socket. All of them are read, because a reply to something sent
//! from an older port comes back to that port, which is what keeps a source-port rotation from
//! costing a round trip.
//!
//! **The remote address is pinned.** The server is one IP and port, the address the SSH
//! connection that ran the bootstrap actually reached. Roaming changes only the LOCAL socket
//! (a fresh source port, or on Android a socket on the new network): every socket must reach
//! exactly that address, and one that does not (a host name that has since resolved to a
//! different IP, even in the same address family) is refused, so the session can never be
//! steered to an address where its `mosh-server` is not listening.
//!
//! Opening a socket resolves the host name and can take as long as the resolver does, so it is
//! never done inside the link: [`Link::next_socket`] hands the driver an owned future to wait
//! on next to everything else, and [`Link::adopt`] takes the result.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::transport::{DatagramSocket, DatagramTransport, Endpoint};

/// How many old sockets to keep reading from at once.
const MAX_PORTS_OPEN: usize = 10;
/// A newest socket that has WORKED (delivered an authenticated datagram) this long makes the old
/// ones pointless: nothing in flight can still be coming back to them. One that has never
/// received anything proves nothing, so the old sockets stay while it has not.
const MAX_OLD_SOCKET_AGE: Duration = Duration::from_secs(60);

/// `EMSGSIZE`, or `WSAEMSGSIZE` on Windows: this datagram is larger than the path will carry.
///
/// The number differs by platform and is stable ABI on each, so it is written down rather than
/// pulled in with a C binding for one integer.
const EMSGSIZE: i32 = if cfg!(windows) {
    10040
} else if cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)) {
    40
} else {
    90
};

/// Whether a send failed because the datagram is too large for the path.
pub(super) fn is_too_large(error: &io::Error) -> bool {
    error.raw_os_error() == Some(EMSGSIZE)
}

pub(super) struct Link<T: DatagramTransport> {
    transport: Arc<T>,
    /// The pinned peer as an endpoint (an IP literal, so opening a socket resolves nothing).
    endpoint: Endpoint,
    /// Oldest first; the last one carries everything that is sent.
    sockets: Vec<T::Socket>,
    /// The only address any socket may reach (IP and port).
    peer: SocketAddr,
    /// When the newest socket first delivered an authenticated datagram.
    newest_worked_since: Option<Instant>,
    /// Whether the datagram last returned by `poll_recv` came in on the newest socket.
    last_on_newest: bool,
}

/// Whether `socket` is connected to exactly `peer` (IP and port; an IPv6 scope or flow label
/// is not part of the identity).
fn reaches<S: DatagramSocket>(socket: &S, peer: SocketAddr) -> bool {
    socket
        .peer_addr()
        .is_ok_and(|address| address.ip() == peer.ip() && address.port() == peer.port())
}

/// The IP literal a socket is opened from. A scoped IPv6 address keeps its scope id
/// (`fe80::1%3`, which the resolver reads numerically): without it a link-local peer is
/// unreachable, or reached through the wrong interface.
fn host_literal(peer: SocketAddr) -> String {
    match peer {
        SocketAddr::V6(v6) if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
        peer => peer.ip().to_string(),
    }
}

/// Errors a socket reports once and then recovers from: ICMP unreachable and its kin, and
/// interruptions. Anything else on an old socket means the socket is finished.
fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
    )
}

impl<T: DatagramTransport> Link<T> {
    /// Opens the first socket to `peer`, which every later socket must reach too. The first
    /// socket is checked as well: a transport that connects somewhere else is an error.
    pub(super) async fn open(transport: Arc<T>, peer: SocketAddr) -> io::Result<Self> {
        let endpoint = Endpoint::new(&host_literal(peer), peer.port())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let socket = transport.bind(&endpoint).await?;
        if !reaches(&socket, peer) {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "the socket reached an address other than the host's",
            ));
        }
        Ok(Self {
            transport,
            endpoint,
            sockets: vec![socket],
            peer,
            newest_worked_since: None,
            last_on_newest: false,
        })
    }

    /// The server's address family sizes the datagrams.
    pub(super) fn peer_is_ipv6(&self) -> bool {
        self.peer.is_ipv6()
    }

    /// Sends one datagram from the newest socket.
    pub(super) fn send(&self, datagram: &[u8]) -> io::Result<usize> {
        self.newest().try_send(datagram)
    }

    /// Polls every socket, newest first, for the next datagram.
    ///
    /// A failing socket never hides the others. An old socket that fails for good is dropped. The
    /// newest one's error is reported only when nothing else has a datagram, so a dead newest
    /// socket does not starve a live old one, and neither way round does an old one starve it.
    pub(super) fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let newest = self.sockets.len() - 1;
        let mut newest_error = None;
        let mut finished = Vec::new();
        let mut received = None;
        for index in (0..self.sockets.len()).rev() {
            match self.sockets[index].poll_recv(cx, buf) {
                Poll::Ready(Ok(length)) => {
                    received = Some((index == newest, length));
                    break;
                }
                Poll::Ready(Err(error)) if index == newest => newest_error = Some(error),
                Poll::Ready(Err(error)) => {
                    if !is_transient(&error) {
                        finished.push(index);
                    }
                }
                Poll::Pending => {}
            }
        }
        // Descending, so removing one does not move the next.
        for index in finished {
            self.sockets.remove(index);
        }
        match (received, newest_error) {
            (Some((on_newest, length)), _) => {
                self.last_on_newest = on_newest;
                Poll::Ready(Ok(length))
            }
            (None, Some(error)) => Poll::Ready(Err(error)),
            (None, None) => Poll::Pending,
        }
    }

    /// The next datagram if one is already waiting, without waiting for one.
    pub(super) fn try_recv(&mut self, buf: &mut [u8]) -> Option<io::Result<usize>> {
        let mut cx = Context::from_waker(Waker::noop());
        match self.poll_recv(&mut cx, buf) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        }
    }

    /// A future that opens the next socket. It owns what it needs, so the driver can wait on it
    /// (with a timeout, next to its commands) while the link keeps reading and sending, and
    /// hand the result to [`Link::adopt`].
    pub(super) fn next_socket(
        &self,
    ) -> impl Future<Output = io::Result<T::Socket>> + Send + use<T> {
        let transport = self.transport.clone();
        let endpoint = self.endpoint.clone();
        async move { transport.bind(&endpoint).await }
    }

    /// Sends from `socket` from now on, keeping the old ones to read from. Refused (and the link
    /// left exactly as it was, still sending from the socket it had: losing the ability to rotate
    /// is worth strictly less than the session) unless the socket reaches the pinned peer
    /// address, IP and port: only the local side may change when roaming. A socket that reaches
    /// another address (the name resolved differently, or to the other family of a dual-stack
    /// host) would send the session's datagrams where its `mosh-server` is not listening.
    pub(super) fn adopt(&mut self, socket: T::Socket) -> io::Result<()> {
        if !reaches(&socket, self.peer) {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "the new socket reaches an address other than the host's",
            ));
        }
        self.sockets.push(socket);
        self.newest_worked_since = None;
        self.last_on_newest = false;
        self.prune(Instant::now());
        Ok(())
    }

    /// Opens a new socket and adopts it, in one step. The driver does not use this (it must not
    /// wait on a resolver inline); it is the tests' shorthand.
    #[cfg(test)]
    pub(super) async fn rebind(&mut self) -> io::Result<()> {
        let socket = self.next_socket().await?;
        self.adopt(socket)
    }

    /// An authenticated datagram arrived: the one `poll_recv` last returned was good. If it came
    /// in on the newest socket, that socket has now worked, and once it has for long enough the
    /// old ones are retired (see [`Link::prune`]).
    pub(super) fn authenticated(&mut self, now: Instant) {
        if std::mem::take(&mut self.last_on_newest) {
            self.newest_worked_since.get_or_insert(now);
        }
        self.prune(now);
    }

    /// Closes sockets that can no longer be useful: all the old ones once the newest has worked
    /// for [`MAX_OLD_SOCKET_AGE`], and the oldest beyond [`MAX_PORTS_OPEN`].
    pub(super) fn prune(&mut self, now: Instant) {
        if self.sockets.len() <= 1 {
            return;
        }
        if let Some(since) = self.newest_worked_since
            && now.saturating_duration_since(since) > MAX_OLD_SOCKET_AGE
        {
            let drop_to = self.sockets.len() - 1;
            self.sockets.drain(..drop_to);
            return;
        }
        if self.sockets.len() > MAX_PORTS_OPEN {
            let drop_to = self.sockets.len() - MAX_PORTS_OPEN;
            self.sockets.drain(..drop_to);
        }
    }

    /// The local port packets are leaving from right now. It CHANGES over the life of a
    /// session, and that is the point.
    #[cfg(test)]
    pub(super) fn local_port(&self) -> Option<u16> {
        self.newest()
            .local_addr()
            .ok()
            .map(|address| address.port())
    }

    #[cfg(test)]
    pub(super) fn open_sockets(&self) -> usize {
        self.sockets.len()
    }

    fn newest(&self) -> &T::Socket {
        self.sockets.last().expect("a link always has a socket")
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use tokio::net::UdpSocket;

    use crate::transport::DirectUdp;

    use super::*;

    async fn link() -> (Link<DirectUdp>, UdpSocket) {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = server.local_addr().unwrap();
        let link = Link::open(Arc::new(DirectUdp), peer).await.unwrap();
        (link, server)
    }

    #[tokio::test]
    async fn a_link_starts_on_one_port() {
        let (link, _server) = link().await;
        assert_eq!(link.open_sockets(), 1);
        assert!(link.local_port().is_some());
        assert!(!link.peer_is_ipv6());
    }

    #[tokio::test]
    async fn a_rebind_sends_from_a_new_port_and_keeps_reading_the_old_one() {
        let (mut link, server) = link().await;
        let before = link.local_port().unwrap();
        link.send(b"one").unwrap();
        let mut buf = [0u8; 16];
        let (n, first_from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], first_from.port()), (&b"one"[..], before));

        link.rebind().await.unwrap();
        let after = link.local_port().unwrap();
        assert_ne!(
            before, after,
            "the client should be sending from somewhere new"
        );
        // And the old one stays open: a reply to something sent from it comes back to IT.
        assert_eq!(link.open_sockets(), 2);
        link.send(b"two").unwrap();
        let (n, second_from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], second_from.port()), (&b"two"[..], after));

        server.send_to(b"late reply", first_from).await.unwrap();
        let n = poll_fn(|cx| link.poll_recv(cx, &mut buf)).await.unwrap();
        assert_eq!(&buf[..n], b"late reply");
        server.send_to(b"fresh reply", second_from).await.unwrap();
        let n = poll_fn(|cx| link.poll_recv(cx, &mut buf)).await.unwrap();
        assert_eq!(&buf[..n], b"fresh reply");
    }

    #[tokio::test]
    async fn try_recv_does_not_wait() {
        let (mut link, server) = link().await;
        let mut buf = [0u8; 16];
        assert!(link.try_recv(&mut buf).is_none());
        link.send(b"x").unwrap();
        let (_, from) = server.recv_from(&mut buf).await.unwrap();
        server.send_to(b"y", from).await.unwrap();
        // Loopback delivery is not instantaneous from the reactor's point of view.
        let n = poll_fn(|cx| link.poll_recv(cx, &mut buf)).await.unwrap();
        assert_eq!(&buf[..n], b"y");
        assert!(link.try_recv(&mut buf).is_none());
    }

    #[tokio::test]
    async fn the_client_never_reads_from_more_than_ten_ports() {
        let (mut link, _server) = link().await;
        for _ in 0..20 {
            link.rebind().await.unwrap();
        }
        assert_eq!(link.open_sockets(), MAX_PORTS_OPEN);
        // The one being sent from is always the newest, never a survivor of the pruning.
        assert!(link.local_port().is_some());
    }

    /// Has the server answer the datagram `from` (a port of the link) and the link read it.
    async fn answered_on(link: &mut Link<DirectUdp>, server: &UdpSocket, port: u16) {
        let to = SocketAddr::from(([127, 0, 0, 1], port));
        server.send_to(b"answer", to).await.unwrap();
        let mut buf = [0u8; 16];
        poll_fn(|cx| link.poll_recv(cx, &mut buf)).await.unwrap();
    }

    #[tokio::test]
    async fn a_socket_that_has_worked_long_enough_retires_the_old_ones() {
        let (mut link, server) = link().await;
        link.rebind().await.unwrap();
        link.rebind().await.unwrap();
        assert_eq!(link.open_sockets(), 3);
        let newest = link.local_port();

        // The server answers on the newest socket: it has worked.
        answered_on(&mut link, &server, newest.unwrap()).await;
        let now = Instant::now();
        link.authenticated(now);
        assert_eq!(link.open_sockets(), 3, "not yet: a minute has not passed");

        // Nothing can still be arriving at ports abandoned a minute ago.
        link.authenticated(now + MAX_OLD_SOCKET_AGE + Duration::from_secs(1));
        assert_eq!(link.open_sockets(), 1);
        assert_eq!(link.local_port(), newest, "the survivor is the newest");
    }

    #[tokio::test]
    async fn a_newest_socket_that_never_received_anything_does_not_retire_the_old_ones() {
        let (mut link, server) = link().await;
        let old = link.local_port().unwrap();
        // The network changed but the new path does not carry anything yet.
        link.rebind().await.unwrap();
        assert_eq!(link.open_sockets(), 2);

        // The server keeps answering the old port: authenticated datagrams, none on the newest.
        let later = Instant::now() + MAX_OLD_SOCKET_AGE * 3;
        answered_on(&mut link, &server, old).await;
        link.authenticated(later);
        link.prune(later);
        assert_eq!(
            link.open_sockets(),
            2,
            "the socket that works is still read"
        );
    }

    #[tokio::test]
    async fn pruning_a_lone_socket_leaves_it_alone() {
        let (mut link, _server) = link().await;
        let port = link.local_port();
        link.newest_worked_since = Some(Instant::now());
        link.prune(Instant::now() + MAX_OLD_SOCKET_AGE * 10);
        assert_eq!(link.open_sockets(), 1);
        assert_eq!(link.local_port(), port);
    }

    /// What a scripted socket does when polled.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        /// Nothing arrives.
        Silent,
        /// A datagram is always waiting.
        Chatty,
        /// A persistent failure.
        Dead,
        /// The one-off ICMP kind of failure.
        Refused,
    }

    struct Scripted {
        mode: Mode,
        peer: SocketAddr,
    }

    impl DatagramSocket for Scripted {
        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(SocketAddr::from(([127, 0, 0, 1], 1)))
        }

        fn peer_addr(&self) -> io::Result<SocketAddr> {
            Ok(self.peer)
        }

        fn try_send(&self, datagram: &[u8]) -> io::Result<usize> {
            Ok(datagram.len())
        }

        fn poll_recv(&self, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
            match self.mode {
                Mode::Silent => Poll::Pending,
                Mode::Chatty => {
                    buf[..2].copy_from_slice(b"hi");
                    Poll::Ready(Ok(2))
                }
                Mode::Dead => Poll::Ready(Err(io::Error::from(io::ErrorKind::BrokenPipe))),
                Mode::Refused => {
                    Poll::Ready(Err(io::Error::from(io::ErrorKind::ConnectionRefused)))
                }
            }
        }
    }

    fn address(ipv6: bool) -> SocketAddr {
        if ipv6 {
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 9))
        } else {
            SocketAddr::from(([127, 0, 0, 1], 9))
        }
    }

    /// Hands out scripted sockets in order: `(mode, the address it reaches)`.
    struct ScriptedTransport(std::sync::Mutex<std::collections::VecDeque<(Mode, SocketAddr)>>);

    impl ScriptedTransport {
        /// `(mode, ipv6)`: every socket reaches the same address of that family.
        fn new(script: &[(Mode, bool)]) -> Arc<Self> {
            Self::with_peers(
                &script
                    .iter()
                    .map(|(mode, ipv6)| (*mode, address(*ipv6)))
                    .collect::<Vec<_>>(),
            )
        }

        fn with_peers(script: &[(Mode, SocketAddr)]) -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(
                script.iter().copied().collect(),
            )))
        }
    }

    impl DatagramTransport for ScriptedTransport {
        type Socket = Scripted;

        async fn bind(&self, _endpoint: &Endpoint) -> io::Result<Scripted> {
            let (mode, peer) = self.0.lock().unwrap().pop_front().expect("scripted socket");
            Ok(Scripted { mode, peer })
        }
    }

    async fn scripted(script: &[(Mode, bool)]) -> Link<ScriptedTransport> {
        let mut link = Link::open(ScriptedTransport::new(script), address(false))
            .await
            .unwrap();
        for _ in 1..script.len() {
            link.rebind().await.unwrap();
        }
        link
    }

    #[tokio::test]
    async fn an_old_socket_that_failed_for_good_is_dropped_and_does_not_hide_the_newest() {
        let mut link = scripted(&[(Mode::Dead, false), (Mode::Silent, false)]).await;
        assert_eq!(link.open_sockets(), 2);
        let mut buf = [0u8; 16];
        // Nothing to report: the dead one is not the newest, so its failure is not surfaced.
        assert!(link.try_recv(&mut buf).is_none());
        assert_eq!(link.open_sockets(), 1, "the dead socket is gone");
        assert!(link.try_recv(&mut buf).is_none());
    }

    #[tokio::test]
    async fn a_dead_newest_socket_does_not_starve_an_old_one_with_data() {
        // Oldest first: the old one has a datagram waiting, the newest only fails.
        let mut link = scripted(&[(Mode::Chatty, false), (Mode::Dead, false)]).await;
        let mut buf = [0u8; 16];
        let length = link.try_recv(&mut buf).unwrap().unwrap();
        assert_eq!(&buf[..length], b"hi");
        assert!(!link.last_on_newest);
        assert_eq!(link.open_sockets(), 2, "the newest is never dropped");

        // With nothing else to read, the newest's own error is reported.
        let mut link = scripted(&[(Mode::Silent, false), (Mode::Dead, false)]).await;
        assert!(link.try_recv(&mut buf).unwrap().is_err());
    }

    #[tokio::test]
    async fn a_one_off_error_on_an_old_socket_does_not_cost_the_socket() {
        let mut link = scripted(&[(Mode::Refused, false), (Mode::Silent, false)]).await;
        let mut buf = [0u8; 16];
        assert!(link.try_recv(&mut buf).is_none());
        assert_eq!(link.open_sockets(), 2);
    }

    #[tokio::test]
    async fn a_socket_on_the_other_address_family_is_refused() {
        let mut link = scripted(&[(Mode::Silent, false)]).await;
        assert!(!link.peer_is_ipv6());
        let flipped = Scripted {
            mode: Mode::Silent,
            peer: address(true),
        };
        assert!(link.adopt(flipped).is_err());
        assert_eq!(link.open_sockets(), 1, "the link is as it was");
        assert!(!link.peer_is_ipv6());
    }

    fn at(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), port)
    }

    #[tokio::test]
    async fn a_scoped_ipv6_peer_is_opened_with_its_scope_id() {
        use std::net::{Ipv6Addr, SocketAddrV6};
        let scoped = SocketAddr::V6(SocketAddrV6::new(
            "fe80::1".parse::<Ipv6Addr>().unwrap(),
            60001,
            0,
            3,
        ));
        assert_eq!(host_literal(scoped), "fe80::1%3");
        // Without a scope, and for IPv4, the plain literal.
        assert_eq!(host_literal(at("fe80::1", 9)), "fe80::1");
        assert_eq!(host_literal(at("10.0.0.2", 9)), "10.0.0.2");
        // What `DirectUdp` does with the literal: the resolver reads it numerically (no
        // lookup) and the address keeps the scope.
        let resolved = tokio::net::lookup_host((host_literal(scoped), 60001))
            .await
            .unwrap()
            .next()
            .unwrap();
        assert_eq!(resolved, scoped);
    }

    #[tokio::test]
    async fn rotating_answers_of_the_same_family_never_move_the_session() {
        // The first resolution reached A; later ones (a round-robin name, a changed record)
        // answer B, C and then A again. Only the local side may change when roaming.
        let (a, b, c) = (at("127.0.0.1", 9), at("127.0.0.2", 9), at("127.0.0.3", 9));
        let transport = ScriptedTransport::with_peers(&[
            (Mode::Silent, a),
            (Mode::Silent, b),
            (Mode::Silent, c),
            (Mode::Silent, a),
        ]);
        let mut link = Link::open(transport, a).await.unwrap();
        for _ in 0..2 {
            assert!(link.rebind().await.is_err(), "another IP is refused");
            assert_eq!(link.open_sockets(), 1, "the link is as it was");
        }
        link.rebind().await.unwrap();
        assert_eq!(
            link.open_sockets(),
            2,
            "the pinned address is accepted again"
        );
    }

    #[tokio::test]
    async fn a_socket_that_reaches_another_port_or_the_first_socket_elsewhere_is_refused() {
        let a = at("127.0.0.1", 9);
        let transport = ScriptedTransport::with_peers(&[
            (Mode::Silent, a),
            (Mode::Silent, at("127.0.0.1", 10)),
        ]);
        let mut link = Link::open(transport, a).await.unwrap();
        assert!(link.rebind().await.is_err(), "the same IP on another port");
        assert_eq!(link.open_sockets(), 1);
        // The very first socket is held to the pinned address too.
        let elsewhere = ScriptedTransport::with_peers(&[(Mode::Silent, at("127.0.0.2", 9))]);
        assert!(Link::open(elsewhere, a).await.is_err());
    }

    /// DNS that changes its answer: the first bind connects where asked, later ones to a
    /// different loopback address.
    struct Rotating(std::sync::atomic::AtomicUsize);

    impl DatagramTransport for Rotating {
        type Socket = tokio::net::UdpSocket;

        async fn bind(&self, endpoint: &Endpoint) -> io::Result<Self::Socket> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                DirectUdp.bind(endpoint).await
            } else {
                DirectUdp
                    .bind(&Endpoint::new("127.0.0.2", endpoint.port()).unwrap())
                    .await
            }
        }
    }

    #[tokio::test]
    async fn real_sockets_keep_sending_to_the_pinned_server_when_the_name_moves() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = server.local_addr().unwrap();
        let mut link = Link::open(Arc::new(Rotating(Default::default())), peer)
            .await
            .unwrap();
        let first = link.local_port().unwrap();
        assert!(link.rebind().await.is_err());
        assert_eq!(link.open_sockets(), 1);
        assert_eq!(link.local_port(), Some(first));
        link.send(b"still here").unwrap();
        let mut buf = [0u8; 32];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], from.port()), (&b"still here"[..], first));
    }

    #[test]
    fn a_too_large_error_is_recognised() {
        assert!(is_too_large(&io::Error::from_raw_os_error(EMSGSIZE)));
        assert!(!is_too_large(&io::Error::from(io::ErrorKind::WouldBlock)));
    }
}
