//! Waking a sleeping host before connecting to it: the Wake-on-LAN magic packet and the TCP wake
//! probe. See docs/design.md, M4 backlog.
//!
//! The magic packet is a UDP broadcast on the local link, so it reaches a host only on the same
//! LAN and only when its network card keeps listening while it sleeps ("Wake for network access" on
//! a Mac). The probe is a TCP connection attempt to the host's SSH port: a Mac that sleeps behind a
//! Bonjour Sleep Proxy (an Apple TV, a HomePod) is woken by the proxy when someone knocks on a port
//! it advertised. It needs no MAC address and can work beyond the broadcast domain.
//!
//! Neither reports whether the host woke: only the connection that follows can tell.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::transport::{DatagramBroadcast, Endpoint, Transport};

/// The discard port, where Wake-on-LAN senders conventionally aim the packet (the network card
/// reads the payload; the port does not matter to it).
pub const WAKE_PORT: u16 = 9;

/// How many times the packet is sent, and how far apart: a broadcast is not acknowledged and a
/// busy Wi-Fi drops one now and then.
pub const WAKE_REPEATS: usize = 3;
pub const WAKE_GAP: Duration = Duration::from_millis(100);

/// The probe's default bound. A Sleep Proxy answers (or wakes the host) on the first SYN; waiting
/// longer only delays the connection that follows.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);

/// The length of a magic packet: six `0xFF` bytes, then the MAC address sixteen times.
pub const MAGIC_PACKET_LEN: usize = 6 + 16 * 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a MAC address is six two-digit hex groups separated by ':' or '-'")]
pub struct MacError;

/// Parses `aa:bb:cc:dd:ee:ff` or `aa-bb-cc-dd-ee-ff`, in either case. Surrounding whitespace is
/// ignored; mixed separators, other lengths and anything else are refused.
pub fn parse_mac(text: &str) -> Result<[u8; 6], MacError> {
    let text = text.trim();
    let separator = if text.contains('-') { '-' } else { ':' };
    let groups: Vec<&str> = text.split(separator).collect();
    if groups.len() != 6 {
        return Err(MacError);
    }
    let mut mac = [0u8; 6];
    for (byte, group) in mac.iter_mut().zip(groups) {
        if group.len() != 2 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(MacError);
        }
        *byte = u8::from_str_radix(group, 16).map_err(|_| MacError)?;
    }
    Ok(mac)
}

/// The standard magic packet for `mac`.
pub fn magic_packet(mac: [u8; 6]) -> [u8; MAGIC_PACKET_LEN] {
    let mut packet = [0xFF; MAGIC_PACKET_LEN];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        *chunk = mac;
    }
    packet
}

/// Where the packet goes: the limited broadcast first (it needs no knowledge of the network), then
/// each of `broadcasts` (the current network's subnet-directed broadcast addresses, which a router
/// or a phone's own stack may handle better), each once, all on [`WAKE_PORT`].
pub fn wake_targets(broadcasts: &[Ipv4Addr]) -> Vec<SocketAddrV4> {
    let mut targets = vec![SocketAddrV4::new(Ipv4Addr::BROADCAST, WAKE_PORT)];
    for address in broadcasts {
        let target = SocketAddrV4::new(*address, WAKE_PORT);
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// Sends `mac`'s magic packet to every [`wake_targets`] address [`WAKE_REPEATS`] times,
/// [`WAKE_GAP`] apart, through `broadcast`. An error means no copy left the phone (no network at
/// all); a sleeping host that did not wake is not an error, since nothing answers a magic packet.
pub async fn wake_on_lan<B: DatagramBroadcast>(
    broadcast: &B,
    mac: [u8; 6],
    broadcasts: &[Ipv4Addr],
) -> std::io::Result<()> {
    let packet = magic_packet(mac);
    let targets = wake_targets(broadcasts);
    let mut result = Ok(());
    let mut sent = false;
    for round in 0..WAKE_REPEATS {
        if round > 0 {
            tokio::time::sleep(WAKE_GAP).await;
        }
        match broadcast.send_all(&packet, &targets).await {
            Ok(()) => sent = true,
            Err(error) => {
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
    }
    if sent { Ok(()) } else { result }
}

/// One TCP connection attempt to each of `addresses` through `transport`, all at once, bounded by
/// `timeout` as a whole ([`PROBE_TIMEOUT`] by default). Every connection made is dropped at once
/// (the knock is the point, not the session), and every attempt still running at the bound is
/// cancelled. It reports nothing: whether the host woke shows in the connection that follows.
pub async fn wake_probe<T: Transport>(
    transport: &Arc<T>,
    addresses: &[Endpoint],
    timeout: Duration,
) {
    // Dropping the set (at the bound) aborts every attempt still running.
    let mut attempts = JoinSet::new();
    for endpoint in addresses {
        let transport = Arc::clone(transport);
        let endpoint = endpoint.clone();
        attempts.spawn(async move {
            // The stream, if any, is dropped here: the probe never keeps a connection.
            let _ = transport.connect(&endpoint).await;
        });
    }
    let _ = tokio::time::timeout(timeout, async {
        while attempts.join_next().await.is_some() {}
    })
    .await;
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io;
    use std::sync::Mutex;

    use tokio::io::DuplexStream;
    use tokio::net::TcpListener;
    use tokio::time::Instant;

    use super::*;
    use crate::transport::DirectTcp;

    const MAC: [u8; 6] = [0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03];

    #[test]
    fn the_magic_packet_is_six_ff_then_the_mac_sixteen_times() {
        let packet = magic_packet(MAC);
        assert_eq!(packet.len(), 102);
        assert_eq!(packet[..6], [0xFF; 6]);
        for copy in packet[6..].chunks(6) {
            assert_eq!(copy, MAC);
        }
        assert_eq!(packet[6..].chunks(6).count(), 16);
    }

    #[test]
    fn macs_parse_with_colons_or_dashes_in_either_case() {
        for text in [
            "aa:bb:cc:01:02:03",
            "AA:BB:CC:01:02:03",
            "aa-bb-cc-01-02-03",
            "Aa-bB-cC-01-02-03",
            " aa:bb:cc:01:02:03\n",
        ] {
            assert_eq!(parse_mac(text), Ok(MAC), "{text:?}");
        }
    }

    #[test]
    fn malformed_macs_are_refused() {
        for text in [
            "",
            "aa:bb:cc:01:02",
            "aa:bb:cc:01:02:03:04",
            "aa:bb-cc:01:02:03",
            "aabbcc010203",
            "aa:bb:cc:01:02:0g",
            "a:bb:cc:01:02:033",
            "aa:bb:cc:01:02:+3",
            "aa::bb:cc:01:02",
            "aa.bb.cc.01.02.03",
            "ａa:bb:cc:01:02:03",
        ] {
            assert_eq!(parse_mac(text), Err(MacError), "{text:?}");
        }
    }

    #[test]
    fn the_limited_broadcast_comes_first_and_no_target_twice() {
        let subnet = Ipv4Addr::new(192, 168, 1, 255);
        assert_eq!(
            wake_targets(&[subnet, Ipv4Addr::BROADCAST, subnet]),
            vec![
                SocketAddrV4::new(Ipv4Addr::BROADCAST, 9),
                SocketAddrV4::new(subnet, 9),
            ]
        );
        assert_eq!(
            wake_targets(&[]),
            vec![SocketAddrV4::new(Ipv4Addr::BROADCAST, 9)]
        );
    }

    /// One recorded `send_all`: when, what and where.
    type Sent = (Duration, Vec<u8>, Vec<SocketAddrV4>);

    /// Records every `send_all` with its time, instead of sending anything.
    struct Recorder {
        origin: Instant,
        fail: bool,
        sent: Mutex<Vec<Sent>>,
    }

    impl Recorder {
        fn new(fail: bool) -> Self {
            Self {
                origin: Instant::now(),
                fail,
                sent: Mutex::default(),
            }
        }
    }

    impl DatagramBroadcast for Recorder {
        async fn send_all(&self, datagram: &[u8], targets: &[SocketAddrV4]) -> io::Result<()> {
            self.sent.lock().unwrap().push((
                self.origin.elapsed(),
                datagram.to_vec(),
                targets.to_vec(),
            ));
            if self.fail {
                Err(io::Error::from(io::ErrorKind::NetworkUnreachable))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wake_on_lan_sends_the_packet_three_times_100_ms_apart_to_every_target() {
        let recorder = Recorder::new(false);
        let subnet = Ipv4Addr::new(10, 0, 0, 255);
        wake_on_lan(&recorder, MAC, &[subnet]).await.unwrap();
        let sent = recorder.sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .map(|(at, _, _)| at.as_millis())
                .collect::<Vec<_>>(),
            vec![0, 100, 200]
        );
        for (_, packet, targets) in sent.iter() {
            assert_eq!(packet[..], magic_packet(MAC)[..]);
            assert_eq!(
                targets,
                &vec![
                    SocketAddrV4::new(Ipv4Addr::BROADCAST, 9),
                    SocketAddrV4::new(subnet, 9)
                ]
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn wake_on_lan_fails_only_when_no_copy_was_sent() {
        let recorder = Recorder::new(true);
        let error = wake_on_lan(&recorder, MAC, &[]).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NetworkUnreachable);
        assert_eq!(recorder.sent.lock().unwrap().len(), 3); // Every round was still tried.
    }

    /// A transport that never answers: each attempt hangs until it is cancelled.
    struct Silent {
        origin: Instant,
        started: Mutex<Vec<(u16, Duration)>>,
    }

    impl Transport for Silent {
        type Stream = DuplexStream;

        fn connect(
            &self,
            endpoint: &Endpoint,
        ) -> impl Future<Output = io::Result<DuplexStream>> + Send {
            self.started
                .lock()
                .unwrap()
                .push((endpoint.port(), self.origin.elapsed()));
            std::future::pending()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_probe_tries_every_address_at_once_and_stops_at_its_bound() {
        let silent = Arc::new(Silent {
            origin: Instant::now(),
            started: Mutex::default(),
        });
        let addresses: Vec<Endpoint> = (1..=3)
            .map(|port| Endpoint::new("silent.invalid", port).unwrap())
            .collect();
        let start = Instant::now();
        wake_probe(&silent, &addresses, PROBE_TIMEOUT).await;
        // Three addresses that never answer cost one bound, not three.
        assert_eq!(start.elapsed(), PROBE_TIMEOUT);
        let mut started = silent.started.lock().unwrap().clone();
        started.sort();
        assert_eq!(
            started,
            vec![
                (1, Duration::ZERO),
                (2, Duration::ZERO),
                (3, Duration::ZERO)
            ]
        );
    }

    #[tokio::test]
    async fn the_probe_knocks_on_an_open_port_drops_the_connection_and_ends_early() {
        let open = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open_port = open.local_addr().unwrap().port();
        let closed_port = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap().port()
        };
        let addresses = [
            Endpoint::new("127.0.0.1", closed_port).unwrap(),
            Endpoint::new("127.0.0.1", open_port).unwrap(),
        ];
        let start = std::time::Instant::now();
        wake_probe(&Arc::new(DirectTcp), &addresses, Duration::from_secs(10)).await;
        // A refused port and an answered one both end at once: no wait for the bound.
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );

        let (mut accepted, _) = tokio::time::timeout(Duration::from_secs(5), open.accept())
            .await
            .unwrap()
            .unwrap();
        // The probe already dropped its end: the host sees the connection close.
        let mut buf = [0u8; 1];
        let read = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::io::AsyncReadExt::read(&mut accepted, &mut buf),
        )
        .await
        .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    }

    #[tokio::test]
    async fn a_probe_of_no_addresses_returns_at_once() {
        let start = std::time::Instant::now();
        wake_probe(&Arc::new(DirectTcp), &[], PROBE_TIMEOUT).await;
        assert!(start.elapsed() < PROBE_TIMEOUT);
    }
}
