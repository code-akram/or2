//! Host-key trust decisions. Kotlin persists trusted keys; Rust only compares them.
//!
//! Trust is keyed by the public key itself (algorithm and key data), never by its fingerprint
//! string or comment. The fingerprint is derived for display.

use russh::keys::ssh_key::{HashAlg, PublicKey};

use crate::keys::PublicKeyInfo;

/// A server's plain public host key. Certificates are not host keys in M1.
#[derive(Debug, Clone)]
pub struct HostKey(PublicKey);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an OpenSSH public key line")]
pub struct HostKeyParseError;

impl HostKey {
    pub fn from_public_key(key: PublicKey) -> Self {
        let mut key = key;
        key.set_comment("");
        Self(key)
    }

    /// Parses `<algorithm> <base64> [comment]`, as `known_hosts`/`authorized_keys` store it.
    pub fn from_openssh(line: &str) -> Result<Self, HostKeyParseError> {
        PublicKey::from_openssh(line.trim())
            .map(Self::from_public_key)
            .map_err(|_| HostKeyParseError)
    }

    pub fn fingerprint(&self) -> String {
        self.0.fingerprint(HashAlg::Sha256).to_string()
    }

    pub fn info(&self) -> PublicKeyInfo {
        PublicKeyInfo::of(&self.0)
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.0
    }
}

impl PartialEq for HostKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.key_data() == other.0.key_data()
    }
}

impl Eq for HostKey {}

/// Outcome of comparing a presented host key with the keys Kotlin trusts for that host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyVerdict {
    /// The key is one of the trusted keys: continue without asking.
    Trusted,
    /// Nothing is trusted for this host yet: ask the user to confirm the fingerprint.
    FirstUse,
    /// Keys are trusted but this is not one of them: stop and ask; never continue silently.
    Changed,
}

pub fn verify(presented: &HostKey, trusted: &[HostKey]) -> HostKeyVerdict {
    if trusted.contains(presented) {
        HostKeyVerdict::Trusted
    } else if trusted.is_empty() {
        HostKeyVerdict::FirstUse
    } else {
        HostKeyVerdict::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::ClientKey;

    fn host_key() -> HostKey {
        HostKey::from_openssh(&ClientKey::generate_ed25519("h").public_key().openssh).unwrap()
    }

    #[test]
    fn verdicts_cover_first_use_trusted_and_changed() {
        let presented = host_key();
        let other = host_key();
        assert_eq!(verify(&presented, &[]), HostKeyVerdict::FirstUse);
        assert_eq!(
            verify(&presented, &[other.clone(), presented.clone()]),
            HostKeyVerdict::Trusted
        );
        assert_eq!(verify(&presented, &[other]), HostKeyVerdict::Changed);
    }

    #[test]
    fn equality_ignores_comment_and_whitespace_but_not_key_data() {
        let line = ClientKey::generate_ed25519("one").public_key().openssh;
        let body = line.rsplit_once(' ').unwrap().0;
        let a = HostKey::from_openssh(&line).unwrap();
        let b = HostKey::from_openssh(&format!("  {body} another-comment\n")).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.info().comment, "");
        assert_ne!(a, host_key());
    }

    #[test]
    fn fingerprint_is_openssh_sha256_form() {
        let key = ClientKey::generate_ed25519("f").public_key();
        let host = HostKey::from_openssh(&key.openssh).unwrap();
        assert_eq!(host.fingerprint(), key.fingerprint);
        assert!(host.fingerprint().starts_with("SHA256:"));
        assert!(!host.fingerprint().ends_with('='));
    }

    #[test]
    fn rejects_non_keys() {
        for line in ["", "ssh-ed25519", "ssh-ed25519 !!!", "hello world"] {
            assert_eq!(
                HostKey::from_openssh(line),
                Err(HostKeyParseError),
                "{line:?}"
            );
        }
    }
}
