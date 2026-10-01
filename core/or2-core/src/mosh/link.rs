//! The session's sockets, opened and rebound only through a [`DatagramTransport`].
//!
//! This is where upstream mosh-rs kept its `Vec<UdpSocket>`; it is the half of its session that
//! touched the OS, moved out so the protocol code (`ssp::session`) can stay free of I/O.
//!
//! Packets go out the NEWEST socket. All of them are read, because a reply to something sent
//! from an older port comes back to that port, which is what keeps a source-port rotation from
//! costing a round trip.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::transport::{DatagramSocket, DatagramTransport, Endpoint};

/// How many old sockets to keep reading from at once.
const MAX_PORTS_OPEN: usize = 10;
/// A newest socket that has worked this long makes the old ones pointless: nothing in flight can
/// still be coming back to them.
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
    endpoint: Endpoint,
    /// Oldest first; the last one carries everything that is sent.
    sockets: Vec<T::Socket>,
    /// When the newest socket was opened.
    newest_since: Instant,
}

impl<T: DatagramTransport> Link<T> {
    pub(super) async fn open(transport: Arc<T>, endpoint: Endpoint) -> io::Result<Self> {
        let socket = transport.bind(&endpoint).await?;
        Ok(Self {
            transport,
            endpoint,
            sockets: vec![socket],
            newest_since: Instant::now(),
        })
    }

    /// The server's address family sizes the datagrams.
    pub(super) fn peer_is_ipv6(&self) -> bool {
        self.newest()
            .peer_addr()
            .is_ok_and(|address: SocketAddr| address.is_ipv6())
    }

    /// Sends one datagram from the newest socket.
    pub(super) fn send(&self, datagram: &[u8]) -> io::Result<usize> {
        self.newest().try_send(datagram)
    }

    /// Polls every socket, oldest first, for the next datagram.
    pub(super) fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        for socket in &self.sockets {
            if let ready @ Poll::Ready(_) = socket.poll_recv(cx, buf) {
                return ready;
            }
        }
        Poll::Pending
    }

    /// The next datagram if one is already waiting, without waiting for one.
    pub(super) fn try_recv(&self, buf: &mut [u8]) -> Option<io::Result<usize>> {
        let mut cx = Context::from_waker(Waker::noop());
        match self.poll_recv(&mut cx, buf) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        }
    }

    /// Opens a new socket and sends from it from now on, keeping the old ones to read from. A
    /// failure leaves the link exactly as it was, still sending from the socket it had: losing
    /// the ability to rotate is worth strictly less than the session.
    pub(super) async fn rebind(&mut self) -> io::Result<()> {
        let socket = self.transport.bind(&self.endpoint).await?;
        self.sockets.push(socket);
        self.newest_since = Instant::now();
        self.prune(self.newest_since);
        Ok(())
    }

    /// Closes sockets that can no longer be useful. Called on every successful receive: that
    /// call site is the only thing that makes the age rule reachable, since a rebind refreshes
    /// `newest_since` on its way in.
    pub(super) fn prune(&mut self, now: Instant) {
        if self.sockets.len() <= 1 {
            return;
        }
        if now.saturating_duration_since(self.newest_since) > MAX_OLD_SOCKET_AGE {
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
        let endpoint = Endpoint::new("127.0.0.1", server.local_addr().unwrap().port()).unwrap();
        let link = Link::open(Arc::new(DirectUdp), endpoint).await.unwrap();
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
        let (link, server) = link().await;
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

    #[tokio::test]
    async fn a_socket_that_has_worked_long_enough_retires_the_old_ones() {
        let (mut link, _server) = link().await;
        link.rebind().await.unwrap();
        link.rebind().await.unwrap();
        assert_eq!(link.open_sockets(), 3);
        let newest = link.local_port();

        // This is the call site a successful receive makes. Nothing can still be arriving at
        // ports abandoned a minute ago.
        link.prune(link.newest_since + MAX_OLD_SOCKET_AGE + Duration::from_secs(1));
        assert_eq!(link.open_sockets(), 1);
        assert_eq!(link.local_port(), newest, "the survivor is the newest");
    }

    #[tokio::test]
    async fn pruning_a_lone_socket_leaves_it_alone() {
        let (mut link, _server) = link().await;
        let port = link.local_port();
        link.prune(link.newest_since + MAX_OLD_SOCKET_AGE * 10);
        assert_eq!(link.open_sockets(), 1);
        assert_eq!(link.local_port(), port);
    }

    #[test]
    fn a_too_large_error_is_recognised() {
        assert!(is_too_large(&io::Error::from_raw_os_error(EMSGSIZE)));
        assert!(!is_too_large(&io::Error::from(io::ErrorKind::WouldBlock)));
    }
}
