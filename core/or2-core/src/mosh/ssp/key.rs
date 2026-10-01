// SPDX-License-Identifier: GPL-3.0-or-later
// Vendored from mosh-rs (https://github.com/wilsonglasser/mosh-rs), commit
// 90b37125f5e4a598be91dec37d23921b6865276e, src/key.rs. Upstream: GPL-3.0-or-later, copyright
// Wilson Glasser; the protocol logic follows mosh (Keith Winstein and contributors, GPL-3.0-or-later).
// See THIRD_PARTY_NOTICES.md. or2 changes: module paths; key bytes and printable forms are held in `Zeroizing`.
//! The 128-bit session key in mosh-server's printable form.
//!
//! `mosh-server new` prints `MOSH CONNECT <port> <key>` where `<key>`
//! is the 16-byte key in standard base64 with the two `=` padding
//! chars stripped: always exactly 22 characters. Decoding is strict
//! (crypto.cc `Base64Key`): standard alphabet only, and the key must
//! round-trip back to the same printable string, which rejects any
//! encoding whose final character carries nonzero low bits (16 bytes
//! occupy 128 of the 132 bits the 22 sixbit chars can express).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

use super::error::{MoshError, Result};

/// A validated 128-bit mosh session key.
#[derive(Clone)]
pub struct Base64Key(Zeroizing<[u8; 16]>);

impl Base64Key {
    /// Parse the 22-character printable form (the `MOSH_KEY` value).
    pub fn from_printable(printable: &str) -> Result<Self> {
        if printable.len() != 22 {
            return Err(MoshError::KeyLength);
        }
        let padded = Zeroizing::new(format!("{printable}=="));
        // The STANDARD engine already refuses non-alphabet characters
        // and nonzero trailing bits, but the canonicality proof mosh
        // relies on is the re-encode comparison below, so both run.
        let raw = Zeroizing::new(
            STANDARD
                .decode(padded.as_bytes())
                .map_err(|_| MoshError::KeyDecode)?,
        );
        let bytes: [u8; 16] = raw
            .as_slice()
            .try_into()
            .map_err(|_| MoshError::KeyDecode)?;
        let key = Self(Zeroizing::new(bytes));
        if key.printable().as_str() != printable {
            return Err(MoshError::KeyNotCanonical);
        }
        Ok(key)
    }

    /// The 22-character printable form (padding stripped).
    pub fn printable(&self) -> Zeroizing<String> {
        let mut s = Zeroizing::new(STANDARD.encode(*self.0));
        s.truncate(22);
        s
    }

    /// The raw 128 bits, for the cipher.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

// Never print key material through Debug.
impl std::fmt::Debug for Base64Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Base64Key(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_canonical_key() {
        let key = Base64Key::from_printable("zr0jtuYVKJnfJHP/XOZs7A").expect("canonical");
        assert_eq!(key.printable().as_str(), "zr0jtuYVKJnfJHP/XOZs7A");
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(matches!(
            Base64Key::from_printable("short"),
            Err(MoshError::KeyLength)
        ));
        assert!(matches!(
            Base64Key::from_printable("zr0jtuYVKJnfJHP/XOZs7Ax"),
            Err(MoshError::KeyLength)
        ));
    }

    #[test]
    fn rejects_non_alphabet() {
        assert!(Base64Key::from_printable("zr0jtuYVKJnfJHP/XOZs7-").is_err());
        assert!(Base64Key::from_printable("zr0jtuYVKJnfJHP XOZs7A").is_err());
    }

    #[test]
    fn rejects_non_canonical_tail() {
        // The 22 sixbit chars express 132 bits but a key is 128, so the
        // final char's low 4 bits must be zero: only A/Q/g/w (0x00, 0x10,
        // 0x20, 0x30) qualify. mosh enforces this by re-encoding; the
        // base64 engine also catches it as nonzero trailing bits.
        // 'B' = 0b000001 and 'E' = 0b000100 both carry low bits: rejected.
        assert!(Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAB").is_err());
        assert!(Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAE").is_err());
        // 'Q' = 0b010000: low 4 bits zero, canonical, accepted.
        assert!(Base64Key::from_printable("AAAAAAAAAAAAAAAAAAAAAQ").is_ok());
    }
}
