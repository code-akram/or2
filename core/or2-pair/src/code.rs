//! The pairing code `K`: 12 characters of Crockford base32 without `Z`, shown on the phone as
//! `7KQ4-M2XD-9PTM` and typed at the host.
//!
//! 11 random data characters, each one of the 31 symbols `0-9` and `A-Y` without `I`, `L`, `O`,
//! `U` (about 54.5 bits), and a check character `c = (sum of i * v_i for i = 1..11) mod 31`, `v_i`
//! being the value of the i-th character in [`ALPHABET`] (0 to 30). Every value is below the
//! prime 31, so weights 1 to 11 catch every single wrong character and every swap of two
//! neighbours, with no exception; `c` is 0 to 30, so `Z` never appears in a code, and a typed `Z`
//! is refused as a character codes never use. The phone implements the same rules in
//! `or2_core::pair`; the two sets of test vectors match, and the contract (`docs/contracts.md`,
//! "The pairing code") is the one source.
//!
//! Typed input is read leniently: case-insensitive, hyphens and white space ignored, `I` and `L`
//! read as `1`, `O` as `0`. The code is zeroized on drop and never printed, logged or saved.

use zeroize::{Zeroize, Zeroizing};

/// Crockford's alphabet without `Z` (no `I`, `L`, `O`, `U` either): the 31 symbols of a code,
/// in value order.
pub const ALPHABET: &[u8; 31] = b"0123456789ABCDEFGHJKMNPQRSTVWXY";

/// Random characters in a code.
pub const DATA_LEN: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("That code has the wrong length: it has {0} characters, not 12")]
    Length(usize),
    #[error("That code has a character that codes never use (they use 0-9 and A-Y, never U or Z)")]
    Character,
    #[error("That code has a typo")]
    Check,
}

/// The 11 data characters, as ASCII uppercase (the HKDF input of the bootstrap key).
#[derive(Clone, PartialEq, Eq)]
pub struct PairCode {
    data: [u8; DATA_LEN],
}

impl Drop for PairCode {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

impl std::fmt::Debug for PairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairCode(..)")
    }
}

/// The value of a character of the alphabet.
fn value(c: u8) -> Option<u32> {
    ALPHABET.iter().position(|a| *a == c).map(|p| p as u32)
}

/// A typed character as the alphabet's: uppercase, `I` and `L` as `1`, `O` as `0`.
fn normal(c: char) -> Option<u8> {
    let c = u8::try_from(c.to_ascii_uppercase()).ok()?;
    let c = match c {
        b'I' | b'L' => b'1',
        b'O' => b'0',
        other => other,
    };
    value(c).map(|_| c)
}

/// The check character of 11 data characters (all in the alphabet).
fn check_of(data: &[u8; DATA_LEN]) -> u8 {
    let sum: u32 = data
        .iter()
        .enumerate()
        .map(|(i, c)| (i as u32 + 1) * value(*c).unwrap_or(0))
        .sum();
    ALPHABET[(sum % 31) as usize]
}

impl PairCode {
    /// Reads what a person typed.
    pub fn parse(input: &str) -> Result<Self, CodeError> {
        let mut chars: Vec<char> = input
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        let result = Self::from_chars(&chars);
        chars.zeroize();
        result
    }

    fn from_chars(chars: &[char]) -> Result<Self, CodeError> {
        if chars.len() != DATA_LEN + 1 {
            return Err(CodeError::Length(chars.len()));
        }
        // Wiped on every way out, a refused character's early return included.
        let mut typed = Zeroizing::new([0u8; DATA_LEN + 1]);
        for (slot, c) in typed.iter_mut().zip(chars) {
            *slot = normal(*c).ok_or(CodeError::Character)?;
        }
        let mut code = Self {
            data: [0u8; DATA_LEN],
        };
        code.data.copy_from_slice(&typed[..DATA_LEN]);
        if check_of(&code.data) == typed[DATA_LEN] {
            Ok(code)
        } else {
            // `code` is zeroized as it drops.
            Err(CodeError::Check)
        }
    }

    /// A new random code. The phone draws its own in production; this is for tests and for
    /// tools that play the phone. Uniform over the 31 symbols: a byte is used only below 248
    /// (8 · 31), as its value modulo 31.
    pub fn generate(random: &dyn Fn(&mut [u8])) -> Self {
        let mut code = Self {
            data: [0u8; DATA_LEN],
        };
        let mut bytes = Zeroizing::new([0u8; DATA_LEN]);
        let mut filled = 0;
        while filled < DATA_LEN {
            let wanted = &mut bytes[..DATA_LEN - filled];
            random(wanted);
            for byte in wanted.iter().filter(|byte| **byte < 248) {
                code.data[filled] = ALPHABET[usize::from(*byte % 31)];
                filled += 1;
            }
        }
        code
    }

    /// The 11 data characters as ASCII uppercase.
    pub fn data(&self) -> &[u8; DATA_LEN] {
        &self.data
    }

    /// `XXXX-XXXX-XXXX`: what the phone shows.
    pub fn display(&self) -> String {
        let mut all = [0u8; DATA_LEN + 1];
        all[..DATA_LEN].copy_from_slice(&self.data);
        all[DATA_LEN] = check_of(&self.data);
        let text = format!(
            "{}-{}-{}",
            String::from_utf8_lossy(&all[..4]),
            String::from_utf8_lossy(&all[4..8]),
            String::from_utf8_lossy(&all[8..]),
        );
        all.zeroize();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The vectors the phone's `or2_core::pair` tests hold too (contract: "The pairing code").
    // The check character is `(sum i * v_i, i = 1..11) mod 31`, for example for 7KQ4M2XD9PT:
    // 7 + 2*19 + 3*23 + 4*4 + 5*20 + 6*2 + 7*29 + 8*13 + 9*9 + 10*22 + 11*26 = 1136 = 36*31 + 20,
    // and value 20 is `M`; for YYYYYYYYYYY: 30 * 66 = 1980 = 63*31 + 27, value 27 is `V`.
    const VECTORS: [(&str, char); 5] = [
        ("7KQ4M2XD9PT", 'M'),
        ("00000000000", '0'),
        ("YYYYYYYYYYY", 'V'),
        ("11111111111", '4'),
        ("0123456789A", '6'),
    ];

    #[test]
    fn the_check_character_follows_the_formula() {
        for (data, check) in VECTORS {
            let code = PairCode::parse(&format!("{data}{check}")).unwrap();
            assert_eq!(code.data(), data.as_bytes());
            assert_eq!(
                code.display(),
                format!("{}-{}-{}{check}", &data[..4], &data[4..8], &data[8..])
            );
        }
    }

    #[test]
    fn every_single_wrong_character_and_every_neighbour_swap_fails_the_check() {
        // With values 0 to 30 below the prime 31 there is no exception (there was one while `Z`,
        // value 31, could be a data character: `0` and `Z` were the same modulo 31).
        let good = "7KQ4M2XD9PTM";
        assert!(PairCode::parse(good).is_ok());
        let chars: Vec<char> = good.chars().collect();
        for i in 0..chars.len() {
            for a in ALPHABET {
                let mut typed = chars.clone();
                typed[i] = *a as char;
                if typed == chars {
                    continue;
                }
                let text: String = typed.iter().collect();
                assert_eq!(
                    PairCode::parse(&text).unwrap_err(),
                    CodeError::Check,
                    "{text}"
                );
            }
        }
        for i in 0..chars.len() - 1 {
            if chars[i] == chars[i + 1] {
                continue;
            }
            let mut typed = chars.clone();
            typed.swap(i, i + 1);
            let text: String = typed.iter().collect();
            assert_eq!(
                PairCode::parse(&text).unwrap_err(),
                CodeError::Check,
                "{text}"
            );
        }
    }

    #[test]
    fn z_is_never_part_of_a_code_and_a_typed_z_is_refused() {
        assert!(!ALPHABET.contains(&b'Z'));
        for typed in [
            "ZZZZZZZZZZZ0",
            "0ZZZZZZZZZZ0",
            "7KQ4M2XD9PTZ",
            "z0000000000-0",
        ] {
            assert_eq!(
                PairCode::parse(typed).unwrap_err(),
                CodeError::Character,
                "{typed}"
            );
        }
        // A check character is never Z: every sum modulo 31 is a value of the alphabet.
        for sum in 0..31 * 66 {
            assert_ne!(ALPHABET[sum % 31], b'Z');
        }
    }

    #[test]
    fn typed_input_is_read_leniently() {
        let good = PairCode::parse("7KQ4M2XD9PTM").unwrap();
        for typed in [
            "7KQ4-M2XD-9PTM",
            "7kq4-m2xd-9ptm",
            " 7KQ4 M2XD 9PTM \n",
            "7KQ4M2XD9PTM\r\n",
            "7-K-Q-4-M-2-X-D-9-P-T-M",
        ] {
            assert_eq!(PairCode::parse(typed).unwrap(), good, "{typed:?}");
        }
        // I and L are 1, O is 0.
        let ones = PairCode::parse("11111111111 4").unwrap();
        assert_eq!(PairCode::parse("ilIL1liL1l1-4").unwrap(), ones);
        assert_eq!(
            PairCode::parse("OoO0o0O0O0O-0").unwrap(),
            PairCode::parse("000000000000").unwrap()
        );
    }

    #[test]
    fn wrong_lengths_and_foreign_characters_are_refused() {
        assert_eq!(PairCode::parse("").unwrap_err(), CodeError::Length(0));
        assert_eq!(PairCode::parse("7KQ4").unwrap_err(), CodeError::Length(4));
        assert_eq!(
            PairCode::parse("7KQ4M2XD9PTMM").unwrap_err(),
            CodeError::Length(13)
        );
        // U and Z are not in the alphabet; neither is punctuation or a non-ASCII letter.
        for typed in [
            "7KQ4M2XD9PTU",
            "7KQ4M2XD9PTZ",
            "7KQ4M2XD9PT!",
            "7KQ4M2XD9PTé",
            "7KQ4M2XD9PT_",
        ] {
            assert_eq!(
                PairCode::parse(typed).unwrap_err(),
                CodeError::Character,
                "{typed}"
            );
        }
    }

    #[test]
    fn a_generated_code_round_trips_and_is_redacted_in_debug() {
        let counter = std::cell::Cell::new(0u8);
        let random = |buf: &mut [u8]| {
            for byte in buf {
                counter.set(counter.get().wrapping_add(37));
                *byte = counter.get();
            }
        };
        let code = PairCode::generate(&random);
        assert_eq!(PairCode::parse(&code.display()).unwrap(), code);
        assert_eq!(format!("{code:?}"), "PairCode(..)");
        assert_eq!(code.display().len(), 14);
    }

    #[test]
    fn generated_codes_are_uniform_over_the_31_symbols_and_never_hold_z() {
        // Every byte value in turn: 248 to 255 are skipped, the other 248 map to each of the 31
        // symbols 8 times. 248 codes use the usable bytes of 11 rounds: 88 of each symbol.
        let next = std::cell::Cell::new(0u32);
        let random = |buf: &mut [u8]| {
            for byte in buf {
                *byte = (next.get() % 256) as u8;
                next.set(next.get() + 1);
            }
        };
        let mut counts = [0u32; 31];
        for _ in 0..248 {
            let code = PairCode::generate(&random);
            for c in code.data() {
                counts[value(*c).unwrap() as usize] += 1;
            }
            assert!(!code.display().contains('Z'));
        }
        assert!(counts.iter().all(|count| *count == 88), "{counts:?}");
        // A source that only ever gives skipped bytes for a while still ends.
        let calls = std::cell::Cell::new(0);
        let stingy = |buf: &mut [u8]| {
            calls.set(calls.get() + 1);
            buf.fill(if calls.get() < 3 { 255 } else { 7 });
        };
        assert_eq!(PairCode::generate(&stingy).data(), b"77777777777");
    }
}
