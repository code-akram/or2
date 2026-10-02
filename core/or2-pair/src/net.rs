//! The one socket `or2-pair` opens: a look at this machine's own sshd, behind a small trait.
//!
//! AGENTS.md says the app's network traffic all goes through the `Transport` trait of `or2-core`.
//! This crate is a host-side tool, not the app's network path: it has no phone, no overlay
//! binding and no async runtime, and it does not depend on `or2-core`. It listens on nothing (the
//! pairing runs over sshd itself); its only connection is the banner probe below, kept in this
//! file so that the rule "no direct `TcpStream::connect` outside the network implementation"
//! holds here too. The phone side of the exchange uses `or2-core`'s `Transport`.

use std::io::{self, Read};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// The host's network, as the CLI uses it.
pub trait Net {
    /// Connects to this machine's own sshd on `port` and returns its banner line.
    fn probe_ssh(&self, port: u16) -> io::Result<String>;
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
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;

    use super::*;

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
