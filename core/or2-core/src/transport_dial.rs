//! One endpoint's TCP connection: resolve its name, then race the addresses it resolved to.
//!
//! [`DirectTcp`](super::DirectTcp) used to hand the name to `TcpStream::connect`, which resolves
//! it and tries each address in turn with no limit of its own. That fails in ways a phone meets:
//! the first `.local` (mDNS) lookup of an app can fail after a second or two and succeed on the
//! next try, a name can resolve to an IPv6 link-local address that no app socket can use without
//! a scope, and one dead address in the middle holds everything behind it. This module resolves
//! once (retrying a `.local` name), drops what cannot work, and races the rest the way
//! [`race`](super::race) races a host's addresses, so a person reads *why* an endpoint failed.
//!
//! Resolution and connection are traits ([`Resolver`], [`Connector`]) so every rule is tested
//! against a scripted resolver on a paused clock.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::{TcpStream, lookup_host};
use tokio::time::{Instant, sleep, timeout_at};

use super::{ADDRESS_TIMEOUT, Endpoint, RACE_STAGGER, race_attempts, seconds};

/// Turns a host name or IP literal into addresses.
pub trait Resolver: Send + Sync + 'static {
    fn resolve(
        &self,
        host: &str,
        port: u16,
    ) -> impl Future<Output = io::Result<Vec<SocketAddr>>> + Send;
}

/// The operating system's resolver (`getaddrinfo`, which on Android also answers `.local`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        Ok(lookup_host((host, port)).await?.collect())
    }
}

/// Opens one connection to one resolved address.
pub trait Connector: Clone + Send + Sync + 'static {
    type Stream: Send + 'static;

    fn connect(&self, address: SocketAddr)
    -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

/// OS TCP sockets.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpConnector;

impl Connector for TcpConnector {
    type Stream = TcpStream;

    async fn connect(&self, address: SocketAddr) -> io::Result<TcpStream> {
        let stream = TcpStream::connect(address).await?;
        // Interactive keystrokes are tiny writes; do not let Nagle delay them.
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

/// The timing of one endpoint's connection.
#[derive(Debug, Clone, Copy)]
pub struct DialTiming {
    /// Everything for this endpoint, resolution included. Slightly under [`ADDRESS_TIMEOUT`]
    /// (the race's own limit for the address) so the endpoint reports why it failed before the
    /// race gives up on it.
    pub budget: Duration,
    /// Between the starts of the resolved addresses (Happy Eyeballs).
    pub stagger: Duration,
    /// How many times a `.local` name is resolved before it is called unresolved.
    pub local_tries: u32,
    /// The longest all of a `.local` name's tries may take together.
    pub local_window: Duration,
    /// Between a `.local` try that failed and the next one.
    pub local_pause: Duration,
}

impl Default for DialTiming {
    fn default() -> Self {
        Self {
            budget: ADDRESS_TIMEOUT - Duration::from_secs(1),
            stagger: RACE_STAGGER,
            local_tries: 3,
            local_window: Duration::from_secs(4),
            local_pause: Duration::from_millis(250),
        }
    }
}

/// Whether `host` is an mDNS name (`blackstark.local`, `Name.LOCAL.`).
fn is_local_name(host: &str) -> bool {
    host.trim_end_matches('.')
        .to_ascii_lowercase()
        .ends_with(".local")
}

/// An IPv6 link-local address (`fe80::/10`) with no scope id: no socket can be connected to it
/// (the kernel does not know which interface), and the name that produced it usually has an
/// IPv4 address too.
fn is_unscoped_link_local(address: &SocketAddr) -> bool {
    match address {
        SocketAddr::V6(v6) => (v6.ip().segments()[0] & 0xffc0) == 0xfe80 && v6.scope_id() == 0,
        SocketAddr::V4(_) => false,
    }
}

/// Alternates the address families, starting with the family of the first address (the
/// resolver's own preference), each family in its original order.
fn interleave(addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addresses.first().copied() else {
        return addresses;
    };
    let (mut same, mut other): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .partition(|address| address.is_ipv4() == first.is_ipv4());
    same.reverse();
    other.reverse();
    let mut out = Vec::with_capacity(same.len() + other.len());
    loop {
        match (same.pop(), other.pop()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b)),
        }
    }
}

/// The error for a name that did not resolve. `.local` names say so, and how hard they were
/// tried, because the cause is usually that the host is asleep or off the network.
fn unresolved(mdns: bool, tries: u32, window_over: bool, window: Duration) -> io::Error {
    let what = match (mdns, window_over) {
        (false, false) => "name not resolved".to_owned(),
        (false, true) => format!("name not resolved within {}", seconds(window)),
        (true, false) => format!(
            "name not resolved (mDNS) after {tries} {}",
            if tries == 1 { "try" } else { "tries" }
        ),
        (true, true) => format!("name not resolved (mDNS) within {}", seconds(window)),
    };
    io::Error::new(io::ErrorKind::NotFound, what)
}

/// Resolves `endpoint` and returns what can be connected to, interleaved. A `.local` name is
/// resolved up to [`DialTiming::local_tries`] times within [`DialTiming::local_window`]; any
/// other name once. Everything stays inside `deadline`.
async fn resolve<R: Resolver>(
    resolver: &R,
    endpoint: &Endpoint,
    timing: &DialTiming,
    deadline: Instant,
) -> io::Result<Vec<SocketAddr>> {
    let mdns = is_local_name(endpoint.host());
    let (tries, window) = if mdns {
        (timing.local_tries.max(1), timing.local_window)
    } else {
        (1, timing.budget)
    };
    let window_end = (Instant::now() + window).min(deadline);
    let mut made = 0;
    let found = loop {
        made += 1;
        match timeout_at(
            window_end,
            resolver.resolve(endpoint.host(), endpoint.port()),
        )
        .await
        {
            Ok(Ok(found)) if !found.is_empty() => break found,
            Ok(_) => {
                // An error, or no address at all: try again while there are tries and time.
                if made >= tries || Instant::now() + timing.local_pause >= window_end {
                    return Err(unresolved(mdns, made, false, window));
                }
                sleep(timing.local_pause).await;
            }
            Err(_) => return Err(unresolved(mdns, made, true, window)),
        }
    };
    let total = found.len();
    let usable: Vec<SocketAddr> = found
        .into_iter()
        .filter(|address| !is_unscoped_link_local(address))
        .collect();
    if usable.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!(
                "the name resolved only to IPv6 link-local {} without a scope, which an app \
                 cannot connect to",
                if total == 1 { "address" } else { "addresses" }
            ),
        ));
    }
    Ok(interleave(usable))
}

/// What several addresses of one endpoint said, as one error: the kind of the first failure and
/// the distinct outcomes in words (`connection refused, no answer within 5 s`).
fn combine(errors: Vec<io::Error>) -> io::Error {
    let kind = errors
        .iter()
        .map(io::Error::kind)
        .find(|kind| *kind != io::ErrorKind::TimedOut)
        .unwrap_or(io::ErrorKind::TimedOut);
    if errors.len() == 1 {
        let error = errors.into_iter().next().expect("one error");
        return error;
    }
    let mut words: Vec<String> = Vec::new();
    for error in &errors {
        let word = super::describe_error(error);
        if !words.contains(&word) {
            words.push(word);
        }
    }
    io::Error::new(
        kind,
        format!("{} addresses: {}", errors.len(), words.join(", ")),
    )
}

/// Connects to `endpoint`: resolves it, drops what cannot work, and races the resolved addresses
/// ([`DialTiming::stagger`] apart, families alternating, each limited by what is left of the
/// budget). Returns the stream and the address it reached. Dropping the future cancels every
/// attempt.
pub async fn dial<R: Resolver, C: Connector>(
    resolver: &R,
    connector: &C,
    endpoint: &Endpoint,
    timing: DialTiming,
) -> io::Result<(C::Stream, SocketAddr)> {
    let deadline = Instant::now() + timing.budget;
    let addresses = resolve(resolver, endpoint, &timing, deadline).await?;
    let attempts = addresses.clone();
    let connector = connector.clone();
    let budget = timing.budget;
    // Every address is limited by the same deadline, wherever its start falls in the stagger;
    // the race's own per-attempt limit is only a backstop past the last start.
    let backstop = budget + timing.stagger * addresses.len() as u32;
    race_attempts(
        addresses.len(),
        timing.stagger,
        backstop,
        |_, _| {},
        move |index| {
            let connector = connector.clone();
            let address = attempts[index];
            async move {
                match timeout_at(deadline, connector.connect(address)).await {
                    Ok(connected) => connected.map(|stream| (stream, address)),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("no answer within {}", seconds(budget)),
                    )),
                }
            }
        },
    )
    .await
    .map(|(_, reached)| reached)
    .map_err(combine)
}

#[cfg(test)]
#[path = "transport_dial_tests.rs"]
mod tests;
