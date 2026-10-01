//! The pairing code: the URI the QR carries and the same text for pasting.
//!
//! `or2-pair:1?name=<label>&user=<u>&port=<p>&a=<addr>…&hk=<algo> <base64>&pair=<ip>:<port>…&otp=<base32>`
//!
//! Values are percent-encoded; `:` and the RFC 3986 unreserved characters stay as they are.
//! The strict parser is `or2_core::pair::PairOffer::parse`; the integration tests round-trip
//! every code this module makes through it.

use std::net::SocketAddr;

use data_encoding::BASE32_NOPAD;

/// At most this many bytes (the QR stays small enough to scan from a terminal).
pub const MAX_BYTES: usize = 1024;

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

    /// Drops the lowest-priority addresses (the last ones) until the code fits in
    /// [`MAX_BYTES`], keeping at least one. Returns the ones it dropped.
    pub fn fit(&mut self) -> Result<Vec<String>, TooLong> {
        let mut dropped = Vec::new();
        while self.encode().len() > MAX_BYTES {
            if self.addresses.len() <= 1 {
                return Err(TooLong(self.encode().len()));
            }
            dropped.insert(0, self.addresses.pop().unwrap_or_default());
        }
        Ok(dropped)
    }
}

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
