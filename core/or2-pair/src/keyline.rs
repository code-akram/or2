//! OpenSSH public key lines: strict parsing and the SHA-256 fingerprint.
//!
//! The CLI never writes a line it received. It parses the algorithm and key data, checks that
//! the key data really is a key of that algorithm, and rebuilds `<algorithm> <base64>` itself,
//! so nothing a phone sends (a comment, a newline, options) can reach `authorized_keys`.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest, Sha256};

/// The key types accepted for the host key and for the phone's key.
pub const ALGORITHMS: [&str; 5] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("not an OpenSSH public key line")]
    Malformed,
    #[error("unsupported key type")]
    Unsupported,
}

/// A validated public key: algorithm and key data, no comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLine {
    algorithm: String,
    blob: Vec<u8>,
}

impl KeyLine {
    /// Parses `<algorithm> <base64> [comment]` (the comment is dropped).
    pub fn parse(line: &str) -> Result<Self, KeyError> {
        let mut tokens = line.split_whitespace();
        let algorithm = tokens.next().ok_or(KeyError::Malformed)?;
        let encoded = tokens.next().ok_or(KeyError::Malformed)?;
        if !ALGORITHMS.contains(&algorithm) {
            return Err(KeyError::Unsupported);
        }
        let blob = STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| KeyError::Malformed)?;
        // Only the canonical encoding: the same key never has two spellings, so duplicate
        // detection by text is exact.
        if STANDARD.encode(&blob) != encoded {
            return Err(KeyError::Malformed);
        }
        check_blob(algorithm, &blob)?;
        Ok(Self {
            algorithm: algorithm.to_owned(),
            blob,
        })
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn base64(&self) -> String {
        STANDARD.encode(&self.blob)
    }

    /// The decoded key data.
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// `<algorithm> <base64>`.
    pub fn openssh(&self) -> String {
        format!("{} {}", self.algorithm, self.base64())
    }

    /// `SHA256:…` as `ssh-keygen -l` prints it.
    pub fn fingerprint(&self) -> String {
        format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(&self.blob))
        )
    }
}

/// Reads one SSH wire string (`u32` length, bytes) and returns it with the rest.
fn string(data: &[u8]) -> Result<(&[u8], &[u8]), KeyError> {
    let (length, rest) = data.split_first_chunk::<4>().ok_or(KeyError::Malformed)?;
    let length = u32::from_be_bytes(*length) as usize;
    if rest.len() < length {
        return Err(KeyError::Malformed);
    }
    Ok(rest.split_at(length))
}

/// The key data starts with its own algorithm name, then the fields of that key type, and
/// nothing after them.
fn check_blob(algorithm: &str, blob: &[u8]) -> Result<(), KeyError> {
    let (name, mut rest) = string(blob)?;
    if name != algorithm.as_bytes() {
        return Err(KeyError::Malformed);
    }
    let fields = match algorithm {
        "ssh-ed25519" => 1,
        "ssh-rsa" | "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => 2,
        _ => return Err(KeyError::Unsupported),
    };
    let mut first = None;
    for index in 0..fields {
        let (field, next) = string(rest)?;
        if field.is_empty() {
            return Err(KeyError::Malformed);
        }
        if index == 0 {
            first = Some(field);
        }
        rest = next;
    }
    if !rest.is_empty() {
        return Err(KeyError::Malformed);
    }
    let first = first.ok_or(KeyError::Malformed)?;
    let valid = match algorithm {
        "ssh-ed25519" => first.len() == 32,
        // The public exponent is small; the modulus follows.
        "ssh-rsa" => first.len() <= 8,
        // The curve name is the algorithm's own suffix.
        other => other.strip_prefix("ecdsa-sha2-").map(str::as_bytes) == Some(first),
    };
    if valid {
        Ok(())
    } else {
        Err(KeyError::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    pub(crate) const ED25519_FINGERPRINT: &str =
        "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI";
    pub(crate) const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";

    #[test]
    fn parses_and_fingerprints_like_ssh_keygen() {
        let key = KeyLine::parse(ED25519).unwrap();
        assert_eq!(key.algorithm(), "ssh-ed25519");
        assert_eq!(key.fingerprint(), ED25519_FINGERPRINT);
        assert_eq!(key.openssh(), ED25519);
        assert!(KeyLine::parse(ECDSA).is_ok());
    }

    #[test]
    fn a_comment_is_dropped_and_options_are_not_a_key() {
        let key = KeyLine::parse(&format!("{ED25519} root@box\n")).unwrap();
        assert_eq!(key.openssh(), ED25519);
        assert_eq!(
            KeyLine::parse(&format!("no-pty {ED25519}")).unwrap_err(),
            KeyError::Unsupported
        );
    }

    #[test]
    fn refuses_anything_that_is_not_one_plain_key() {
        for line in [
            "",
            "ssh-ed25519",
            "ssh-ed25519 AAAA",
            "ssh-ed25519 !!!!",
            // Wrong algorithm name inside the key data.
            "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7",
            // Key data with a byte missing, and with a byte added.
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7AA==",
            // Not canonical base64 (trailing bits).
            "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFB=",
        ] {
            assert!(KeyLine::parse(line).is_err(), "{line:?}");
        }
        for line in [
            "ssh-dss AAAAB3NzaC1kc3M=",
            "sk-ssh-ed25519@openssh.com AAAA",
            "ssh-ed25519-cert-v01@openssh.com AAAA",
        ] {
            assert_eq!(
                KeyLine::parse(line).unwrap_err(),
                KeyError::Unsupported,
                "{line}"
            );
        }
    }

    #[test]
    fn a_newline_inside_the_line_cannot_smuggle_a_second_line() {
        let key = KeyLine::parse(&format!("{ED25519}\nssh-rsa AAAAB3NzaC1yc2E=")).unwrap();
        assert_eq!(key.openssh(), ED25519);
        assert!(!key.openssh().contains('\n'));
    }
}
