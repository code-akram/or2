//! Easy pair for Kotlin (FFI API 11): parse a pairing code, and send the phone's key to the
//! host that showed it. See docs/contracts.md, "Easy pair".
//!
//! The one-time password never reaches Kotlin: `PairOffer.exchange.secret` is an opaque object
//! that Rust holds and zeroizes (`wipe()`, or when the last reference goes). Its generated
//! `toString()` shows nothing, and `Debug` is redacted.

use std::fmt;
use std::sync::{Arc, Mutex};

use or2_core::pair as core;
use or2_core::transport::{DirectTcp, Endpoint};

use crate::host::HostAddress;
use crate::keys::PublicKeyInfo;

/// The one-time password, held by Rust. Opaque to Kotlin.
#[derive(uniffi::Object)]
pub struct PairSecret {
    otp: Mutex<Option<core::Otp>>,
}

impl fmt::Debug for PairSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairSecret(<redacted>)")
    }
}

impl PairSecret {
    fn new(otp: core::Otp) -> Arc<Self> {
        Arc::new(Self {
            otp: Mutex::new(Some(otp)),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<core::Otp>> {
        self.otp
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn take_copy(&self) -> Option<core::Otp> {
        self.lock().clone()
    }
}

#[uniffi::export]
impl PairSecret {
    /// Overwrites the password now. Call it when the pairing flow ends, however it ends;
    /// submitting afterwards fails with `Wiped`. Idempotent.
    pub fn wipe(&self) {
        if let Some(mut otp) = self.lock().take() {
            otp.wipe();
        }
    }

    pub fn is_wiped(&self) -> bool {
        self.lock().is_none()
    }
}

/// How to reach the host's one-shot listener.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairExchange {
    /// Where the host listens, in preference order (IP literals).
    pub endpoints: Vec<HostAddress>,
    pub secret: Arc<PairSecret>,
}

/// A validated pairing code.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairOffer {
    /// The host's own name for itself, to pre-fill the host's label.
    pub name: String,
    pub username: String,
    /// The SSH port; every address in `addresses` carries it.
    pub port: u16,
    /// Where SSH reaches the host, in preference order.
    pub addresses: Vec<HostAddress>,
    /// The host's key, to trust before the first connection (`fingerprint` is for display).
    pub host_key: PublicKeyInfo,
    /// `None` for a code made with `--no-listen`: there is nothing to submit to, so the phone
    /// shows its public key for the user to install by hand.
    pub exchange: Option<PairExchange>,
}

fn address(endpoint: &Endpoint) -> HostAddress {
    HostAddress {
        host: endpoint.host().to_owned(),
        port: endpoint.port(),
    }
}

impl From<core::PairOffer> for PairOffer {
    fn from(offer: core::PairOffer) -> Self {
        Self {
            name: offer.name,
            username: offer.username,
            port: offer.port,
            addresses: offer.addresses.iter().map(address).collect(),
            host_key: offer.host_key.info().into(),
            exchange: offer.exchange.map(|exchange| PairExchange {
                endpoints: exchange.endpoints.iter().map(address).collect(),
                secret: PairSecret::new(exchange.otp),
            }),
        }
    }
}

/// Why a pairing code was refused. `field` is the payload's own field name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PairParseError {
    #[error("this is not an or2 pairing code")]
    NotPairingCode,
    #[error("this pairing code is from a newer or2-pair; update the app")]
    UnsupportedVersion,
    #[error("the pairing code is too long")]
    TooLong,
    #[error("the pairing code is not well formed")]
    Malformed,
    #[error("the pairing code has no `{field}`")]
    MissingField { field: String },
    #[error("the pairing code has `{field}` twice")]
    DuplicateField { field: String },
    #[error("the pairing code has a field this app does not know")]
    UnknownField,
    #[error("the pairing code's `{field}` is not valid")]
    InvalidField { field: String },
}

impl From<core::PairParseError> for PairParseError {
    fn from(error: core::PairParseError) -> Self {
        use core::PairParseError as E;
        match error {
            E::NotPairingCode => Self::NotPairingCode,
            E::UnsupportedVersion => Self::UnsupportedVersion,
            E::TooLong => Self::TooLong,
            E::Malformed => Self::Malformed,
            E::MissingField(field) => Self::MissingField {
                field: field.into(),
            },
            E::DuplicateField(field) => Self::DuplicateField {
                field: field.into(),
            },
            E::UnknownField => Self::UnknownField,
            E::InvalidField(field) => Self::InvalidField {
                field: field.into(),
            },
        }
    }
}

/// Why the key could not be sent, or was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PairError {
    #[error("the pairing code has no listener (it was made with --no-listen)")]
    NoExchange,
    #[error("the pairing code was already used or wiped")]
    Wiped,
    #[error("the offer is not one `parse_pair_payload` made")]
    InvalidOffer,
    #[error("the phone key is not an OpenSSH public key this app can authorize")]
    InvalidKey,
    #[error("the device label must be 1 to 64 characters without control characters")]
    InvalidDevice,
    #[error("the host cannot be reached on its pairing addresses")]
    Unreachable,
    #[error("the host did not answer in time")]
    TimedOut,
    #[error("the host does not speak this pairing protocol")]
    Protocol,
    #[error("the connection to the host ended early")]
    ConnectionLost,
    /// The answer did not prove the host knows the code (an old or wrong code, or someone else
    /// answering): nothing was believed and the code is not spent.
    #[error("the host's answer could not be verified")]
    HostNotAuthenticated,
    /// The person at the host answered no.
    #[error("the host declined the key")]
    Declined,
    /// A wrong or already used code.
    #[error("the pairing code did not verify")]
    AuthenticationFailed,
    #[error("the host does not accept this kind of key")]
    KeyNotAccepted,
    /// Nobody answered at the host before its window closed.
    #[error("nobody confirmed on the host in time")]
    HostTimedOut,
    #[error("the host could not read the request")]
    BadRequest,
    /// The host could not write `authorized_keys`; the person at the host sees why.
    #[error("the host could not add the key")]
    HostFailed,
    #[error("the host refused")]
    Refused,
}

impl From<core::PairError> for PairError {
    fn from(error: core::PairError) -> Self {
        use core::PairError as E;
        use core::Refusal as R;
        match error {
            E::NoExchange => Self::NoExchange,
            E::InvalidKey => Self::InvalidKey,
            E::InvalidDevice => Self::InvalidDevice,
            E::Unreachable => Self::Unreachable,
            E::TimedOut => Self::TimedOut,
            E::Protocol => Self::Protocol,
            E::ConnectionLost => Self::ConnectionLost,
            E::HostNotAuthenticated => Self::HostNotAuthenticated,
            E::Refused(R::Declined) => Self::Declined,
            E::Refused(R::AuthenticationFailed) => Self::AuthenticationFailed,
            E::Refused(R::KeyNotAccepted) => Self::KeyNotAccepted,
            E::Refused(R::TimedOut) => Self::HostTimedOut,
            E::Refused(R::BadRequest) => Self::BadRequest,
            E::Refused(R::HostFailed) => Self::HostFailed,
            E::Refused(R::Other) => Self::Refused,
        }
    }
}

/// Parses and validates a pairing code (the text of the QR, or what the user pasted).
#[uniffi::export]
pub fn parse_pair_payload(text: String) -> Result<PairOffer, PairParseError> {
    Ok(core::PairOffer::parse(&text)?.into())
}

/// Sends `public_key_line` (an OpenSSH public key line; its comment is not sent) to the host
/// that made `offer`, proving the one-time password, and resolves when the person at the host
/// has confirmed. Each connect, the host's greeting and the write take at most 10 s; the wait for
/// the confirmation (a person typing `y`) at most the host's own 120 s window plus a margin.
/// `device_label` is what the host shows (and puts in the key's comment); 1 to 64 characters.
///
/// The secret is wiped on success and when the host refused (the code is spent then); after
/// network failures it is kept, so the same code can be tried again while the host still
/// listens. Cancelling the coroutine cancels the exchange and closes the connection. The
/// transport is the app's direct TCP (the phone must reach the host's address itself).
#[uniffi::export(async_runtime = "tokio")]
pub async fn pair_submit_key(
    offer: PairOffer,
    public_key_line: String,
    device_label: String,
) -> Result<(), PairError> {
    let exchange = offer.exchange.ok_or(PairError::NoExchange)?;
    let otp = exchange.secret.take_copy().ok_or(PairError::Wiped)?;
    let endpoints = exchange
        .endpoints
        .iter()
        .map(|a| Endpoint::new(&a.host, a.port))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PairError::InvalidOffer)?;
    if endpoints.is_empty() {
        return Err(PairError::InvalidOffer);
    }
    let result = core::submit_exchange(
        &Arc::new(DirectTcp),
        &core::PairExchange { endpoints, otp },
        &public_key_line,
        &device_label,
        core::PairTiming::default(),
    )
    .await;
    let spent = code_is_spent(&result);
    if spent {
        exchange.secret.wipe();
    }
    result.map_err(Into::into)
}

/// Whether the code has served: a success, or a refusal that carried the host's proof. Anything
/// the host did not authenticate (including a forged success or refusal) and every network failure
/// leaves the code usable.
fn code_is_spent(result: &Result<(), core::PairError>) -> bool {
    matches!(result, Ok(()) | Err(core::PairError::Refused(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_authenticated_outcome_spends_the_code() {
        use self::core::{PairError as E, Refusal};
        assert!(code_is_spent(&Ok(())));
        assert!(code_is_spent(&Err(E::Refused(Refusal::Declined))));
        for kept in [
            E::HostNotAuthenticated,
            E::Protocol,
            E::TimedOut,
            E::Unreachable,
            E::ConnectionLost,
        ] {
            assert!(!code_is_spent(&Err(kept)), "{kept:?}");
        }
        assert_eq!(
            PairError::from(E::HostNotAuthenticated),
            PairError::HostNotAuthenticated
        );
    }

    const CODE: &str = "or2-pair:1?name=Work%20Mac&user=alice&port=22&a=192.168.1.20\
        &hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7\
        &pair=192.168.1.20:41234&otp=AAAQEAYEAUDAOCAJBIFQYDIOB4";

    #[test]
    fn parsing_maps_the_offer_without_exposing_the_password() {
        let offer = parse_pair_payload(CODE.into()).unwrap();
        assert_eq!(offer.name, "Work Mac");
        assert_eq!(offer.addresses[0].host, "192.168.1.20");
        assert_eq!(offer.addresses[0].port, 22);
        assert_eq!(
            offer.host_key.fingerprint,
            "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
        );
        let exchange = offer.exchange.as_ref().unwrap();
        assert_eq!(exchange.endpoints[0].port, 41234);
        let shown = format!("{offer:?}");
        assert!(!shown.contains("AAAQEAYEAUDAOCAJBIFQYDIOB4"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
        assert!(!exchange.secret.is_wiped());
        exchange.secret.wipe();
        assert!(exchange.secret.is_wiped());
    }

    #[test]
    fn parse_errors_carry_the_field_name() {
        let bad = CODE.replace("&user=alice", "");
        assert_eq!(
            parse_pair_payload(bad).unwrap_err(),
            PairParseError::MissingField {
                field: "user".into()
            }
        );
        assert_eq!(
            parse_pair_payload("nope".into()).unwrap_err(),
            PairParseError::NotPairingCode
        );
    }

    #[tokio::test]
    async fn a_wiped_secret_cannot_be_submitted_and_a_bare_code_has_nothing_to_submit() {
        let offer = parse_pair_payload(CODE.into()).unwrap();
        offer.exchange.as_ref().unwrap().secret.wipe();
        assert_eq!(
            pair_submit_key(offer, "k".into(), "d".into()).await,
            Err(PairError::Wiped)
        );
        let bare = CODE.split("&pair=").next().unwrap();
        let offer = parse_pair_payload(bare.into()).unwrap();
        assert!(offer.exchange.is_none());
        assert_eq!(
            pair_submit_key(offer, "k".into(), "d".into()).await,
            Err(PairError::NoExchange)
        );
    }
}
