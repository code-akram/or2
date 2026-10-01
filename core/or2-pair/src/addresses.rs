//! The addresses a phone can reach this host on, and which of them the pairing listener may
//! bind.
//!
//! Order (what the phone tries first): overlay networks (ZeroTier `zt*`, Tailscale
//! `tailscale*`, anything in 100.64.0.0/10) work on every network the phone is on, so they come
//! first; then public addresses, which also work everywhere; then LAN addresses; the mDNS name
//! `<hostname>.local` last, because it only resolves on the same link. An address the user asked
//! for with `--address` goes before all of them.
//!
//! # Which addresses are "non-public"
//!
//! The pairing listener carries a one-time password exchange and an on-host confirmation. It
//! must never be reachable from the internet, so it binds only to addresses that are not public:
//!
//! - IPv4 private ranges 10/8, 172.16/12, 192.168/16 and carrier-grade NAT 100.64/10
//!   (Tailscale, and the usual ZeroTier-adjacent overlays),
//! - IPv6 unique-local fc00::/7 and link-local fe80::/10,
//! - any address that lives on an interface named like an overlay (`zt*`, `tailscale*`,
//!   `ZeroTier*`): the overlay's own network is private to its members whatever range it uses.
//!
//! Loopback is not public either, but it is not listed or bound unless `--bind` names it. A
//! public address or a wildcard (`0.0.0.0`, `::`) is never bound by default; `--bind` can choose
//! one explicitly and prints a warning.

use std::net::{IpAddr, Ipv4Addr};

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
    Public,
    Lan,
    Mdns,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Overlay => "overlay",
            Self::Public => "public",
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
    /// Whether the pairing listener may bind it by default: an overlay or LAN interface address.
    /// This is about what the address *is*, not about where `--address` put it in the list, so
    /// naming a detected address to put it first does not take its listener away.
    pub bindable: bool,
}

impl Address {
    /// The IP, when this is an interface address (a name has none).
    pub fn ip(&self) -> Option<IpAddr> {
        self.text.parse().ok()
    }
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
    lower.starts_with("zt") || lower.starts_with("tailscale") || lower.starts_with("zerotier")
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

/// See the module docs: private, carrier-grade NAT, unique-local, link-local or loopback.
pub fn is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || is_cgnat(v4) || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xFE00) == 0xFC00 || (first & 0xFFC0) == 0xFE80 || v6.is_loopback()
        }
    }
}

/// What to do with one interface address; `None` skips it (loopback, link-local, wildcard,
/// IPv6, virtual interfaces).
pub fn classify(iface: &Iface) -> Option<Kind> {
    let IpAddr::V4(v4) = iface.ip else {
        // IPv6 needs scopes and has no overlay-free story on a phone's Wi-Fi yet.
        return None;
    };
    if v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || virtual_name(&iface.name)
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
        // says it is (its name and whether the listener may bind it); only its place in the
        // list is the user's choice.
        let own = text
            .parse::<IpAddr>()
            .ok()
            .and_then(|ip| interfaces.iter().find(|iface| iface.ip == ip))
            .and_then(|iface| classify(iface).map(|kind| (iface, kind)));
        push(Address {
            text: text.clone(),
            kind: Kind::Requested,
            interface: own.map(|(iface, _)| iface.name.clone()),
            bindable: own.is_some_and(|(_, kind)| matches!(kind, Kind::Overlay | Kind::Lan)),
        });
    }
    for iface in interfaces {
        if let Some(kind) = classify(iface) {
            push(Address {
                text: iface.ip.to_string(),
                kind,
                interface: Some(iface.name.clone()),
                bindable: matches!(kind, Kind::Overlay | Kind::Lan),
            });
        }
    }
    if let Some(name) = hostname.and_then(mdns_name) {
        push(Address {
            text: name,
            kind: Kind::Mdns,
            interface: None,
            bindable: false,
        });
    }
    // Stable: addresses of one kind keep the order the system listed them in.
    out.sort_by_key(|address| address.kind);
    out
}

/// The addresses the listener binds by default: the overlay and LAN interface addresses, never
/// a public address, a name or a wildcard.
pub fn bindable(addresses: &[Address]) -> Vec<IpAddr> {
    addresses
        .iter()
        .filter(|address| address.bindable)
        .filter_map(Address::ip)
        .collect()
}

/// What an explicit `--bind` address deserves a warning about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindWarning {
    /// `0.0.0.0` or `::`: every interface, public ones included.
    Wildcard,
    /// A public address.
    Public,
}

pub fn bind_warning(ip: IpAddr) -> Option<BindWarning> {
    if ip.is_unspecified() {
        Some(BindWarning::Wildcard)
    } else if !is_non_public(ip) {
        Some(BindWarning::Public)
    } else {
        None
    }
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
            ("eth0", "fd00::1", None),
        ] {
            assert_eq!(classify(&iface(name, ip)), kind, "{name} {ip}");
        }
    }

    #[test]
    fn orders_overlay_then_public_then_lan_then_mdns() {
        let interfaces = [
            iface("lo", "127.0.0.1"),
            iface("eth0", "192.168.1.20"),
            iface("docker0", "172.17.0.1"),
            iface("zt0", "10.147.17.5"),
            iface("eth1", "203.0.113.9"),
            iface("tailscale0", "100.101.102.103"),
            iface("wlan0", "10.0.0.7"),
        ];
        let addresses = gather(&interfaces, Some("work-mac.example.net"), &[]);
        let order: Vec<_> = addresses.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(
            order,
            [
                "10.147.17.5",
                "100.101.102.103",
                "203.0.113.9",
                "192.168.1.20",
                "10.0.0.7",
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
    }

    fn bound(addresses: &[Address]) -> Vec<String> {
        bindable(addresses)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn naming_a_detected_address_moves_it_first_without_losing_its_listener() {
        // Finding 8: `--address 192.168.1.20` on a host whose only usable interface is that one
        // used to leave nothing to bind.
        let lan = [iface("eth0", "192.168.1.20")];
        let addresses = gather(&lan, None, &["192.168.1.20".into()]);
        assert_eq!(bound(&addresses), ["192.168.1.20"]);
        assert_eq!(addresses[0].kind, Kind::Requested, "still listed first");
        assert_eq!(addresses[0].interface.as_deref(), Some("eth0"));

        // Several interfaces: naming the LAN one puts it ahead of the overlay one, and both
        // keep their listener.
        let both = [iface("zt0", "10.147.17.5"), iface("eth0", "192.168.1.20")];
        let addresses = gather(&both, Some("box"), &["192.168.1.20".into()]);
        let order: Vec<_> = addresses.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(order, ["192.168.1.20", "10.147.17.5", "box.local"]);
        assert_eq!(bound(&addresses), ["192.168.1.20", "10.147.17.5"]);

        // The same for an overlay address named ahead of a LAN one.
        let addresses = gather(&both, None, &["10.147.17.5".into()]);
        assert_eq!(bound(&addresses), ["10.147.17.5", "192.168.1.20"]);
    }

    #[test]
    fn naming_what_is_not_bindable_does_not_make_it_bindable() {
        // A public interface address, a container bridge and a name stay unbound by default,
        // however they are named.
        let interfaces = [
            iface("eth1", "203.0.113.9"),
            iface("docker0", "172.17.0.1"),
            iface("lo", "127.0.0.1"),
        ];
        let addresses = gather(
            &interfaces,
            None,
            &[
                "203.0.113.9".into(),
                "172.17.0.1".into(),
                "127.0.0.1".into(),
                "dev.example.org".into(),
                "10.9.9.9".into(),
            ],
        );
        assert!(bound(&addresses).is_empty(), "{:?}", bound(&addresses));
        assert_eq!(addresses.len(), 5, "all five are still advertised");
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
        let interfaces = [iface("lo", "127.0.0.1"), iface("eth0", "169.254.1.1")];
        assert!(gather(&interfaces, None, &[]).is_empty());
    }

    #[test]
    fn the_listener_binds_overlay_and_lan_addresses_only() {
        let interfaces = [
            iface("eth0", "192.168.1.20"),
            iface("eth1", "203.0.113.9"),
            iface("zt0", "10.147.17.5"),
        ];
        let addresses = gather(&interfaces, Some("box"), &["dev.example.org".into()]);
        let bind: Vec<String> = bindable(&addresses)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(bind, ["10.147.17.5", "192.168.1.20"]);
        assert!(bindable(&gather(&[iface("eth1", "203.0.113.9")], None, &[])).is_empty());
    }

    #[test]
    fn non_public_means_private_overlay_unique_local_link_local_or_loopback() {
        for (ip, non_public) in [
            ("10.0.0.1", true),
            ("172.16.0.1", true),
            ("172.31.255.255", true),
            ("172.15.255.255", false),
            ("192.168.255.255", true),
            ("100.64.0.0", true),
            ("100.127.255.255", true),
            ("100.128.0.0", false),
            ("169.254.1.1", true),
            ("127.0.0.1", true),
            ("8.8.8.8", false),
            ("203.0.113.9", false),
            ("fd12::1", true),
            ("fc00::1", true),
            ("fe80::1", true),
            ("::1", true),
            ("2001:db8::1", false),
            ("2606:4700::1", false),
        ] {
            assert_eq!(is_non_public(ip.parse().unwrap()), non_public, "{ip}");
        }
    }

    #[test]
    fn an_explicit_bind_warns_about_wildcards_and_public_addresses() {
        for (ip, warning) in [
            ("0.0.0.0", Some(BindWarning::Wildcard)),
            ("::", Some(BindWarning::Wildcard)),
            ("203.0.113.9", Some(BindWarning::Public)),
            ("192.168.1.2", None),
            ("127.0.0.1", None),
            ("100.101.1.1", None),
        ] {
            assert_eq!(bind_warning(ip.parse().unwrap()), warning, "{ip}");
        }
    }
}
