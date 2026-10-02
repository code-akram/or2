//! Easy pair for Kotlin (FFI API 13): the pairing code the phone shows, parsing the code the host
//! prints, and enrolling the phone's key over the host's own sshd. See docs/contracts.md, "Easy
//! pair".
//!
//! The pairing code never leaves Rust except as the text on the screen (`PairCode.display`): it is
//! an opaque object that Rust zeroizes when the last reference goes, and its `Debug` is redacted.

use std::fmt;
use std::sync::Arc;

use or2_core::pair as core;
use or2_core::transport::{DirectTcp, Endpoint};
use or2_core::trust::HostKey;

use crate::host::HostAddress;
use crate::keys::PublicKeyInfo;

/// The pairing code `K` (`7KQ4-M2XD-9PTM`): 11 random characters of Crockford base32 without `Z`
/// (about 54.5 bits) and a check character, shown on the Easy pair screen and typed into
/// `or2-pair` on the host. Opaque to Kotlin.
#[derive(uniffi::Object)]
pub struct PairCode {
    code: core::PairCode,
}

impl fmt::Debug for PairCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairCode(<redacted>)")
    }
}

#[uniffi::export]
impl PairCode {
    /// The code as shown, three groups of four: `7KQ4-M2XD-9PTM`.
    pub fn display(&self) -> String {
        self.code.display()
    }
}

/// A new pairing code from the operating system's random source. Draw one each time the Easy pair
/// screen opens and after every pairing that reached the host, successful or not.
#[uniffi::export]
pub fn pair_new_code() -> Arc<PairCode> {
    Arc::new(PairCode {
        code: core::PairCode::generate(),
    })
}

/// A validated pairing code from the host (the text of its QR).
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairOffer {
    /// The host's own name for itself, to pre-fill the host's label.
    pub name: String,
    pub username: String,
    /// The SSH port; every address in `addresses` carries it.
    pub port: u16,
    /// Where SSH reaches the host, in preference order.
    pub addresses: Vec<HostAddress>,
    /// The host's key: pinned for the pairing, trusted afterwards (`fingerprint` is for display).
    pub host_key: PublicKeyInfo,
    /// This run's pairing id. `None` for a code made with `--manual`: there is nothing to enrol
    /// with, so the phone shows its public key for the user to install by hand.
    pub pairing_id: Option<String>,
}

impl From<core::PairOffer> for PairOffer {
    fn from(offer: core::PairOffer) -> Self {
        Self {
            name: offer.name,
            username: offer.username,
            port: offer.port,
            addresses: offer
                .addresses
                .iter()
                .map(|endpoint| HostAddress {
                    host: endpoint.host().to_owned(),
                    port: endpoint.port(),
                })
                .collect(),
            host_key: offer.host_key.info().into(),
            pairing_id: offer.pairing_id,
        }
    }
}

impl PairOffer {
    /// The core's offer again; `None` if this is not what `parse_pair_payload` made.
    fn to_core(&self) -> Option<core::PairOffer> {
        let addresses = self
            .addresses
            .iter()
            .map(|a| Endpoint::new(&a.host, a.port))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some(core::PairOffer {
            name: self.name.clone(),
            username: self.username.clone(),
            port: self.port,
            addresses,
            host_key: HostKey::from_openssh(&self.host_key.openssh).ok()?,
            pairing_id: self.pairing_id.clone(),
        })
    }
}

/// Why a pairing code was refused. `field` is the payload's own field name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PairParseError {
    #[error("this is not an or2 pairing code")]
    NotPairingCode,
    /// Version 1 is from an older `or2-pair` (update it on the host); a higher one needs a newer app.
    #[error("this pairing code is version {version}, which this app does not read")]
    UnsupportedVersion { version: u32 },
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
            E::UnsupportedVersion { version } => Self::UnsupportedVersion { version },
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

/// What the host reported after it installed the phone's key.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairResult {
    /// The account the key was added to.
    pub username: String,
    /// `SHA256:…` of the key the host installed.
    pub fingerprint: String,
}

/// Why the pairing did not complete. Nothing is saved on the host unless the pairing succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PairError {
    #[error("this pairing code has no pairing id (it was made with --manual)")]
    NoPairingId,
    #[error("the offer is not one `parse_pair_payload` made")]
    InvalidOffer,
    #[error("the phone key is not an OpenSSH public key this app can authorize")]
    InvalidKey,
    #[error("the device label must be 1 to 64 characters without control characters")]
    InvalidDevice,
    #[error("the host cannot be reached on its addresses")]
    Unreachable,
    #[error("the host did not answer in time")]
    TimedOut,
    #[error("the host presented a different key than the pairing code")]
    HostKeyMismatch,
    #[error("the host did not accept this phone's code")]
    BootstrapRefused,
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
    /// The host said `request`, or a reason this app does not know.
    #[error("the host refused")]
    Refused,
}

impl From<core::PairError> for PairError {
    fn from(error: core::PairError) -> Self {
        use core::PairError as E;
        match error {
            E::NoPairingId => Self::NoPairingId,
            E::InvalidOffer => Self::InvalidOffer,
            E::InvalidKey => Self::InvalidKey,
            E::InvalidDevice => Self::InvalidDevice,
            E::Unreachable => Self::Unreachable,
            E::TimedOut => Self::TimedOut,
            E::HostKeyMismatch => Self::HostKeyMismatch,
            E::BootstrapRefused => Self::BootstrapRefused,
            E::NotOr2Pair => Self::NotOr2Pair,
            E::Protocol => Self::Protocol,
            E::ConnectionLost => Self::ConnectionLost,
            E::Expired => Self::Expired,
            E::Gone => Self::Gone,
            E::KeyNotAccepted => Self::KeyNotAccepted,
            E::HostFailed => Self::HostFailed,
            E::Refused => Self::Refused,
        }
    }
}

impl From<core::PairResult> for PairResult {
    fn from(result: core::PairResult) -> Self {
        Self {
            username: result.username,
            fingerprint: result.fingerprint,
        }
    }
}

/// Parses and validates a pairing code (the text of the QR, or what the user pasted).
#[uniffi::export]
pub fn parse_pair_payload(text: String) -> Result<PairOffer, PairParseError> {
    Ok(core::PairOffer::parse(&text)?.into())
}

/// Enrols `public_key_line` (an OpenSSH public key line; its comment is not sent) with the host
/// that made `offer`, using `code`, the pairing code shown on the phone and typed at the host. One
/// SSH connection to the host's own sshd on the offer's addresses (raced), with its host key pinned
/// from the offer; it logs in with a throwaway key derived from `code`, which `or2-pair` on the
/// host authorized for this run. Every step takes at most 10 s. `device_label` is what the host
/// shows (and puts in the key's comment); 1 to 64 characters.
///
/// Cancelling the coroutine closes the channel and the connection. The transport is the app's
/// direct TCP (the phone must reach the host's SSH port itself).
#[uniffi::export(async_runtime = "tokio")]
pub async fn pair_enroll(
    offer: PairOffer,
    code: Arc<PairCode>,
    public_key_line: String,
    device_label: String,
) -> Result<PairResult, PairError> {
    let offer = offer.to_core().ok_or(PairError::InvalidOffer)?;
    core::pair_enroll(
        &Arc::new(DirectTcp),
        &offer,
        &code.code,
        &public_key_line,
        &device_label,
        core::PairTiming::default(),
    )
    .await
    .map(Into::into)
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "or2-pair:2?name=Work%20Mac&user=alice&port=22&a=192.0.2.20\
        &hk=ssh-ed25519%20AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83%2BXgmwnHmYtMQRjLeaZ2U7\
        &id=abcdefghijklm";

    #[test]
    fn a_new_code_shows_three_groups_and_hides_itself_from_debug() {
        let code = pair_new_code();
        let shown = code.display();
        let groups: Vec<_> = shown.split('-').collect();
        assert_eq!(groups.len(), 3, "{shown}");
        assert!(groups.iter().all(|group| group.len() == 4));
        let debug = format!("{code:?}");
        assert!(!debug.contains(groups[0]), "{debug}");
        assert!(debug.contains("redacted"), "{debug}");
        assert_ne!(shown, pair_new_code().display());
    }

    #[test]
    fn parsing_maps_the_offer() {
        let offer = parse_pair_payload(CODE.into()).unwrap();
        assert_eq!(offer.name, "Work Mac");
        assert_eq!(offer.username, "alice");
        assert_eq!(offer.addresses[0].host, "192.0.2.20");
        assert_eq!(offer.addresses[0].port, 22);
        assert_eq!(
            offer.host_key.fingerprint,
            "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI"
        );
        assert_eq!(offer.pairing_id.as_deref(), Some("abcdefghijklm"));
        assert!(offer.to_core().is_some());
        let manual = parse_pair_payload(CODE.split("&id=").next().unwrap().into()).unwrap();
        assert_eq!(manual.pairing_id, None);
    }

    #[test]
    fn parse_errors_carry_the_field_name_and_the_version() {
        assert_eq!(
            parse_pair_payload(CODE.replace("&user=alice", "")).unwrap_err(),
            PairParseError::MissingField {
                field: "user".into()
            }
        );
        assert_eq!(
            parse_pair_payload("nope".into()).unwrap_err(),
            PairParseError::NotPairingCode
        );
        assert_eq!(
            parse_pair_payload(CODE.replace("or2-pair:2", "or2-pair:1")).unwrap_err(),
            PairParseError::UnsupportedVersion { version: 1 }
        );
        assert_eq!(
            parse_pair_payload(CODE.replace("or2-pair:2", "or2-pair:3")).unwrap_err(),
            PairParseError::UnsupportedVersion { version: 3 }
        );
    }

    #[test]
    fn every_core_error_has_its_own_ffi_error() {
        use self::core::PairError as E;
        let all = [
            (E::NoPairingId, PairError::NoPairingId),
            (E::InvalidOffer, PairError::InvalidOffer),
            (E::InvalidKey, PairError::InvalidKey),
            (E::InvalidDevice, PairError::InvalidDevice),
            (E::Unreachable, PairError::Unreachable),
            (E::TimedOut, PairError::TimedOut),
            (E::HostKeyMismatch, PairError::HostKeyMismatch),
            (E::BootstrapRefused, PairError::BootstrapRefused),
            (E::NotOr2Pair, PairError::NotOr2Pair),
            (E::Protocol, PairError::Protocol),
            (E::ConnectionLost, PairError::ConnectionLost),
            (E::Expired, PairError::Expired),
            (E::Gone, PairError::Gone),
            (E::KeyNotAccepted, PairError::KeyNotAccepted),
            (E::HostFailed, PairError::HostFailed),
            (E::Refused, PairError::Refused),
        ];
        for (core, ffi) in all {
            assert_eq!(PairError::from(core), ffi);
        }
    }

    #[tokio::test]
    async fn a_manual_code_has_nothing_to_enrol_with_and_a_forged_offer_is_refused() {
        let manual = parse_pair_payload(CODE.split("&id=").next().unwrap().into()).unwrap();
        assert_eq!(
            pair_enroll(manual, pair_new_code(), "k".into(), "d".into()).await,
            Err(PairError::NoPairingId)
        );
        let mut forged = parse_pair_payload(CODE.into()).unwrap();
        forged.host_key.openssh = "nonsense".into();
        assert_eq!(
            pair_enroll(forged, pair_new_code(), "k".into(), "d".into()).await,
            Err(PairError::InvalidOffer)
        );
    }
}
