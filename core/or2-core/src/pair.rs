//! Easy pair, phone side (version 2): the pairing code `K` the phone shows, the strict parser of
//! the code the host prints as a QR, the bootstrap key both derive, and the client that enrols the
//! phone's key over the host's own sshd. See `docs/contracts.md`, "Easy pair".
//!
//! The QR is public data (nothing secret in it):
//!
//! ```text
//! or2-pair:2?name=<label>&user=<u>&port=<p>&a=<addr>&a=<addr>…&hk=<algo> <base64>&id=<pairing id>
//! ```
//!
//! Values are percent-encoded. `id` is absent in a code made with `--manual`, which only describes
//! the host. The parser is strict about every field and returns typed errors; it never guesses.
//!
//! The one secret is [`PairCode`], shown on the phone and typed at the host. Both derive the same
//! Ed25519 bootstrap key from it and the pairing id ([`PairCode`]'s derivation, below); the host
//! authorizes that key for one forced command, and the phone logs in with it over SSH (host key
//! pinned from the QR), runs `or2-pair` and exchanges newline-delimited JSON on the session channel:
//!
//! ```text
//! host  -> {"v":2,"hello":"or2-pair","id":"<id>"}
//! phone -> {"v":2,"key":"<algo> <base64>","device":"<label>"}
//! host  -> {"v":2,"ok":true,"user":"<account>","fingerprint":"SHA256:…"}
//!        | {"v":2,"ok":false,"reason":"expired|gone|key|failed|request"}
//! ```
//!
//! SSH authenticates both ends and encrypts the channel, so the exchange carries no MACs.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use data_encoding::BASE32_NOPAD;
use hkdf::Hkdf;
use rand::TryRng;
use rand::rngs::SysRng;
use russh::keys::ssh_key::{Algorithm, HashAlg, PublicKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::keys::ClientKey;
use crate::ssh::{Next, PairSession};
use crate::transport::{Endpoint, RACE_STAGGER, RaceTiming, Transport, race_with};
use crate::trust::HostKey;

/// The URI scheme and version prefix of a pairing code.
pub const PAYLOAD_PREFIX: &str = "or2-pair:";
/// The version of the pairing code and of the exchange.
pub const VERSION: u32 = 2;
/// A pairing code is at most this many bytes (the host trims addresses to fit).
pub const MAX_PAYLOAD_BYTES: usize = 1024;
/// At most as many `a` addresses as a host may have.
pub const MAX_ADDRESSES: usize = 8;
/// Longest label for a host name, user name or device.
pub const MAX_LABEL_CHARS: usize = 64;
/// The pairing id: 8 random bytes as 13 lowercase RFC 4648 base32 characters.
pub const PAIRING_ID_CHARS: usize = 13;
/// The command the phone asks sshd to run (the bootstrap key's forced command runs whatever is asked).
pub const PAIR_COMMAND: &str = "or2-pair";
/// The longest line the phone accepts from the host (the hello and the verdict), and the host from
/// the phone.
pub const HELLO_LIMIT: usize = 256;
pub const REQUEST_LIMIT: usize = 2048;
pub const VERDICT_LIMIT: usize = 512;
/// How many bytes of shell noise (an rc file that prints) may precede the hello.
pub const NOISE_LIMIT: usize = 4096;
/// The first bytes of the hello line; the phone skips everything before the first line that starts
/// with them.
const HELLO_PREFIX: &[u8] = br#"{"v":2,"hello""#;

// --- the pairing code K ------------------------------------------------------------------------

/// Crockford base32 without `Z`: `0-9` and `A-Y` without `I`, `L`, `O`, `U`. 31 symbols, one for
/// each value modulo the prime [`CHECK_MODULUS`], so no two characters share a check value (with
/// `Z`, value 31, a `0` typed for a `Z` passed the check).
const ALPHABET: &[u8; 31] = b"0123456789ABCDEFGHJKMNPQRSTVWXY";
/// Random characters of `K`; a twelfth is the check character.
const DATA_CHARS: usize = 11;
const CODE_CHARS: usize = DATA_CHARS + 1;
const CHECK_MODULUS: u32 = 31;
/// Random bytes below this (8 · 31) map onto the 31 symbols uniformly; the rest are dropped.
const RANDOM_LIMIT: u8 = (256 / CHECK_MODULUS * CHECK_MODULUS) as u8;
/// HKDF `info` of the bootstrap key.
const BOOTSTRAP_INFO: &[u8] = b"or2-pair/2 bootstrap ed25519";

/// Why typed input is not a pairing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairCodeError {
    #[error("a pairing code has 12 characters")]
    Length,
    #[error("a pairing code has only digits and letters (no U or Z)")]
    Character,
    #[error("that code has a typo")]
    Check,
}

/// The pairing code `K`: 11 random characters of the 31-symbol alphabet (about 54.5 bits from the
/// OS CSPRNG) and a check character. It is the one secret of an Easy pair: shown on the phone, typed at the host, never
/// logged or saved. Zeroized on drop; `Debug` never shows it.
pub struct PairCode(Zeroizing<[u8; CODE_CHARS]>);

// The only field is a `Zeroizing`, which wipes it when the code is dropped.
impl zeroize::ZeroizeOnDrop for PairCode {}

impl fmt::Debug for PairCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairCode(<redacted>)")
    }
}

impl PairCode {
    /// A new code from the operating system's CSPRNG.
    pub fn generate() -> Self {
        Self::from_random(|pool| {
            SysRng
                .try_fill_bytes(pool)
                .expect("the operating system provides random bytes");
        })
    }

    /// The data characters from the random bytes `fill` writes, one per kept byte
    /// ([`random_value`]: rejection sampling, so each of the 31 values is equally likely); `fill`
    /// is called again until 11 bytes were kept.
    fn from_random(mut fill: impl FnMut(&mut [u8])) -> Self {
        let mut pool = Zeroizing::new([0u8; 2 * DATA_CHARS]);
        let mut values = Zeroizing::new([0u8; CODE_CHARS]);
        let mut count = 0;
        while count < DATA_CHARS {
            fill(&mut pool[..]);
            for value in pool.iter().filter_map(|byte| random_value(*byte)) {
                if count == DATA_CHARS {
                    break;
                }
                values[count] = value;
                count += 1;
            }
        }
        values[DATA_CHARS] = check_value(&values[..DATA_CHARS]);
        Self(values)
    }

    /// Reads a code as a person types it: case-insensitive, hyphens and spaces ignored, `I` and
    /// `L` read as `1`, `O` as `0`. A character codes never use (`U`, `Z`, anything not a digit or
    /// letter) is [`PairCodeError::Character`]; a wrong check character is
    /// [`PairCodeError::Check`].
    pub fn parse_typed(text: &str) -> Result<Self, PairCodeError> {
        let mut values = Zeroizing::new([0u8; CODE_CHARS]);
        let mut count = 0;
        for character in text.chars().filter(|c| !matches!(c, '-' | ' ')) {
            let mapped = match character.to_ascii_uppercase() {
                'I' | 'L' => '1',
                'O' => '0',
                other => other,
            };
            let value = u8::try_from(mapped)
                .ok()
                .and_then(|byte| ALPHABET.iter().position(|a| *a == byte))
                .ok_or(PairCodeError::Character)?;
            if count == CODE_CHARS {
                return Err(PairCodeError::Length);
            }
            values[count] = value as u8;
            count += 1;
        }
        if count != CODE_CHARS {
            return Err(PairCodeError::Length);
        }
        if values[DATA_CHARS] != check_value(&values[..DATA_CHARS]) {
            return Err(PairCodeError::Check);
        }
        Ok(Self(values))
    }

    /// `7KQ4-M2XD-9PTM`: three groups of four, for the screen.
    pub fn display(&self) -> String {
        let mut text = String::with_capacity(CODE_CHARS + 2);
        for (index, value) in self.0.iter().enumerate() {
            if index > 0 && index % 4 == 0 {
                text.push('-');
            }
            text.push(char::from(ALPHABET[usize::from(*value)]));
        }
        text
    }

    /// The 11 data characters as ASCII uppercase: the key material of the derivation.
    fn data(&self) -> Zeroizing<[u8; DATA_CHARS]> {
        let mut ascii = Zeroizing::new([0u8; DATA_CHARS]);
        for (character, value) in ascii.iter_mut().zip(&self.0[..DATA_CHARS]) {
            *character = ALPHABET[usize::from(*value)];
        }
        ascii
    }

    /// The bootstrap key's RFC 8032 seed: `HKDF-SHA256(ikm = the 11 data characters as ASCII
    /// uppercase, salt = the pairing id's 13 ASCII characters, info = "or2-pair/2 bootstrap
    /// ed25519")`, 32 bytes. Salting with the id binds the key to one run of `or2-pair`.
    fn bootstrap_seed(&self, pairing_id: &str) -> Zeroizing<[u8; 32]> {
        let ikm = self.data();
        let mut seed = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(pairing_id.as_bytes()), &*ikm)
            .expand(BOOTSTRAP_INFO, &mut *seed)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        seed
    }

    fn bootstrap_key(&self, pairing_id: &str) -> ClientKey {
        ClientKey::from_ed25519_seed(&self.bootstrap_seed(pairing_id))
    }

    /// The bootstrap key's public half as an `authorized_keys` key (`ssh-ed25519 <base64>`): what
    /// the host installs. Tests of the phone's client need it to play the host.
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap_public_key(&self, pairing_id: &str) -> String {
        let key = self.bootstrap_key(pairing_id).public_key().openssh;
        key.split(' ').take(2).collect::<Vec<_>>().join(" ")
    }
}

/// The value of one random byte, or `None` for the 8 bytes from 248 up, which are dropped: the 248
/// below map onto the 31 values 8 times each.
fn random_value(byte: u8) -> Option<u8> {
    (byte < RANDOM_LIMIT).then(|| byte % CHECK_MODULUS as u8)
}

/// `Σ i·vᵢ (i = 1..11) mod 31`: catches every single wrong character and every swap of two
/// neighbours, as 31 is prime, the weights differ and the 31 values are distinct modulo 31.
fn check_value(data: &[u8]) -> u8 {
    let sum: u32 = data
        .iter()
        .zip(1u32..)
        .map(|(value, weight)| weight * u32::from(*value))
        .sum();
    (sum % CHECK_MODULUS) as u8
}

// --- the pairing code the host prints ----------------------------------------------------------

/// Why a pairing code was refused. The field names are the payload's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairParseError {
    #[error("this is not an or2 pairing code")]
    NotPairingCode,
    /// A version other than 2: older ones need `or2-pair` updated on the host, newer ones this app.
    #[error("this pairing code is version {version}, which this app does not read")]
    UnsupportedVersion { version: u32 },
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
    /// The host's public key: pinned for the pairing and trusted afterwards.
    pub host_key: HostKey,
    /// This run's pairing id. `None` for a code made with `--manual`: the user installs the
    /// phone's key by hand.
    pub pairing_id: Option<String>,
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
        match version_number(version).ok_or(PairParseError::Malformed)? {
            VERSION => {}
            version => return Err(PairParseError::UnsupportedVersion { version }),
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

/// A version number: digits only, no leading zero.
fn version_number(text: &str) -> Option<u32> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    plain.then(|| text.parse().ok()).flatten()
}

/// Whether `id` is a pairing id: exactly 13 lowercase base32 characters that decode to 8 bytes
/// (canonical: the last character carries no stray bit).
pub fn is_pairing_id(id: &str) -> bool {
    id.len() == PAIRING_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        && BASE32_NOPAD
            .decode(id.to_ascii_uppercase().as_bytes())
            .is_ok_and(|bytes| bytes.len() == 8)
}

#[derive(Default)]
struct Fields {
    name: Option<String>,
    user: Option<String>,
    port: Option<String>,
    addresses: Vec<String>,
    hk: Option<String>,
    id: Option<String>,
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
            "id" => once(&mut self.id, "id", value),
            "a" => {
                self.addresses.push(value);
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

        let pairing_id = match self.id {
            None => None,
            Some(id) if is_pairing_id(&id) => Some(id),
            Some(_) => return Err(PairParseError::InvalidField("id")),
        };

        Ok(PairOffer {
            name,
            username,
            port,
            addresses,
            host_key,
            pairing_id,
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

// --- the enrolment -----------------------------------------------------------------------------

/// How long each step may take (the contract's 10 s). Tests shorten it.
#[derive(Debug, Clone, Copy)]
pub struct PairTiming {
    /// Each address of the race, the handshake, the authentication, the channel and `exec`, the
    /// wait for the hello, the write of the request and the wait for the verdict.
    pub step: Duration,
}

impl Default for PairTiming {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(10),
        }
    }
}

/// What the host reported after it installed the phone's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairResult {
    /// The account the key was added to.
    pub username: String,
    /// `SHA256:…` of the key the host installed (the phone checks it is its own).
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairError {
    #[error("this pairing code has no pairing id (it was made with --manual)")]
    NoPairingId,
    #[error("the offer is not one `PairOffer::parse` made")]
    InvalidOffer,
    #[error("the phone key is not an OpenSSH public key this app can authorize")]
    InvalidKey,
    #[error(
        "the device label must be 1 to {MAX_LABEL_CHARS} characters without control characters"
    )]
    InvalidDevice,
    #[error("the host cannot be reached on its addresses")]
    Unreachable,
    #[error("the host did not answer in time")]
    TimedOut,
    /// The host presented a different key than the code's: the connection ended before
    /// authentication and nothing was sent.
    #[error("the host presented a different key than the pairing code")]
    HostKeyMismatch,
    /// The host did not accept the bootstrap key: another code was typed, the run ended or
    /// expired, or sshd ignores `authorized_keys`.
    #[error("the host did not accept this phone's code")]
    BootstrapRefused,
    /// Not the hello of `or2-pair` (a `ForceCommand`, another program, too much shell noise).
    #[error("something other than or2-pair answered on the host")]
    NotOr2Pair,
    #[error("the host does not speak this pairing protocol")]
    Protocol,
    #[error("the connection to the host ended early")]
    ConnectionLost,
    #[error("or2-pair has stopped or timed out on the host")]
    Expired,
    #[error("another device already used this pairing")]
    Gone,
    #[error("the host does not accept this key")]
    KeyNotAccepted,
    #[error("the host could not add the key")]
    HostFailed,
    /// `request`, or a reason this app does not know.
    #[error("the host refused")]
    Refused,
}

#[derive(Serialize)]
struct Request<'a> {
    v: u32,
    key: &'a str,
    device: &'a str,
}

#[derive(Deserialize)]
struct Hello {
    v: u32,
    hello: String,
    id: String,
}

#[derive(Deserialize)]
struct Verdict {
    v: u32,
    ok: bool,
    user: Option<String>,
    fingerprint: Option<String>,
    reason: Option<String>,
}

/// The phone's key as it is sent (`<algorithm> <base64>`, no comment) and its fingerprint.
fn clean_key(line: &str) -> Result<(String, String), PairError> {
    let mut key = PublicKey::from_openssh(line.trim()).map_err(|_| PairError::InvalidKey)?;
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => {}
        _ => return Err(PairError::InvalidKey),
    }
    key.set_comment("");
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let line = key.to_openssh().map_err(|_| PairError::InvalidKey)?;
    Ok((line, fingerprint))
}

/// Enrols `public_key_line` (an OpenSSH public key line; its comment is not sent) with the host
/// that made `offer`, using the pairing code `code` the person typed there, and resolves with the
/// host's report. One run, in order:
///
/// 1. the offer's addresses are raced through `transport` and **one** SSH handshake runs on the
///    winner, accepting only the pinned host key ([`PairError::HostKeyMismatch`] otherwise);
/// 2. one authentication with the bootstrap key derived from `code` and the pairing id, then the
///    key is zeroized ([`PairError::BootstrapRefused`] if the host does not accept it);
/// 3. one session channel, no PTY, `exec "or2-pair"`; shell noise before the hello is skipped up to
///    [`NOISE_LIMIT`] bytes, and the hello's id must be the offer's;
/// 4. the request, then the host's verdict, every refusal mapped to its own [`PairError`].
///
/// Every step has [`PairTiming::step`]. The channel and the connection are closed through the
/// connection's own close path however this ends, and dropping the future cancels the pairing the
/// same way.
pub async fn pair_enroll<T: Transport>(
    transport: &Arc<T>,
    offer: &PairOffer,
    code: &PairCode,
    public_key_line: &str,
    device: &str,
    timing: PairTiming,
) -> Result<PairResult, PairError> {
    let id = offer.pairing_id.as_deref().ok_or(PairError::NoPairingId)?;
    if !is_pairing_id(id) || offer.addresses.is_empty() {
        return Err(PairError::InvalidOffer);
    }
    let (key, fingerprint) = clean_key(public_key_line)?;
    let device = label(device.to_owned()).ok_or(PairError::InvalidDevice)?;
    let request = request_line(&key, &device)?;

    let raced = race_with(
        transport,
        &offer.addresses,
        RaceTiming {
            stagger: RACE_STAGGER,
            address_timeout: timing.step,
        },
        None,
    )
    .await
    .map_err(|failure| {
        let all_timed_out = failure
            .errors
            .iter()
            .all(|error| error.kind() == std::io::ErrorKind::TimedOut);
        if all_timed_out {
            PairError::TimedOut
        } else {
            PairError::Unreachable
        }
    })?;

    let bootstrap = code.bootstrap_key(id);
    let mut session = PairSession::open(
        raced.stream,
        &offer.username,
        &offer.host_key,
        bootstrap,
        timing.step,
    )
    .await?;
    let result = converse(&mut session, id, &request, &fingerprint, timing.step).await;
    session.close().await;
    result
}

fn request_line(key: &str, device: &str) -> Result<Vec<u8>, PairError> {
    let mut line = serde_json::to_vec(&Request {
        v: VERSION,
        key,
        device,
    })
    .map_err(|_| PairError::Protocol)?;
    if line.len() >= REQUEST_LIMIT {
        return Err(PairError::InvalidKey);
    }
    line.push(b'\n');
    Ok(line)
}

/// The exchange on an open session.
async fn converse(
    session: &mut PairSession,
    id: &str,
    request: &[u8],
    fingerprint: &str,
    step: Duration,
) -> Result<PairResult, PairError> {
    let mut rest = timeout(step, read_hello(session, id))
        .await
        .map_err(|_| PairError::TimedOut)??;
    timeout(step, session.send(request))
        .await
        .map_err(|_| PairError::TimedOut)??;
    let line = timeout(step, read_line(session, &mut rest, VERDICT_LIMIT))
        .await
        .map_err(|_| PairError::TimedOut)??;
    verdict(&line, fingerprint)
}

/// Reads until the hello line, skipping shell noise, and returns what followed it. The hello is
/// the first line that starts with [`HELLO_PREFIX`] and begins within [`NOISE_LIMIT`] bytes;
/// anything else is `NotOr2Pair`. Its `id` must be `id`.
async fn read_hello(session: &mut PairSession, id: &str) -> Result<Vec<u8>, PairError> {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        if let Some(found) = locate_hello(&buffer)? {
            let hello: Hello = serde_json::from_slice(&buffer[found.start..found.end])
                .map_err(|_| PairError::Protocol)?;
            if hello.v != VERSION || hello.hello != PAIR_COMMAND || hello.id != id {
                return Err(PairError::Protocol);
            }
            return Ok(buffer.split_off(found.after));
        }
        match session.next().await {
            Next::Data(data) => buffer.extend_from_slice(&data),
            // The command ran and exited without a hello (not found, a ForceCommand, an rc file).
            Next::Closed => return Err(PairError::NotOr2Pair),
            Next::Lost => return Err(PairError::ConnectionLost),
        }
    }
}

/// Where the hello line is in a buffer: its bytes are `start..end` (no terminator) and what
/// follows it begins at `after`.
struct Located {
    start: usize,
    end: usize,
    after: usize,
}

/// The hello line in `buffer`: `None` if it may still come, an error if it cannot any more.
fn locate_hello(buffer: &[u8]) -> Result<Option<Located>, PairError> {
    let mut start = 0;
    loop {
        if start > NOISE_LIMIT {
            return Err(PairError::NotOr2Pair);
        }
        let line = &buffer[start.min(buffer.len())..];
        let end = line.iter().position(|byte| *byte == b'\n');
        if line.starts_with(HELLO_PREFIX) {
            return match end {
                Some(end) if end <= HELLO_LIMIT => Ok(Some(Located {
                    start,
                    end: start + trim_cr(line, end),
                    after: start + end + 1,
                })),
                _ if line.len() > HELLO_LIMIT => Err(PairError::Protocol),
                _ => Ok(None),
            };
        }
        match end {
            Some(end) => start += end + 1,
            // An unfinished line: it may still become the hello if it is its beginning, or the
            // next line may start within the budget.
            None if HELLO_PREFIX.starts_with(line) => return Ok(None),
            None if buffer.len() > NOISE_LIMIT => return Err(PairError::NotOr2Pair),
            None => return Ok(None),
        }
    }
}

/// `end` of a line without a trailing carriage return.
fn trim_cr(line: &[u8], end: usize) -> usize {
    if end > 0 && line[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

/// One line without its terminator, at most `limit` bytes, starting with what is already in
/// `buffer`.
async fn read_line(
    session: &mut PairSession,
    buffer: &mut Vec<u8>,
    limit: usize,
) -> Result<Vec<u8>, PairError> {
    loop {
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            if end > limit {
                return Err(PairError::Protocol);
            }
            let mut line: Vec<u8> = buffer.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(line);
        }
        if buffer.len() > limit {
            return Err(PairError::Protocol);
        }
        match session.next().await {
            Next::Data(data) => buffer.extend_from_slice(&data),
            // The command ended or the connection broke before it answered.
            Next::Closed | Next::Lost => return Err(PairError::ConnectionLost),
        }
    }
}

/// The host's verdict: a success carries the account and the fingerprint of the key it installed,
/// which must be the key that was sent; a refusal carries a reason.
fn verdict(line: &[u8], fingerprint: &str) -> Result<PairResult, PairError> {
    let verdict: Verdict = serde_json::from_slice(line).map_err(|_| PairError::Protocol)?;
    if verdict.v != VERSION {
        return Err(PairError::Protocol);
    }
    if verdict.ok {
        let user = verdict
            .user
            .filter(|user| !user.is_empty() && !user.chars().any(char::is_control))
            .ok_or(PairError::Protocol)?;
        if verdict.fingerprint.as_deref() != Some(fingerprint) {
            return Err(PairError::Protocol);
        }
        return Ok(PairResult {
            username: user,
            fingerprint: fingerprint.to_owned(),
        });
    }
    Err(match verdict.reason.as_deref() {
        Some("expired") => PairError::Expired,
        Some("gone") => PairError::Gone,
        Some("key") => PairError::KeyNotAccepted,
        Some("failed") => PairError::HostFailed,
        _ => PairError::Refused,
    })
}

#[cfg(test)]
#[path = "pair_tests.rs"]
mod tests;
