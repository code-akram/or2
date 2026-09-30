//! Network paths. SSH (and later mosh) reach hosts only through [`Transport`].
//!
//! A transport turns an [`Endpoint`] into a byte stream. Its `Stream` bound is exactly the bound
//! `russh::client::connect_stream` requires, so any transport can carry an SSH connection.
//! M1 has one implementation, [`DirectTcp`]. Address racing (M2), jump hosts and UDP for mosh
//! (M3) are added as further implementations or methods when those milestones need them.

use std::future::Future;
use std::io;
use std::num::NonZeroU16;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
