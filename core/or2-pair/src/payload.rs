//! The pairing code: the URI the QR carries and the same text for pasting.
//!
//! `or2-pair:2?name=<label>&user=<u>&port=<p>&a=<addr>…&hk=<algo> <base64>&id=<pairing id>`
//!
//! Values are percent-encoded; `:` and the RFC 3986 unreserved characters stay as they are.
//! Nothing in it is secret (the secret is the code `K` the phone shows). `--manual` omits `id`.
//! The strict parser is the phone's, in `or2_core::pair`; [`Payload::validate`] applies the same
//! rules here so a code the phone would refuse is never drawn.

use crate::bootstrap::PairingId;
use crate::keyline::KeyLine;

/// At most this many bytes (the QR stays small enough to scan from a terminal).
pub const MAX_BYTES: usize = 1024;
/// The phone accepts one to this many `a` addresses...
pub const MAX_ADDRESSES: usize = 8;
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
    /// The pairing id; `None` for `--manual`.
    pub id: Option<PairingId>,
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
        if let Some(id) = &self.id {
            parts.push(format!("id={id}"));
        }
        format!("or2-pair:2?{}", parts.join("&"))
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
    /// never printed: the same rules as `or2_core::pair` (that crate is not a dependency of this
    /// tool; the tests hold the two in step).
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
        if let Some(id) = &self.id
            && PairingId::parse(id.as_str()).is_err()
        {
            return bad("id", "must be 13 lowercase base32 characters");
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

/// A strict reader of the code, as the contract describes the phone's: only for the tests of
/// this crate (and shared with its integration tests), to hold `validate` and the encoder in
/// step with it.
#[doc(hidden)]
pub mod reference {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        UnsupportedVersion(u32),
        Malformed(&'static str),
    }

    fn unescape(value: &str) -> Result<String, Error> {
        let bytes = value.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or(Error::Malformed("a bad escape"))?;
                out.push(hex);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).map_err(|_| Error::Malformed("not UTF-8"))
    }

    /// Parses a version 2 code. `Err(UnsupportedVersion)` for any other version.
    pub fn parse(text: &str) -> Result<Payload, Error> {
        let rest = text
            .strip_prefix("or2-pair:")
            .ok_or(Error::Malformed("not a pairing code"))?;
        let (version, query) = rest.split_once('?').ok_or(Error::Malformed("no fields"))?;
        let version: u32 = version
            .parse()
            .map_err(|_| Error::Malformed("no version"))?;
        if version != 2 {
            return Err(Error::UnsupportedVersion(version));
        }
        if text.len() > MAX_BYTES {
            return Err(Error::Malformed("too long"));
        }
        let (mut name, mut user, mut port, mut hk, mut id) = (None, None, None, None, None);
        let mut addresses = Vec::new();
        let once = |slot: &mut Option<String>, value: String| {
            if slot.replace(value).is_some() {
                Err(Error::Malformed("a repeated field"))
            } else {
                Ok(())
            }
        };
        for part in query.split('&') {
            let (key, value) = part.split_once('=').ok_or(Error::Malformed("no ="))?;
            let value = unescape(value)?;
            match key {
                "name" => once(&mut name, value)?,
                "user" => once(&mut user, value)?,
                "port" => once(&mut port, value)?,
                "hk" => once(&mut hk, value)?,
                "id" => once(&mut id, value)?,
                "a" => addresses.push(value),
                _ => return Err(Error::Malformed("an unknown field")),
            }
        }
        let missing = || Error::Malformed("a missing field");
        let payload = Payload {
            name: name.ok_or_else(missing)?,
            user: user.ok_or_else(missing)?,
            port: port
                .ok_or_else(missing)?
                .parse()
                .map_err(|_| Error::Malformed("a bad port"))?,
            addresses,
            host_key: hk.ok_or_else(missing)?,
            id: id
                .map(|id| PairingId::parse(&id).map_err(|_| Error::Malformed("a bad id")))
                .transpose()?,
        };
        // The same field rules the CLI applies before drawing (kept in one place).
        payload
            .validate()
            .map_err(|_| Error::Malformed("a field out of range"))?;
        Ok(payload)
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
            id: Some(PairingId::parse("abcdefghijklm").unwrap()),
        }
    }

    #[test]
    fn encodes_the_documented_shape() {
        assert_eq!(
            payload().encode(),
            "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=100.101.102.103&a=work-mac.local\
             &hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7\
             &id=abcdefghijklm"
        );
    }

    #[test]
    fn ipv6_addresses_are_not_bracketed_and_values_are_escaped() {
        let mut p = payload();
        p.name = "a&b=c#d é".into();
        p.addresses = vec!["2001:db8::9".into(), "fd00::1".into()];
        let code = p.encode();
        assert!(code.contains("name=a%26b%3Dc%23d%20%C3%A9&"), "{code}");
        assert!(code.contains("&a=2001:db8::9&a=fd00::1&"), "{code}");
        assert!(!code.contains(' '));
        assert_eq!(reference::parse(&code).unwrap(), p);
    }

    #[test]
    fn manual_has_no_id() {
        let mut p = payload();
        p.id = None;
        let code = p.encode();
        assert!(!code.contains("id="), "{code}");
        assert_eq!(reference::parse(&code).unwrap(), p);
    }

    #[test]
    fn a_typical_code_is_far_below_the_limit() {
        assert!(payload().encode().len() < 400);
    }

    #[test]
    fn the_code_round_trips_through_the_strict_reader() {
        let p = payload();
        assert_eq!(p.validate(), Ok(()));
        assert_eq!(reference::parse(&p.encode()).unwrap(), p);
    }

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
        assert_eq!(reference::parse(&p.encode()).unwrap().addresses.len(), 8);
    }

    #[test]
    fn validate_refuses_what_the_strict_reader_refuses() {
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
                "brackets around an ipv6 address",
                Box::new(|p| p.addresses = vec!["[2001:db8::9]".into()]),
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
        ];
        for (what, change) in cases {
            let mut p = payload();
            change(&mut p);
            assert!(p.validate().is_err(), "{what} was let through");
            // Whatever `validate` refuses, the strict reader refuses (the converse is not required).
            assert!(
                reference::parse(&p.encode()).is_err(),
                "{what}: the reader takes it"
            );
        }
        assert_eq!(payload().validate(), Ok(()));
    }

    #[test]
    fn the_strict_reader_is_strict() {
        let good = payload().encode();
        assert!(reference::parse(&good).is_ok());
        assert_eq!(
            reference::parse(&good.replacen("or2-pair:2", "or2-pair:1", 1)),
            Err(reference::Error::UnsupportedVersion(1))
        );
        assert_eq!(
            reference::parse(&good.replacen("or2-pair:2", "or2-pair:3", 1)),
            Err(reference::Error::UnsupportedVersion(3))
        );
        for bad in [
            format!("{good}&extra=1"),
            format!("{good}&port=23"),
            format!("{good}&id=abcdefghijklm"),
            good.replace("id=abcdefghijklm", "id=ABCDEFGHIJKLM"),
            good.replace("id=abcdefghijklm", "id=abcdefghijkl"),
            good.replace("name=Work%20Mac", "name=Work%2"),
            good.replace("name=Work%20Mac", "name=Work%ZZ"),
            good.replace("&user=alice", ""),
        ] {
            assert!(reference::parse(&bad).is_err(), "{bad}");
        }
        // A plus is a plus, not a space.
        let plus = good.replace("Work%20Mac", "Work+Mac");
        assert_eq!(reference::parse(&plus).unwrap().name, "Work+Mac");
    }

    #[test]
    fn too_long_a_code_loses_its_last_addresses_first_but_keeps_one() {
        let mut p = payload();
        p.addresses = (1..=8).map(|i| format!("10.0.0.{i}")).collect();
        p.name = "n".repeat(64);
        assert!(p.fit().unwrap().is_empty());
        // A huge host key (RSA 8192) leaves room for few addresses.
        p.host_key = format!("ssh-rsa {}", "A".repeat(830));
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
