//! Easy pair, phone side: the strict parser of the pairing code and the client of the one-shot
//! key exchange with `or2-pair` on the host. See `docs/contracts.md`, "Easy pair".
//!
//! The pairing code is a URI the host prints as a QR code (and as text for pasting):
//!
//! ```text
//! or2-pair:1?name=<label>&user=<u>&port=<p>&a=<addr>&a=<addr>…&hk=<algo> <base64>
//!           &pair=<ip>:<port>&otp=<base32 128-bit>
//! ```
//!
//! Values are percent-encoded. `pair` (one to four, in preference order) and `otp` come together
//! or not at all: a code made with `--no-listen` has neither and only describes the host. The
//! parser is strict about every field and returns typed errors; it never guesses.
//!
//! The exchange (newline-delimited JSON over [`Transport`], every line bounded):
//!
//! ```text
//! host  -> {"v":1,"nonce":"<base64 of 32 bytes>"}
//! phone -> {"v":1,"key":"<openssh public key>","device":"<label>","mac":"<base64 HMAC-SHA256(otp, nonce || key)>"}
//! host  -> {"ok":true}  |  {"ok":false,"reason":"<code>"}
//! ```
//!
//! The one-time password never crosses the network: only a MAC over the host's fresh nonce and the
//! key does. It is held in [`Otp`] (zeroized on drop, redacted in `Debug`).

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::{Algorithm, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::transport::{Endpoint, RACE_STAGGER, RaceTiming, Transport, race_with};
use crate::trust::HostKey;

/// The URI scheme and version prefix of a pairing code.
pub const PAYLOAD_PREFIX: &str = "or2-pair:";
/// A pairing code is at most this many bytes (the host trims addresses to fit).
pub const MAX_PAYLOAD_BYTES: usize = 1024;
/// At most as many `a` addresses as a host may have.
pub const MAX_ADDRESSES: usize = 8;
/// At most this many `pair` addresses.
pub const MAX_PAIR_ADDRESSES: usize = 4;
/// The one-time password: 128 bits, 26 base32 characters.
pub const OTP_BYTES: usize = 16;
/// The host's nonce.
pub const NONCE_BYTES: usize = 32;
/// Longest label for a host name, user name or device.
pub const MAX_LABEL_CHARS: usize = 64;
/// The longest line the phone accepts from the host, and the host from the phone.
pub const HELLO_LIMIT: usize = 256;
pub const REPLY_LIMIT: usize = 512;

/// Why a pairing code was refused. The field names are the payload's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairParseError {
    #[error("this is not an or2 pairing code")]
    NotPairingCode,
    #[error("this pairing code is from a newer or2-pair; update the app")]
    UnsupportedVersion,
    #[error("the pairing code is longer than {MAX_PAYLOAD_BYTES} bytes")]
    TooLong,
    #[error("the pairing code is not well formed")]
    Malformed,
    #[error("the pairing code has no `{0}`")]
    MissingField(&'static str),
    #[error("the pairing code has `{0}` twice")]
    DuplicateField(&'static str),
    #[error("the pairing code has a field this app does not know")]
    UnknownField,
    #[error("the pairing code's `{0}` is not valid")]
    InvalidField(&'static str),
}

/// The one-time password of an exchange. Zeroized on drop; `Debug` never shows it.
#[derive(Clone)]
pub struct Otp(Zeroizing<[u8; OTP_BYTES]>);

impl fmt::Debug for Otp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Otp(<redacted>)")
    }
}

impl Otp {
    pub fn from_bytes(bytes: [u8; OTP_BYTES]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The payload's form: 26 uppercase base32 characters, no padding, canonical.
    fn from_base32(text: &str) -> Option<Self> {
        if text.len() != 26 {
            return None;
        }
        let raw = Zeroizing::new(BASE32_NOPAD.decode(text.as_bytes()).ok()?);
        let bytes: [u8; OTP_BYTES] = raw.as_slice().try_into().ok()?;
        Some(Self::from_bytes(bytes))
    }

    /// `HMAC-SHA256(otp, nonce || key)`: what the phone proves it knows without sending the
    /// password. `key` is the exact text sent in the request's `key` field.
    pub fn mac(&self, nonce: &[u8], key: &str) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(self.0.as_slice())
            .expect("HMAC accepts a key of any length");
        mac.update(nonce);
        mac.update(key.as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Overwrites the password now (the owner is done with it) rather than at drop.
    pub fn wipe(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

/// How to reach the host's one-shot listener.
#[derive(Debug, Clone)]
pub struct PairExchange {
    /// Where the host listens, in preference order (IP literals).
    pub endpoints: Vec<Endpoint>,
    pub otp: Otp,
}

/// A parsed pairing code.
#[derive(Debug, Clone)]
pub struct PairOffer {
    /// The host's label, to name it on the phone.
    pub name: String,
    pub username: String,
    /// The SSH port, shared by every address.
    pub port: u16,
    /// Where SSH can reach the host, in preference order, each with [`PairOffer::port`].
    pub addresses: Vec<Endpoint>,
    /// The host's public key, to trust before the first connection.
    pub host_key: HostKey,
    /// `None` for a code made with `--no-listen`: the user installs the phone's key by hand.
    pub exchange: Option<PairExchange>,
}

impl PairOffer {
    /// Parses and validates a pairing code (leading and trailing whitespace is ignored, as a
    /// paste may carry a newline).
    pub fn parse(text: &str) -> Result<Self, PairParseError> {
        let text = text.trim();
        if text.len() > MAX_PAYLOAD_BYTES {
            return Err(PairParseError::TooLong);
        }
        let rest = text
            .strip_prefix(PAYLOAD_PREFIX)
            .ok_or(PairParseError::NotPairingCode)?;
        let (version, query) = rest.split_once('?').ok_or(PairParseError::Malformed)?;
        match version {
            "1" => {}
            v if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => {
                return Err(PairParseError::UnsupportedVersion);
            }
            _ => return Err(PairParseError::Malformed),
        }
        if !query.bytes().all(|b| b.is_ascii_graphic()) || query.contains('#') {
            return Err(PairParseError::Malformed);
        }

        let mut fields = Fields::default();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or(PairParseError::Malformed)?;
            let value = percent_decode(value)?;
            fields.set(key, value)?;
        }
        fields.finish()
    }
}

#[derive(Default)]
struct Fields {
    name: Option<String>,
    user: Option<String>,
    port: Option<String>,
    addresses: Vec<String>,
    hk: Option<String>,
    pair: Vec<String>,
    otp: Option<String>,
}

impl Fields {
    fn set(&mut self, key: &str, value: String) -> Result<(), PairParseError> {
        fn once(
            slot: &mut Option<String>,
            name: &'static str,
            value: String,
        ) -> Result<(), PairParseError> {
            if slot.replace(value).is_some() {
                Err(PairParseError::DuplicateField(name))
            } else {
                Ok(())
            }
        }
        match key {
            "name" => once(&mut self.name, "name", value),
            "user" => once(&mut self.user, "user", value),
            "port" => once(&mut self.port, "port", value),
            "hk" => once(&mut self.hk, "hk", value),
            "otp" => once(&mut self.otp, "otp", value),
            "a" => {
                self.addresses.push(value);
                Ok(())
            }
            "pair" => {
                self.pair.push(value);
                Ok(())
            }
            _ => Err(PairParseError::UnknownField),
        }
    }

    fn finish(self) -> Result<PairOffer, PairParseError> {
        let name = label(self.name.ok_or(PairParseError::MissingField("name"))?)
            .ok_or(PairParseError::InvalidField("name"))?;
        let username = label(self.user.ok_or(PairParseError::MissingField("user"))?)
            .ok_or(PairParseError::InvalidField("user"))?;
        let port = parse_port(&self.port.ok_or(PairParseError::MissingField("port"))?)
            .ok_or(PairParseError::InvalidField("port"))?;

        if self.addresses.is_empty() {
            return Err(PairParseError::MissingField("a"));
        }
        if self.addresses.len() > MAX_ADDRESSES {
            return Err(PairParseError::InvalidField("a"));
        }
        let mut addresses: Vec<Endpoint> = Vec::new();
        for address in &self.addresses {
            let endpoint = host_endpoint(address, port).ok_or(PairParseError::InvalidField("a"))?;
            if addresses.contains(&endpoint) {
                return Err(PairParseError::InvalidField("a"));
            }
            addresses.push(endpoint);
        }

        let host_key = parse_host_key(&self.hk.ok_or(PairParseError::MissingField("hk"))?)
            .ok_or(PairParseError::InvalidField("hk"))?;

        let exchange = match (self.pair.is_empty(), self.otp) {
            (true, None) => None,
            (false, Some(otp)) => {
                if self.pair.len() > MAX_PAIR_ADDRESSES {
                    return Err(PairParseError::InvalidField("pair"));
                }
                let mut endpoints: Vec<Endpoint> = Vec::new();
                for pair in &self.pair {
                    let endpoint =
                        pair_endpoint(pair).ok_or(PairParseError::InvalidField("pair"))?;
                    if endpoints.contains(&endpoint) {
                        return Err(PairParseError::InvalidField("pair"));
                    }
                    endpoints.push(endpoint);
                }
                let otp = Otp::from_base32(&otp).ok_or(PairParseError::InvalidField("otp"))?;
                Some(PairExchange { endpoints, otp })
            }
            (true, Some(_)) => return Err(PairParseError::MissingField("pair")),
            (false, None) => return Err(PairParseError::MissingField("otp")),
        };

        Ok(PairOffer {
            name,
            username,
            port,
            addresses,
            host_key,
            exchange,
        })
    }
}

fn percent_decode(value: &str) -> Result<String, PairParseError> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let high = bytes.get(i + 1).copied().and_then(hex);
            let low = bytes.get(i + 2).copied().and_then(hex);
            match (high, low) {
                (Some(high), Some(low)) => out.push(high << 4 | low),
                _ => return Err(PairParseError::Malformed),
            }
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| PairParseError::Malformed)
}

/// A name, user or device label: 1 to 64 characters after trimming, no control characters.
fn label(value: String) -> Option<String> {
    let trimmed = value.trim();
    let valid = !trimmed.is_empty()
        && trimmed.chars().count() <= MAX_LABEL_CHARS
        && !trimmed.chars().any(char::is_control);
    valid.then(|| trimmed.to_owned())
}

fn parse_port(value: &str) -> Option<u16> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<u16>().ok().filter(|port| *port != 0)
}

/// A host name or IP literal of a payload address: letters, digits, `.`, `-`, `_` and `:` (IPv6).
fn host_endpoint(host: &str, port: u16) -> Option<Endpoint> {
    let plain = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'));
    if !plain {
        return None;
    }
    Endpoint::new(host, port).ok()
}

/// `<ipv4>:<port>` or `[<ipv6>]:<port>`: always an IP literal, never a name to resolve, and
/// never an address nothing can listen on.
fn pair_endpoint(value: &str) -> Option<Endpoint> {
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        (host, port)
    } else {
        value.rsplit_once(':')?
    };
    let ip: IpAddr = host.parse().ok()?;
    if ip.is_unspecified() || ip.is_multicast() {
        return None;
    }
    if let IpAddr::V4(v4) = ip
        && v4.is_broadcast()
    {
        return None;
    }
    // An IPv4 literal had no brackets and an IPv6 one had: no `1.2.3.4` in brackets, no bare `::1:80`.
    if ip.is_ipv6() != value.starts_with('[') {
        return None;
    }
    Endpoint::new(host, parse_port(port)?).ok()
}

/// `<algorithm> <base64>` with nothing after it: the host's plain public key.
fn parse_host_key(value: &str) -> Option<HostKey> {
    let mut tokens = value.split(' ');
    let (algorithm, blob) = (tokens.next()?, tokens.next()?);
    if tokens.next().is_some() || algorithm.is_empty() || blob.is_empty() {
        return None;
    }
    let key = HostKey::from_openssh(value).ok()?;
    match key.public_key().algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => Some(key),
        _ => None,
    }
}

// --- the exchange ------------------------------------------------------------------------------

/// How long each step may take. The host's own window is 120 s, so the wait for its verdict (a
/// person typing `y`) is that plus a margin; everything else is quick.
#[derive(Debug, Clone, Copy)]
pub struct PairTiming {
    /// Each connect, the host's hello and the write of the request: 10 s.
    pub step: Duration,
    /// The wait for the host's verdict after the request: 125 s.
    pub verdict: Duration,
}

impl Default for PairTiming {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(10),
            verdict: Duration::from_secs(125),
        }
    }
}

/// Why the host said no (`{"ok":false,"reason":…}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The person at the host answered `n`.
    Declined,
    /// The password did not match: an old or wrong code, or the code was already used.
    AuthenticationFailed,
    /// The host would not accept this kind of key.
    KeyNotAccepted,
    /// Nobody answered on the host before its window closed.
    TimedOut,
    /// The host could not read the request.
    BadRequest,
    /// A reason this app does not know.
    Other,
}

impl Refusal {
    fn of(reason: Option<&str>) -> Self {
        match reason {
            Some("declined") => Self::Declined,
            Some("authentication") => Self::AuthenticationFailed,
            Some("key") => Self::KeyNotAccepted,
            Some("timeout") => Self::TimedOut,
            Some("request") => Self::BadRequest,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    #[error("this pairing code has no listener (it was made with --no-listen)")]
    NoExchange,
    #[error("the phone key is not an OpenSSH public key this app can authorize")]
    InvalidKey,
    #[error(
        "the device label must be 1 to {MAX_LABEL_CHARS} characters without control characters"
    )]
    InvalidDevice,
    #[error("the host cannot be reached on its pairing addresses")]
    Unreachable,
    #[error("the host did not answer in time")]
    TimedOut,
    #[error("the host does not speak this pairing protocol")]
    Protocol,
    #[error("the connection to the host ended early")]
    ConnectionLost,
    #[error("the host refused: {0:?}")]
    Refused(Refusal),
}

#[derive(Deserialize)]
struct Hello {
    v: u32,
    nonce: String,
}

#[derive(Serialize)]
struct Request<'a> {
    v: u32,
    key: &'a str,
    device: &'a str,
    mac: String,
}

#[derive(Deserialize)]
struct Reply {
    ok: bool,
    reason: Option<String>,
}

/// A key the host may authorize, in the form that is sent: algorithm and key data, no comment.
fn clean_key(line: &str) -> Result<String, PairError> {
    let mut key = PublicKey::from_openssh(line.trim()).map_err(|_| PairError::InvalidKey)?;
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => {}
        _ => return Err(PairError::InvalidKey),
    }
    key.set_comment("");
    key.to_openssh().map_err(|_| PairError::InvalidKey)
}

/// Sends `public_key_line` to the host named by `offer`, proving knowledge of the one-time
/// password, and resolves when the host's user has confirmed (or refused). The password is used
/// once and is not kept anywhere but the offer.
///
/// Every connection goes through `transport`: the offer's `pair` addresses are raced, the
/// winner carries the exchange. Reads are bounded and every step has its [`PairTiming`] limit.
/// Dropping the future cancels it and closes the connection.
pub async fn submit_key<T: Transport>(
    transport: &Arc<T>,
    offer: &PairOffer,
    public_key_line: &str,
    device: &str,
    timing: PairTiming,
) -> Result<(), PairError> {
    let exchange = offer.exchange.as_ref().ok_or(PairError::NoExchange)?;
    let key = clean_key(public_key_line)?;
    let device = label(device.to_owned()).ok_or(PairError::InvalidDevice)?;
    let raced = race_with(
        transport,
        &exchange.endpoints,
        RaceTiming {
            stagger: RACE_STAGGER,
            address_timeout: timing.step,
        },
        None,
    )
    .await
    .map_err(|failure| {
        if failure
            .errors
            .iter()
            .all(|error| error.kind() == std::io::ErrorKind::TimedOut)
        {
            PairError::TimedOut
        } else {
            PairError::Unreachable
        }
    })?;
    converse(raced.stream, &exchange.otp, &key, &device, timing).await
}

/// The exchange on an established stream.
pub async fn converse<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    otp: &Otp,
    key: &str,
    device: &str,
    timing: PairTiming,
) -> Result<(), PairError> {
    let hello = within(timing.step, read_line(&mut stream, HELLO_LIMIT)).await??;
    let hello: Hello = serde_json::from_slice(&hello).map_err(|_| PairError::Protocol)?;
    if hello.v != 1 {
        return Err(PairError::Protocol);
    }
    let nonce = STANDARD
        .decode(hello.nonce.as_bytes())
        .ok()
        .filter(|nonce| nonce.len() == NONCE_BYTES)
        .ok_or(PairError::Protocol)?;

    let request = Request {
        v: 1,
        key,
        device,
        mac: STANDARD.encode(otp.mac(&nonce, key)),
    };
    let mut line = serde_json::to_vec(&request).map_err(|_| PairError::Protocol)?;
    line.push(b'\n');
    within(timing.step, async {
        stream
            .write_all(&line)
            .await
            .map_err(|_| PairError::ConnectionLost)?;
        stream.flush().await.map_err(|_| PairError::ConnectionLost)
    })
    .await??;

    let reply = within(timing.verdict, read_line(&mut stream, REPLY_LIMIT)).await??;
    let reply: Reply = serde_json::from_slice(&reply).map_err(|_| PairError::Protocol)?;
    if reply.ok {
        Ok(())
    } else {
        Err(PairError::Refused(Refusal::of(reply.reason.as_deref())))
    }
}

async fn within<F: std::future::Future>(
    limit: Duration,
    future: F,
) -> Result<F::Output, PairError> {
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| PairError::TimedOut)
}

/// One line without its terminator, at most `limit` bytes: a longer one is a protocol error, not
/// a reason to buffer without bound.
async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Vec<u8>, PairError> {
    let mut line = Vec::new();
    let mut chunk = [0u8; 128];
    loop {
        let count = reader
            .read(&mut chunk)
            .await
            .map_err(|_| PairError::ConnectionLost)?;
        if count == 0 {
            return Err(PairError::ConnectionLost);
        }
        let end = chunk[..count].iter().position(|byte| *byte == b'\n');
        line.extend_from_slice(&chunk[..end.unwrap_or(count)]);
        if line.len() > limit {
            return Err(PairError::Protocol);
        }
        if end.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(line);
        }
    }
}

#[cfg(test)]
#[path = "pair_tests.rs"]
mod tests;
