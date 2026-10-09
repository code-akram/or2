//! Waking a sleeping host for Kotlin: the Wake-on-LAN magic packet and the TCP wake probe. See
//! `or2_core::wake`. Kotlin supplies what Rust cannot know: the MAC address it stored and the
//! current network's subnet-directed broadcast addresses (from Android's `LinkProperties`).

use std::net::Ipv4Addr;
use std::sync::Arc;

use or2_core::transport::{DirectBroadcast, DirectTcp, Endpoint, describe_error};
use or2_core::wake as core;

use crate::host::HostAddress;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WakeError {
    #[error("a MAC address is six two-digit hex groups separated by ':' or '-'")]
    InvalidMac,
    #[error("a broadcast address is not an IPv4 address")]
    InvalidAddress,
    /// No copy of the packet left the phone (no network). Nothing ever reports a host that did not
    /// wake: only the connection that follows can tell.
    #[error("the wake packet could not be sent: {reason}")]
    Network { reason: String },
}

/// Sends the magic packet for `mac` (`aa:bb:cc:dd:ee:ff` or with `-`, either case) to UDP port 9
/// of the limited broadcast and of each of `broadcasts` (dotted IPv4), three times 100 ms apart.
/// Every input is validated before anything is sent.
#[uniffi::export(async_runtime = "tokio")]
pub async fn wake_on_lan(mac: String, broadcasts: Vec<String>) -> Result<(), WakeError> {
    let mac = core::parse_mac(&mac).map_err(|_| WakeError::InvalidMac)?;
    let broadcasts = broadcasts
        .iter()
        .map(|address| address.trim().parse::<Ipv4Addr>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WakeError::InvalidAddress)?;
    core::wake_on_lan(&DirectBroadcast, mac, &broadcasts)
        .await
        .map_err(|error| WakeError::Network {
            reason: describe_error(&error),
        })
}

/// One TCP connection attempt to each of `addresses`, in parallel, bounded by 1.5 s; any
/// connection made is dropped at once. It reports nothing but completion. An address that is not
/// a valid endpoint is skipped. Run it before connecting to a host whose "Wake probe" is on.
#[uniffi::export(async_runtime = "tokio")]
pub async fn wake_probe(addresses: Vec<HostAddress>) {
    let endpoints: Vec<Endpoint> = addresses
        .iter()
        .filter_map(|address| Endpoint::new(&address.host, address.port).ok())
        .collect();
    core::wake_probe(&Arc::new(DirectTcp), &endpoints, core::PROBE_TIMEOUT).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_input_is_refused_before_anything_is_sent() {
        assert!(matches!(
            wake_on_lan("aa:bb:cc:dd:ee".into(), vec![]).await,
            Err(WakeError::InvalidMac)
        ));
        assert!(matches!(
            wake_on_lan(
                "aa:bb:cc:dd:ee:ff".into(),
                vec!["192.168.1.255".into(), "fe80::1".into()]
            )
            .await,
            Err(WakeError::InvalidAddress)
        ));
        assert!(matches!(
            wake_on_lan("aa:bb:cc:dd:ee:ff".into(), vec!["host.invalid".into()]).await,
            Err(WakeError::InvalidAddress)
        ));
    }

    #[tokio::test]
    async fn the_probe_skips_invalid_endpoints_and_returns() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        wake_probe(vec![
            HostAddress {
                host: String::new(),
                port: 22,
            },
            HostAddress {
                host: "127.0.0.1".into(),
                port: 0,
            },
            HostAddress {
                host: "127.0.0.1".into(),
                port,
            },
        ])
        .await;
        // The one valid address was knocked on.
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
    }
}
