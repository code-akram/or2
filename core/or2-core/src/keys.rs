//! Client key material. Rust parses, generates and re-encodes keys; Kotlin stores them.
//!
//! The storage form is an unencrypted OpenSSH private key (`-----BEGIN OPENSSH PRIVATE KEY-----`,
//! LF line endings). Kotlin encrypts it with a Keystore key, decrypts it only to connect, and
//! hands the bytes to Rust. A passphrase is needed only once, at import.

use std::fmt;

use russh::keys::ssh_key::{self, Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};
use zeroize::Zeroizing;

const OPENSSH_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("not a readable OpenSSH private key")]
    Malformed,
    #[error("only OpenSSH-format private keys are supported; convert with ssh-keygen -p")]
    UnsupportedFormat,
    #[error("the private key is encrypted; a passphrase is required")]
    PassphraseRequired,
    #[error("the passphrase does not decrypt this private key")]
    WrongPassphrase,
    #[error("unsupported key algorithm {0}")]
    UnsupportedAlgorithm(String),
}

/// Public identity of a key: what the UI shows and what `authorized_keys` receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKeyInfo {
    /// SSH algorithm name, e.g. `ssh-ed25519`.
    pub algorithm: String,
    /// `authorized_keys` line: `<algorithm> <base64> [comment]`.
    pub openssh: String,
    /// OpenSSH SHA-256 fingerprint, e.g. `SHA256:…` (as printed by `ssh-keygen -l`).
    pub fingerprint: String,
    pub comment: String,
}

impl PublicKeyInfo {
    pub(crate) fn of(key: &PublicKey) -> Self {
        Self {
            algorithm: key.algorithm().as_str().to_owned(),
            openssh: key
                .to_openssh()
                .expect("encoding a parsed public key cannot fail"),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            comment: key.comment().to_string(),
        }
    }
}

/// A decrypted private key usable for public-key authentication. Zeroized on drop by `ssh-key`.
pub struct ClientKey(PrivateKey);

impl fmt::Debug for ClientKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientKey")
            .field("fingerprint", &self.public_key().fingerprint)
            .finish_non_exhaustive()
    }
}

impl ClientKey {
    pub fn generate_ed25519(comment: &str) -> Self {
        let mut key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
            .expect("Ed25519 generation is infallible with the OS-seeded RNG");
        key.set_comment(comment);
        Self(key)
    }

    /// The Ed25519 key of an RFC 8032 seed (the pairing bootstrap key derives one from the code).
    pub(crate) fn from_ed25519_seed(seed: &[u8; 32]) -> Self {
        Self(PrivateKey::from(
            ssh_key::private::Ed25519Keypair::from_seed(seed),
        ))
    }

    /// Parses an OpenSSH private key as the user supplied it, decrypting it if needed.
    /// The passphrase is ignored for unencrypted keys.
    pub fn import_openssh(pem: &[u8], passphrase: Option<&str>) -> Result<Self, KeyError> {
        let key = parse_openssh(pem)?;
        let key = if key.is_encrypted() {
            let passphrase = passphrase.ok_or(KeyError::PassphraseRequired)?;
            key.decrypt(passphrase).map_err(|error| match error {
                ssh_key::Error::Crypto => KeyError::WrongPassphrase,
                _ => KeyError::Malformed,
            })?
        } else {
            key
        };
        Self::checked(key)
    }

    /// Parses the storage form Kotlin hands over at connect time. It must be unencrypted.
    pub fn from_stored(pem: &[u8]) -> Result<Self, KeyError> {
        let key = parse_openssh(pem)?;
        if key.is_encrypted() {
            return Err(KeyError::PassphraseRequired);
        }
        Self::checked(key)
    }

    /// The storage form: unencrypted OpenSSH PEM with LF line endings.
    pub fn to_stored(&self) -> Zeroizing<Vec<u8>> {
        let pem = self
            .0
            .to_openssh(LineEnding::LF)
            .expect("encoding a decrypted private key cannot fail");
        Zeroizing::new(pem.as_bytes().to_vec())
    }

    pub fn public_key(&self) -> PublicKeyInfo {
        PublicKeyInfo::of(self.0.public_key())
    }

    /// For the SSH authentication step only.
    pub fn private_key(&self) -> &PrivateKey {
        &self.0
    }

    fn checked(key: PrivateKey) -> Result<Self, KeyError> {
        match key.algorithm() {
            Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => Ok(Self(key)),
            other => Err(KeyError::UnsupportedAlgorithm(other.as_str().to_owned())),
        }
    }
}

fn parse_openssh(pem: &[u8]) -> Result<PrivateKey, KeyError> {
    let text = std::str::from_utf8(pem).map_err(|_| KeyError::Malformed)?;
    if !text.trim_start().starts_with(OPENSSH_BEGIN) {
        return Err(if text.trim_start().starts_with("-----BEGIN ") {
            KeyError::UnsupportedFormat
        } else {
            KeyError::Malformed
        });
    }
    PrivateKey::from_openssh(text).map_err(|error| match error {
        ssh_key::Error::AlgorithmUnknown => KeyError::UnsupportedAlgorithm("unknown".into()),
        ssh_key::Error::AlgorithmUnsupported { algorithm } => {
            KeyError::UnsupportedAlgorithm(algorithm.as_str().to_owned())
        }
        _ => KeyError::Malformed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encrypted(key: &ClientKey, passphrase: &str) -> Vec<u8> {
        let encrypted = key.0.encrypt(&mut rand::rng(), passphrase).unwrap();
        encrypted
            .to_openssh(LineEnding::LF)
            .unwrap()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn generated_ed25519_round_trips_through_storage_form() {
        let key = ClientKey::generate_ed25519("phone");
        let info = key.public_key();
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert!(info.openssh.starts_with("ssh-ed25519 AAAA"));
        assert!(info.openssh.ends_with(" phone"));
        assert!(info.fingerprint.starts_with("SHA256:"));
        assert_eq!(info.comment, "phone");

        let stored = key.to_stored();
        assert!(stored.starts_with(OPENSSH_BEGIN.as_bytes()));
        assert!(!stored.contains(&b'\r'));
        let restored = ClientKey::from_stored(&stored).unwrap();
        assert_eq!(restored.public_key(), info);
    }

    #[test]
    fn two_generated_keys_differ() {
        let a = ClientKey::generate_ed25519("a").public_key().fingerprint;
        let b = ClientKey::generate_ed25519("a").public_key().fingerprint;
        assert_ne!(a, b);
    }

    #[test]
    fn import_decrypts_with_the_right_passphrase_only() {
        let key = ClientKey::generate_ed25519("laptop");
        let pem = encrypted(&key, "correct horse");
        assert_eq!(
            ClientKey::import_openssh(&pem, None).unwrap_err(),
            KeyError::PassphraseRequired
        );
        assert_eq!(
            ClientKey::import_openssh(&pem, Some("wrong horse")).unwrap_err(),
            KeyError::WrongPassphrase
        );
        let imported = ClientKey::import_openssh(&pem, Some("correct horse")).unwrap();
        assert_eq!(imported.public_key(), key.public_key());
        // The storage form is decrypted, so connect time needs no passphrase.
        assert!(ClientKey::from_stored(&imported.to_stored()).is_ok());
        // An encrypted key is never accepted as the storage form.
        assert_eq!(
            ClientKey::from_stored(&pem).unwrap_err(),
            KeyError::PassphraseRequired
        );
    }

    #[test]
    fn passphrase_is_ignored_for_unencrypted_keys() {
        let key = ClientKey::generate_ed25519("x");
        let imported = ClientKey::import_openssh(&key.to_stored(), Some("unused")).unwrap();
        assert_eq!(imported.public_key(), key.public_key());
    }

    #[test]
    fn rejects_garbage_other_pem_formats_and_truncation() {
        assert_eq!(
            ClientKey::import_openssh(b"hello", None).unwrap_err(),
            KeyError::Malformed
        );
        assert_eq!(
            ClientKey::import_openssh(&[0xff, 0xfe], None).unwrap_err(),
            KeyError::Malformed
        );
        let pkcs8 =
            b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIA==\n-----END PRIVATE KEY-----\n";
        assert_eq!(
            ClientKey::import_openssh(pkcs8, None).unwrap_err(),
            KeyError::UnsupportedFormat
        );
        let stored = ClientKey::generate_ed25519("t").to_stored();
        let truncated = &stored[..stored.len() / 2];
        assert_eq!(
            ClientKey::import_openssh(truncated, None).unwrap_err(),
            KeyError::Malformed
        );
    }

    #[test]
    fn debug_output_never_contains_private_material() {
        let key = ClientKey::generate_ed25519("secret-comment-is-fine");
        let debug = format!("{key:?}");
        assert!(debug.contains("SHA256:"));
        assert!(!debug.contains("OPENSSH"));
        let stored = key.to_stored();
        let body = std::str::from_utf8(&stored)
            .unwrap()
            .lines()
            .nth(1)
            .unwrap();
        assert!(!debug.contains(body));
    }
}
