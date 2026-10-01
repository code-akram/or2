//! Every socket `or2-pair` opens, behind one small trait.
//!
//! AGENTS.md says the app's network traffic all goes through the `Transport` trait of `or2-core`.
//! This crate is a host-side tool, not the app's network path: it has no phone, no overlay
//! binding and no async runtime, and it does not depend on `or2-core`. It still keeps its own
//! sockets in one place, so that the listener is replaceable in tests and the rule "no direct
//! `TcpStream::connect` or `TcpListener::bind` outside the network implementation" holds here
//! too: only this file touches `std::net`'s sockets. The phone side of the exchange uses
//! `or2-core`'s `Transport` (see `or2_core::pair`).

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// One accepted connection.
pub trait Connection: Read + Write {
    /// Who connected, for the screen (`192.168.1.50:51234`).
    fn peer(&self) -> String;
    /// The peer's IP address, when it has one.
    fn peer_ip(&self) -> Option<IpAddr> {
        self.peer().parse::<SocketAddr>().ok().map(|peer| peer.ip())
    }
    /// Both the read and the write timeout.
    fn set_timeout(&mut self, timeout: Duration) -> io::Result<()>;
}

/// A bound listener, possibly on several addresses with one port.
pub trait PairListener {
    /// The addresses it listens on.
    fn endpoints(&self) -> Vec<SocketAddr>;
    /// The next connection, or `None` once `deadline` has passed.
    fn accept(&mut self, deadline: Instant) -> io::Result<Option<Box<dyn Connection>>>;
}

/// The host's network, as the CLI uses it.
pub trait Net {
    /// Connects to this machine's own sshd on `port` and returns its banner line.
    fn probe_ssh(&self, port: u16) -> io::Result<String>;
    /// Listens on every address in `ips` with one shared port (`port` 0 picks a free one).
    fn listen(&self, ips: &[IpAddr], port: u16) -> io::Result<Box<dyn PairListener>>;
}

/// The operating system's sockets.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdNet;

impl Net for StdNet {
    fn probe_ssh(&self, port: u16) -> io::Result<String> {
        let addresses = [
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
        ];
        let mut last = io::Error::from(io::ErrorKind::NotFound);
        for address in addresses {
            match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
                Ok(mut stream) => {
                    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                    let mut banner = [0u8; 255];
                    let count = stream.read(&mut banner)?;
                    let text = String::from_utf8_lossy(&banner[..count]);
                    return Ok(text.lines().next().unwrap_or_default().trim().to_owned());
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn listen(&self, ips: &[IpAddr], port: u16) -> io::Result<Box<dyn PairListener>> {
        let Some(first) = ips.first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no address to listen on",
            ));
        };
        // One port on every address: bind the first (a random port when none was asked for),
        // then the others on that port. If another process took the port on a later address,
        // start over with a new random one.
        let attempts = if port == 0 { 8 } else { 1 };
        let mut last = io::Error::from(io::ErrorKind::AddrInUse);
        for _ in 0..attempts {
            match bind_all(first, &ips[1..], port) {
                Ok(listener) => return Ok(Box::new(listener)),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}

fn bind_all(first: &IpAddr, rest: &[IpAddr], port: u16) -> io::Result<TcpPairListener> {
    let head = TcpListener::bind(SocketAddr::new(*first, port))?;
    let port = head.local_addr()?.port();
    let mut listeners = vec![head];
    for ip in rest {
        listeners.push(TcpListener::bind(SocketAddr::new(*ip, port))?);
    }
    for listener in &listeners {
        listener.set_nonblocking(true)?;
    }
    Ok(TcpPairListener { listeners })
}

/// Plain TCP listeners polled in turn: no runtime, no threads.
pub struct TcpPairListener {
    listeners: Vec<TcpListener>,
}

impl PairListener for TcpPairListener {
    fn endpoints(&self) -> Vec<SocketAddr> {
        self.listeners
            .iter()
            .filter_map(|listener| listener.local_addr().ok())
            .collect()
    }

    fn accept(&mut self, deadline: Instant) -> io::Result<Option<Box<dyn Connection>>> {
        loop {
            for listener in &self.listeners {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        stream.set_nonblocking(false)?;
                        let _ = stream.set_nodelay(true);
                        return Ok(Some(Box::new(TcpConnection { stream, peer })));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(20)));
        }
    }
}

struct TcpConnection {
    stream: TcpStream,
    peer: SocketAddr,
}

impl Read for TcpConnection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for TcpConnection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl Connection for TcpConnection {
    fn peer(&self) -> String {
        self.peer.to_string()
    }

    fn set_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        self.stream.set_read_timeout(Some(timeout))?;
        self.stream.set_write_timeout(Some(timeout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loopback() -> IpAddr {
        IpAddr::from([127, 0, 0, 1])
    }

    #[test]
    fn listens_on_a_random_port_and_accepts_a_connection() {
        let mut listener = StdNet.listen(&[loopback()], 0).unwrap();
        let endpoint = listener.endpoints()[0];
        assert_ne!(endpoint.port(), 0);
        assert_eq!(endpoint.ip(), loopback());
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(endpoint).unwrap();
            stream.write_all(b"hi").unwrap();
        });
        let mut connection = listener
            .accept(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        connection.set_timeout(Duration::from_secs(5)).unwrap();
        let mut buf = [0u8; 2];
        connection.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hi");
        assert!(connection.peer().starts_with("127.0.0.1:"));
        client.join().unwrap();
    }

    #[test]
    fn accept_returns_none_at_the_deadline() {
        let mut listener = StdNet.listen(&[loopback()], 0).unwrap();
        let started = Instant::now();
        let accepted = listener
            .accept(started + Duration::from_millis(100))
            .unwrap();
        assert!(accepted.is_none());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn an_address_that_is_not_ours_fails_to_bind() {
        // 203.0.113.0/24 is documentation space: no interface owns it.
        assert!(StdNet.listen(&[IpAddr::from([203, 0, 113, 9])], 0).is_err());
        assert!(StdNet.listen(&[], 0).is_err());
    }

    #[test]
    fn probe_reads_a_banner() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"SSH-2.0-Test_1.0\r\nignored").unwrap();
        });
        assert_eq!(StdNet.probe_ssh(port).unwrap(), "SSH-2.0-Test_1.0");
        server.join().unwrap();
    }

    #[test]
    fn probe_of_a_closed_port_is_an_error() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        assert!(StdNet.probe_ssh(port).is_err());
    }
}
