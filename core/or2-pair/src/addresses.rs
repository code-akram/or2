//! The addresses a phone can reach this host on, in the order it should try them.
//!
//! Every non-virtual unicast address is listed, global IPv6 included (link-local is not), plus
//! the mDNS name. Order (what the phone tries first): an address the user asked for with
//! `--address`; overlay networks (ZeroTier `zt*` and macOS `feth*`, Tailscale `tailscale*`,
//! anything in 100.64.0.0/10 such as a `utun*`), which work on every network the phone is on;
//! then LAN addresses (private IPv4, unique-local IPv6); then public IPv4; then public IPv6; the
//! mDNS name `<hostname>.local` last, because it only resolves on the same link.
//!
//! Nothing is bound or dialled here, so a misclassified interface only changes the order: the
//! phone connects to the SSH port like any SSH client does.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// An interface address as the operating system lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub ip: IpAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// An address the user named with `--address`.
    Requested,
    Overlay,
    Lan,
    Public,
    PublicV6,
    Mdns,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Overlay => "overlay",
            Self::Public | Self::PublicV6 => "public",
            Self::Lan => "LAN",
            Self::Mdns => "mDNS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// What goes into the pairing code: an IP literal or a name.
    pub text: String,
    pub kind: Kind,
    /// The interface it lives on, when it is an interface address (also when the user named it
    /// with `--address`).
    pub interface: Option<String>,
}

/// Interface names of things that are not a network the phone can be on: container bridges, VM
/// host-only networks and the like. Their addresses are private but lead nowhere useful.
const VIRTUAL_PREFIXES: [&str; 14] = [
    "docker",
    "br-",
    "veth",
    "virbr",
    "vmnet",
    "vboxnet",
    "lxc",
    "lxd",
    "cni",
    "flannel",
    "cali",
    "kube",
    "bridge",
    "vEthernet",
];

fn overlay_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("zt")
        || lower.starts_with("tailscale")
        || lower.starts_with("zerotier")
        // ZeroTier's fake ethernet interfaces on macOS.
        || lower.starts_with("feth")
}

fn virtual_name(name: &str) -> bool {
    name == "lo"
        || VIRTUAL_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// 100.64.0.0/10.
pub fn is_cgnat(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 0x40
}

/// fe80::/10.
fn is_link_local_v6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xFFC0) == 0xFE80
}

/// fc00::/7.
fn is_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xFE00) == 0xFC00
}

/// What to do with one interface address; `None` skips it (loopback, link-local, wildcard,
/// multicast, virtual interfaces).
pub fn classify(iface: &Iface) -> Option<Kind> {
    if virtual_name(&iface.name) {
        return None;
    }
    match iface.ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
            {
                return None;
            }
            if overlay_name(&iface.name) || is_cgnat(v4) {
                Some(Kind::Overlay)
            } else if v4.is_private() {
                Some(Kind::Lan)
            } else {
                Some(Kind::Public)
            }
        }
        IpAddr::V6(v6) => {
            // Deprecated site-local (fec0::/10) and IPv4-mapped addresses are not addresses a
            // phone would dial.
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || is_link_local_v6(v6)
                || (v6.segments()[0] & 0xFFC0) == 0xFEC0
                || v6.to_ipv4_mapped().is_some()
            {
                return None;
            }
            if overlay_name(&iface.name) {
                Some(Kind::Overlay)
            } else if is_unique_local(v6) {
                Some(Kind::Lan)
            } else {
                Some(Kind::PublicV6)
            }
        }
    }
}

/// `<hostname>.local`: the first label of the host name, as the mDNS responder publishes it.
pub fn mdns_name(hostname: &str) -> Option<String> {
    let first = hostname.split('.').next().unwrap_or_default();
    let label: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let label = label.trim_matches('-');
    (!label.is_empty()).then(|| format!("{label}.local"))
}

/// Every address worth listing, in the order the phone should try them.
pub fn gather(interfaces: &[Iface], hostname: Option<&str>, requested: &[String]) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    let mut push = |address: Address| {
        if !out.iter().any(|known| known.text == address.text) {
            out.push(address);
        }
    };
    for text in requested {
        // An address the user names that is one of this host's own keeps what the interface
        // says it is (its name); only its place in the list is the user's choice.
        let own = text
            .parse::<IpAddr>()
            .ok()
            .and_then(|ip| interfaces.iter().find(|iface| iface.ip == ip))
            .filter(|iface| classify(iface).is_some());
        push(Address {
            text: text.clone(),
            kind: Kind::Requested,
            interface: own.map(|iface| iface.name.clone()),
        });
    }
    for iface in interfaces {
        if let Some(kind) = classify(iface) {
            push(Address {
                text: iface.ip.to_string(),
                kind,
                interface: Some(iface.name.clone()),
            });
        }
    }
    if let Some(name) = hostname.and_then(mdns_name) {
        push(Address {
            text: name,
            kind: Kind::Mdns,
            interface: None,
        });
    }
    // Stable: addresses of one kind keep the order the system listed them in.
    out.sort_by_key(|address| address.kind);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str, ip: &str) -> Iface {
        Iface {
            name: name.into(),
            ip: ip.parse().unwrap(),
        }
    }

    #[test]
    fn classifies_by_interface_name_and_range() {
        for (name, ip, kind) in [
            ("eth0", "192.168.1.20", Some(Kind::Lan)),
            ("en0", "10.1.2.3", Some(Kind::Lan)),
            ("wlan0", "172.16.5.5", Some(Kind::Lan)),
            ("wlan0", "172.31.255.1", Some(Kind::Lan)),
            ("wlan0", "172.32.0.1", Some(Kind::Public)),
            ("zt7nnig26", "10.147.17.5", Some(Kind::Overlay)),
            ("ztabcdef", "172.27.1.1", Some(Kind::Overlay)),
            ("feth1234", "192.168.192.5", Some(Kind::Overlay)),
            ("tailscale0", "100.101.102.103", Some(Kind::Overlay)),
            ("utun4", "100.88.1.2", Some(Kind::Overlay)),
            ("eth0", "100.64.0.1", Some(Kind::Overlay)),
            ("eth0", "100.127.255.254", Some(Kind::Overlay)),
            ("eth0", "100.128.0.1", Some(Kind::Public)),
            ("eth0", "100.63.255.255", Some(Kind::Public)),
            ("eth0", "203.0.113.9", Some(Kind::Public)),
            ("lo", "127.0.0.1", None),
            ("eth0", "169.254.7.7", None),
            ("eth0", "0.0.0.0", None),
            ("docker0", "172.17.0.1", None),
            ("br-1a2b3c", "172.18.0.1", None),
            ("veth123", "10.0.0.2", None),
            ("virbr0", "192.168.122.1", None),
            ("bridge100", "192.168.64.1", None),
            // IPv6: global addresses are listed; link-local, loopback and the like are not.
            ("eth0", "2001:db8::9", Some(Kind::PublicV6)),
            ("eth0", "2606:4700::1", Some(Kind::PublicV6)),
            ("eth0", "fd00::1", Some(Kind::Lan)),
            ("eth0", "fc00::1", Some(Kind::Lan)),
            ("zt0", "fd12:3456::1", Some(Kind::Overlay)),
            ("zt0", "2001:db8::5", Some(Kind::Overlay)),
            ("eth0", "fe80::1", None),
            ("eth0", "::1", None),
            ("eth0", "::", None),
            ("eth0", "ff02::1", None),
            ("eth0", "fec0::1", None),
            ("eth0", "::ffff:192.0.2.1", None),
            ("docker0", "2001:db8::2", None),
            ("lo", "::1", None),
        ] {
            assert_eq!(classify(&iface(name, ip)), kind, "{name} {ip}");
        }
    }

    #[test]
    fn orders_overlay_then_lan_then_public_then_public_v6_then_mdns() {
        let interfaces = [
            iface("lo", "127.0.0.1"),
            iface("eth0", "2001:db8::9"),
            iface("eth0", "192.168.1.20"),
            iface("docker0", "172.17.0.1"),
            iface("zt0", "10.147.17.5"),
            iface("eth1", "203.0.113.9"),
            iface("tailscale0", "100.101.102.103"),
            iface("wlan0", "10.0.0.7"),
            iface("wlan0", "fe80::1"),
        ];
        let addresses = gather(&interfaces, Some("work-mac.example.net"), &[]);
        let order: Vec<_> = addresses.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(
            order,
            [
                "10.147.17.5",
                "100.101.102.103",
                "192.168.1.20",
                "10.0.0.7",
                "203.0.113.9",
                "2001:db8::9",
                "work-mac.local"
            ]
        );
        assert_eq!(addresses[0].interface.as_deref(), Some("zt0"));
        assert_eq!(addresses.last().unwrap().kind, Kind::Mdns);
    }

    #[test]
    fn requested_addresses_go_first_and_duplicates_are_dropped() {
        let interfaces = [iface("eth0", "192.168.1.20"), iface("en1", "192.168.1.20")];
        let addresses = gather(
            &interfaces,
            Some("box"),
            &["dev.example.org".into(), "192.168.1.20".into()],
        );
        let order: Vec<_> = addresses.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(order, ["dev.example.org", "192.168.1.20", "box.local"]);
        assert_eq!(addresses[1].kind, Kind::Requested);
        assert_eq!(addresses[1].interface.as_deref(), Some("eth0"));
        // Naming an address that is not a listed interface address keeps no interface.
        let named = gather(&interfaces, None, &["172.17.0.1".into(), "10.9.9.9".into()]);
        assert!(named.iter().take(2).all(|a| a.interface.is_none()));
    }

    #[test]
    fn mdns_name_is_the_first_label() {
        assert_eq!(
            mdns_name("Work-Mac.local").as_deref(),
            Some("Work-Mac.local")
        );
        assert_eq!(mdns_name("box.example.org").as_deref(), Some("box.local"));
        assert_eq!(mdns_name("My Mac's").as_deref(), Some("MyMacs.local"));
        assert_eq!(mdns_name(""), None);
        assert_eq!(mdns_name("--"), None);
        assert_eq!(mdns_name(".local"), None);
    }

    #[test]
    fn nothing_is_listed_without_a_usable_interface_or_name() {
        let interfaces = [
            iface("lo", "127.0.0.1"),
            iface("eth0", "169.254.1.1"),
            iface("eth0", "fe80::1"),
        ];
        assert!(gather(&interfaces, None, &[]).is_empty());
    }
}
