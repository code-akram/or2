//! Client key generation and import. Kotlin stores `ClientKeyMaterial.private_key` encrypted
//! with a Keystore key and passes it back unchanged in `HostConnectRequest.private_key`.

use std::fmt;

use or2_core::keys as core;
use zeroize::Zeroizing;

#[derive(uniffi::Record)]
pub struct ClientKeyMaterial {
    /// Unencrypted OpenSSH private key (the storage form). Secret: encrypt at rest, zero after use.
    pub private_key: Vec<u8>,
    pub public_key: PublicKeyInfo,
}

impl fmt::Debug for ClientKeyMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientKeyMaterial")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PublicKeyInfo {
    pub algorithm: String,
    /// `authorized_keys` line.
    pub openssh: String,
    /// `SHA256:…`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
    pub comment: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KeyError {
    #[error("not a readable OpenSSH private key")]
    Malformed,
    #[error("only OpenSSH-format private keys are supported")]
    UnsupportedFormat,
    #[error("the private key is encrypted; a passphrase is required")]
    PassphraseRequired,
    #[error("the passphrase does not decrypt this private key")]
    WrongPassphrase,
    #[error("unsupported key algorithm {algorithm}")]
    UnsupportedAlgorithm { algorithm: String },
}

impl From<core::KeyError> for KeyError {
    fn from(error: core::KeyError) -> Self {
        match error {
            core::KeyError::Malformed => Self::Malformed,
            core::KeyError::UnsupportedFormat => Self::UnsupportedFormat,
            core::KeyError::PassphraseRequired => Self::PassphraseRequired,
            core::KeyError::WrongPassphrase => Self::WrongPassphrase,
            core::KeyError::UnsupportedAlgorithm(algorithm) => {
                Self::UnsupportedAlgorithm { algorithm }
            }
        }
    }
}

impl From<core::PublicKeyInfo> for PublicKeyInfo {
    fn from(info: core::PublicKeyInfo) -> Self {
        Self {
            algorithm: info.algorithm,
            openssh: info.openssh,
            fingerprint: info.fingerprint,
            comment: info.comment,
        }
    }
}

fn material(key: &core::ClientKey) -> ClientKeyMaterial {
    ClientKeyMaterial {
        private_key: key.to_stored().to_vec(),
        public_key: key.public_key().into(),
    }
}

/// A new Ed25519 key. `comment` is the conventional `user@device` label.
#[uniffi::export]
pub fn generate_ed25519_key(comment: String) -> ClientKeyMaterial {
    material(&core::ClientKey::generate_ed25519(&comment))
}

/// Imports an OpenSSH private key, decrypting it with `passphrase` if it is encrypted, and
/// returns the unencrypted storage form. Ed25519, ECDSA and RSA keys are accepted.
#[uniffi::export]
pub fn import_private_key(
    private_key: Vec<u8>,
    passphrase: Option<String>,
) -> Result<ClientKeyMaterial, KeyError> {
    let private_key = Zeroizing::new(private_key);
    let passphrase = passphrase.map(Zeroizing::new);
    let key =
        core::ClientKey::import_openssh(&private_key, passphrase.as_deref().map(String::as_str))?;
    Ok(material(&key))
}
