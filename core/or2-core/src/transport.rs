//! Network paths. SSH (and later mosh) reach hosts only through [`Transport`].
//!
//! A transport turns an [`Endpoint`] into a byte stream. Its `Stream` bound is exactly the bound
//! `russh::client::connect_stream` requires, so any transport can carry an SSH connection.
//! M1 has one implementation, [`DirectTcp`]. [`race`] connects to a host's several addresses
//! over any transport. Jump hosts and UDP for mosh (M3) are added as further implementations
//! or methods when those milestones need them.

use std::fmt;
use std::future::Future;
use std::io;
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
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

/// Each further address starts this long after the previous one started, unless the previous
/// one failed first.
pub const RACE_STAGGER: Duration = Duration::from_millis(250);

/// The winner of a [`race`]: its position in the address list and its stream.
#[derive(Debug)]
pub struct Raced<S> {
    pub index: usize,
    pub stream: S,
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
    let mut attempts: JoinSet<(usize, io::Result<T::Stream>)> = JoinSet::new();
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
                attempts.spawn(async move { (index, transport.connect(&endpoint).await) });
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
                    Ok(stream) => return Ok(Raced { index, stream }),
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
