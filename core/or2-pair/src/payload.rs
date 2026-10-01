//! The pairing code: the URI the QR carries and the same text for pasting.
//!
//! `or2-pair:1?name=<label>&user=<u>&port=<p>&a=<addr>…&hk=<algo> <base64>&pair=<ip>:<port>…&otp=<base32>`
//!
//! Values are percent-encoded; `:` and the RFC 3986 unreserved characters stay as they are.
//! The strict parser is `or2_core::pair::PairOffer::parse`; the integration tests round-trip
//! every code this module makes through it.

use std::net::SocketAddr;

use data_encoding::BASE32_NOPAD;

use crate::keyline::KeyLine;

/// At most this many bytes (the QR stays small enough to scan from a terminal).
pub const MAX_BYTES: usize = 1024;
/// The phone accepts one to this many `a` addresses...
pub const MAX_ADDRESSES: usize = 8;
/// ...and this many `pair` addresses...
pub const MAX_PAIR_ADDRESSES: usize = 4;
/// ...and a name or user of at most this many characters.
pub const MAX_LABEL_CHARS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub name: String,
    pub user: String,
    pub port: u16,
    /// Where SSH reaches the host, in preference order.
    pub addresses: Vec<String>,
    /// `<algorithm> <base64>`.
    pub host_key: String,
    /// Where the listener is; empty for `--no-listen`.
    pub pair: Vec<SocketAddr>,
    /// The one-time password; `None` for `--no-listen`.
    pub otp: Option<[u8; 16]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the pairing code is {0} bytes even with one address; the limit is {MAX_BYTES}")]
pub struct TooLong(pub usize);

/// `A-Za-z0-9-._~` and `:` as they are, everything else `%XX`.
fn encode_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The OTP in the payload's form: 26 base32 characters.
pub fn otp_text(otp: &[u8; 16]) -> String {
    BASE32_NOPAD.encode(otp)
}

impl Payload {
    pub fn encode(&self) -> String {
        let mut parts = vec![
            format!("name={}", encode_value(&self.name)),
            format!("user={}", encode_value(&self.user)),
            format!("port={}", self.port),
        ];
        parts.extend(
            self.addresses
                .iter()
                .map(|address| format!("a={}", encode_value(address))),
        );
        parts.push(format!("hk={}", encode_value(&self.host_key)));
        parts.extend(
            self.pair
                .iter()
                .map(|pair| format!("pair={}", encode_value(&pair.to_string()))),
        );
        if let Some(otp) = &self.otp {
            parts.push(format!("otp={}", otp_text(otp)));
        }
        format!("or2-pair:1?{}", parts.join("&"))
    }

    /// Drops the lowest-priority addresses (the last ones) until the code is within both of the
    /// phone's limits, [`MAX_ADDRESSES`] addresses and [`MAX_BYTES`] bytes, keeping at least one.
    /// Returns the ones it dropped.
    pub fn fit(&mut self) -> Result<Vec<String>, TooLong> {
        let mut dropped = Vec::new();
        while self.addresses.len() > MAX_ADDRESSES || self.encode().len() > MAX_BYTES {
            if self.addresses.len() <= 1 {
                return Err(TooLong(self.encode().len()));
            }
            dropped.insert(0, self.addresses.pop().unwrap_or_default());
        }
        Ok(dropped)
    }

    /// Checks everything the phone's strict parser would, so a code that would be refused is
    /// never printed: the same rules as `or2_core::pair::PairOffer::parse` (that crate is not a
    /// dependency of this tool; the tests hold the two in step against the real parser).
    pub fn validate(&self) -> Result<(), Invalid> {
        let bad = |field: &'static str, why: &str| Err(Invalid(field, why.to_owned()));
        for (field, text) in [("name", &self.name), ("user", &self.user)] {
            let text = text.trim();
            if text.is_empty() || text.chars().count() > MAX_LABEL_CHARS {
                return bad(field, "must be 1 to 64 characters");
            }
            if text.chars().any(char::is_control) {
                return bad(field, "must not contain control characters");
            }
        }
        if self.port == 0 {
            return bad("port", "must be 1 to 65535");
        }
        if self.addresses.is_empty() || self.addresses.len() > MAX_ADDRESSES {
            return bad("a", "needs one to eight addresses");
        }
        for (index, address) in self.addresses.iter().enumerate() {
            let plain = address
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
            if address.is_empty() || address.len() > 255 || !plain {
                return bad(
                    "a",
                    &format!("{address:?} is not a host name or IP address the phone accepts"),
                );
            }
            if self.addresses[..index].contains(address) {
                return bad("a", &format!("{address} is listed twice"));
            }
        }
        match KeyLine::parse(&self.host_key) {
            Ok(key) if key.openssh() == self.host_key => {}
            _ => return bad("hk", "must be one plain public key without a comment"),
        }
        if self.pair.is_empty() != self.otp.is_none() {
            return bad(
                "pair",
                "the listener address and the password come together",
            );
        }
        if self.pair.len() > MAX_PAIR_ADDRESSES {
            return bad("pair", "at most four listener addresses");
        }
        for (index, pair) in self.pair.iter().enumerate() {
            let ip = pair.ip();
            let unusable = ip.is_unspecified()
                || ip.is_multicast()
                || matches!(ip, std::net::IpAddr::V4(v4) if v4.is_broadcast());
            if unusable || pair.port() == 0 {
                return bad(
                    "pair",
                    &format!("{pair} is not an address a phone can dial"),
                );
            }
            if self.pair[..index].contains(pair) {
                return bad("pair", &format!("{pair} is listed twice"));
            }
        }
        let size = self.encode().len();
        if size > MAX_BYTES {
            return bad("size", &format!("{size} bytes, the limit is {MAX_BYTES}"));
        }
        Ok(())
    }
}

/// A field the phone would refuse, and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the pairing code would not be accepted by the phone: `{0}` {1}")]
pub struct Invalid(pub &'static str, pub String);

#[cfg(test)]
mod tests {
    use super::*;

    const HK: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";

    fn payload() -> Payload {
        Payload {
            name: "Work Mac".into(),
            user: "alice".into(),
            port: 22,
            addresses: vec!["100.101.102.103".into(), "work-mac.local".into()],
            host_key: HK.into(),
            pair: vec!["192.168.1.20:41234".parse().unwrap()],
            otp: Some(core::array::from_fn(|i| i as u8)),
        }
    }

    #[test]
    fn encodes_the_documented_shape() {
        assert_eq!(
            payload().encode(),
            "or2-pair:1?name=Work%20Mac&user=alice&port=22&a=100.101.102.103&a=work-mac.local\
             &hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7\
             &pair=192.168.1.20:41234&otp=AAAQEAYEAUDAOCAJBIFQYDIOB4"
        );
    }

    #[test]
    fn ipv6_pair_addresses_are_bracketed_and_values_are_escaped() {
        let mut p = payload();
        p.name = "a&b=c#d é".into();
        p.pair = vec!["[fd00::1]:5000".parse().unwrap()];
        let code = p.encode();
        assert!(code.contains("name=a%26b%3Dc%23d%20%C3%A9&"), "{code}");
        assert!(code.contains("&pair=%5Bfd00::1%5D:5000&"), "{code}");
        assert!(!code.contains(' '));
    }

    #[test]
    fn no_listen_has_neither_pair_nor_otp() {
        let mut p = payload();
        p.pair.clear();
        p.otp = None;
        let code = p.encode();
        assert!(!code.contains("pair=") && !code.contains("otp="), "{code}");
    }

    #[test]
    fn a_typical_code_is_far_below_the_limit() {
        assert!(payload().encode().len() < 400);
    }

    // --- Finding 9: never a code the phone rejects ---------------------------------------------

    #[test]
    fn more_than_eight_short_addresses_are_trimmed_to_the_phones_limit() {
        let mut p = payload();
        p.addresses = (1..=12).map(|i| format!("10.0.0.{i}")).collect();
        assert!(
            p.encode().len() < MAX_BYTES,
            "short enough for the byte limit"
        );
        let dropped = p.fit().unwrap();
        assert_eq!(p.addresses.len(), MAX_ADDRESSES);
        assert_eq!(dropped, ["10.0.0.9", "10.0.0.10", "10.0.0.11", "10.0.0.12"]);
        assert_eq!(p.validate(), Ok(()));
        // And the real, strict parser agrees.
        let offer = or2_core::pair::PairOffer::parse(&p.encode()).unwrap();
        assert_eq!(offer.addresses.len(), 8);
    }

    #[test]
    fn validate_refuses_what_the_phones_parser_refuses() {
        let long = "x".repeat(65);
        type Change = Box<dyn Fn(&mut Payload)>;
        let cases: Vec<(&str, Change)> = vec![
            ("empty name", Box::new(|p| p.name = "  ".into())),
            (
                "long name",
                Box::new({
                    let long = long.clone();
                    move |p| p.name = long.clone()
                }),
            ),
            ("control in name", Box::new(|p| p.name = "a\nb".into())),
            ("empty user", Box::new(|p| p.user = String::new())),
            (
                "long user",
                Box::new({
                    let long = long.clone();
                    move |p| p.user = long.clone()
                }),
            ),
            (
                "control in user",
                Box::new(|p| p.user = "al\u{7}ice".into()),
            ),
            ("no port", Box::new(|p| p.port = 0)),
            ("no addresses", Box::new(|p| p.addresses.clear())),
            (
                "nine addresses",
                Box::new(|p| {
                    p.addresses = (1..=9).map(|i| format!("10.0.0.{i}")).collect();
                }),
            ),
            (
                "a space in an address",
                Box::new(|p| p.addresses = vec!["my host".into()]),
            ),
            (
                "a slash in an address",
                Box::new(|p| p.addresses = vec!["a/b".into()]),
            ),
            (
                "a long address",
                Box::new(|p| p.addresses = vec!["a".repeat(256)]),
            ),
            (
                "a duplicate address",
                Box::new(|p| {
                    p.addresses = vec!["10.0.0.1".into(), "10.0.0.1".into()];
                }),
            ),
            (
                "a host key with a comment",
                Box::new(|p| p.host_key.push_str(" root@box")),
            ),
            (
                "an unsupported host key",
                Box::new(|p| p.host_key = "ssh-dss AAAA".into()),
            ),
            (
                "five listener addresses",
                Box::new(|p| {
                    p.pair = (1..=5)
                        .map(|i| format!("10.0.0.{i}:9").parse().unwrap())
                        .collect();
                }),
            ),
            (
                "a wildcard listener",
                Box::new(|p| p.pair = vec!["0.0.0.0:9".parse().unwrap()]),
            ),
            (
                "a multicast listener",
                Box::new(|p| p.pair = vec!["224.0.0.1:9".parse().unwrap()]),
            ),
            (
                "a broadcast listener",
                Box::new(|p| p.pair = vec!["255.255.255.255:9".parse().unwrap()]),
            ),
            (
                "a duplicate listener",
                Box::new(|p| {
                    p.pair = vec!["10.0.0.1:9".parse().unwrap(), "10.0.0.1:9".parse().unwrap()];
                }),
            ),
            ("a listener without a password", Box::new(|p| p.otp = None)),
            (
                "a password without a listener",
                Box::new(|p| p.pair.clear()),
            ),
        ];
        for (what, change) in cases {
            let mut p = payload();
            change(&mut p);
            assert!(p.validate().is_err(), "{what} was let through");
            // Whatever `validate` refuses, the phone refuses (the converse is not required).
            assert!(
                or2_core::pair::PairOffer::parse(&p.encode()).is_err(),
                "{what}: the phone takes it"
            );
        }
        assert_eq!(payload().validate(), Ok(()));
    }

    #[test]
    fn too_long_a_code_loses_its_last_addresses_first_but_keeps_one() {
        let mut p = payload();
        p.addresses = (1..=8).map(|i| format!("10.0.0.{i}")).collect();
        p.name = "n".repeat(64);
        assert!(p.fit().unwrap().is_empty());
        // A huge host key (RSA 8192) leaves room for few addresses.
        p.host_key = format!("ssh-rsa {}", "A".repeat(780));
        let dropped = p.fit().unwrap();
        assert!(!dropped.is_empty());
        assert_eq!(p.addresses.len() + dropped.len(), 8);
        assert_eq!(dropped.last().unwrap(), "10.0.0.8");
        assert!(p.encode().len() <= MAX_BYTES);
        assert_eq!(p.addresses[0], "10.0.0.1");
        p.host_key = format!("ssh-rsa {}", "A".repeat(2000));
        assert!(p.fit().is_err());
        assert_eq!(p.addresses.len(), 1);
    }
}
