//! The host's pairing code (`or2_pair::code`) against the phone's (`or2_core::pair::PairCode`):
//! one contract ("The pairing code `K`" in `docs/contracts.md`), two implementations, and these
//! tests hold them to the same answers. The phone shows a code, the person types it at the host:
//! whatever one side accepts, refuses or reads as another character, the other must too.

use or2_core::pair::{PairCode as PhoneCode, PairCodeError};
use or2_pair::bootstrap::{self, PairingId};
use or2_pair::code::{ALPHABET, CodeError, PairCode as HostCode};

/// The five vectors of the contract (data characters, check character).
const VECTORS: [(&str, char); 5] = [
    ("7KQ4M2XD9PT", 'M'),
    ("00000000000", '0'),
    ("YYYYYYYYYYY", 'V'),
    ("11111111111", '4'),
    ("0123456789A", '6'),
];

/// What one side made of a typed text, comparable across the two.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    Code(String),
    Length,
    Character,
    Check,
}

fn host(text: &str) -> Read {
    match HostCode::parse(text) {
        Ok(code) => Read::Code(code.display()),
        Err(CodeError::Length(_)) => Read::Length,
        Err(CodeError::Character) => Read::Character,
        Err(CodeError::Check) => Read::Check,
    }
}

fn phone(text: &str) -> Read {
    match PhoneCode::parse_typed(text) {
        Ok(code) => Read::Code(code.display()),
        Err(PairCodeError::Length) => Read::Length,
        Err(PairCodeError::Character) => Read::Character,
        Err(PairCodeError::Check) => Read::Check,
    }
}

fn agree(text: &str) -> Read {
    let (h, p) = (host(text), phone(text));
    assert_eq!(h, p, "host and phone read {text:?} differently");
    h
}

#[test]
fn both_sides_read_the_contracts_vectors_alike() {
    for (data, check) in VECTORS {
        let shown = format!("{}-{}-{}{check}", &data[..4], &data[4..8], &data[8..]);
        assert_eq!(agree(&format!("{data}{check}")), Read::Code(shown.clone()));
        assert_eq!(agree(&shown), Read::Code(shown.clone()));
        // The same code, so the same bootstrap key for the same run.
        let id = PairingId::parse("abcdefghijklm").unwrap();
        let host_key = bootstrap::public_key(&HostCode::parse(&shown).unwrap(), &id).openssh();
        let phone_key = PhoneCode::parse_typed(&shown)
            .unwrap()
            .bootstrap_public_key(id.as_str());
        assert_eq!(host_key, phone_key, "{shown}");
    }
}

#[test]
fn both_sides_refuse_and_accept_every_character_alike() {
    // Every character in every position of every vector: the alphabet, `Z` and `U`, the lenient
    // `I`, `L`, `O` and lower case, white space and hyphens (ignored, so the length changes),
    // punctuation, control characters, Latin-1 and a few others.
    let mut characters: Vec<char> = (0u32..=0x24f).filter_map(char::from_u32).collect();
    characters.extend(['\u{2028}', '\u{3000}', '\u{ff21}', '\u{1d400}', '\u{2010}']);
    for (data, check) in VECTORS {
        let good: Vec<char> = format!("{data}{check}").chars().collect();
        for (position, own) in good.iter().enumerate() {
            let mut accepted = 0;
            for c in &characters {
                let mut typed = good.clone();
                typed[position] = *c;
                let text: String = typed.into_iter().collect();
                if let Read::Code(_) = agree(&text) {
                    accepted += 1;
                }
            }
            // Only the position's own character passes, in its spellings: itself, its lower case,
            // and `i I l L` for `1`, `o O` for `0`. The check lets no other substitution through.
            let spellings = match own {
                '1' => 5,
                '0' => 3,
                c if c.is_ascii_alphabetic() => 2,
                _ => 1,
            };
            assert_eq!(accepted, spellings, "{data}{check}: position {position}");
        }
    }
}

#[test]
fn both_sides_refuse_z_and_never_show_it() {
    assert!(!ALPHABET.contains(&b'Z'));
    for text in ["7KQ4-M2XD-9PTZ", "Z000-0000-0000", "zzzz-zzzz-zzz0"] {
        assert_eq!(agree(text), Read::Character, "{text}");
    }
    // A `0` where the old alphabet allowed `Z` (value 31, 0 modulo 31) is a plain code.
    assert_eq!(agree("0000-0000-0000"), Read::Code("0000-0000-0000".into()));
}

#[test]
fn both_sides_read_lenient_and_wrong_input_alike() {
    for (text, expected) in [
        ("7kq4-m2xd-9ptm", Read::Code("7KQ4-M2XD-9PTM".into())),
        (" 7KQ4 M2XD 9PTM \n", Read::Code("7KQ4-M2XD-9PTM".into())),
        ("\t7KQ4\tM2XD-9PTM\r\n", Read::Code("7KQ4-M2XD-9PTM".into())),
        (
            "7-K-Q-4-M-2-X-D-9-P-T-M",
            Read::Code("7KQ4-M2XD-9PTM".into()),
        ),
        ("ilIL-1liL-1l14", Read::Code("1111-1111-1114".into())),
        ("OoO0-o0O0-O0O0", Read::Code("0000-0000-0000".into())),
        ("", Read::Length),
        ("ZZZ", Read::Length),
        ("7KQ4-M2XD-9PT", Read::Length),
        ("7KQ4-M2XD-9PTMM", Read::Length),
        ("7KQ4-M2XD-9PTM-Z", Read::Length),
        ("7KQ4-M2XD-9PTU", Read::Character),
        ("7KQ4-M2XD-9PT!", Read::Character),
        ("7KQ4-M2XD-9PT_", Read::Character),
        ("7KQ4-M2XD-9PT\u{e9}", Read::Character),
        ("7KQ4-M2XD-9PTA", Read::Check),
        ("K7Q4-M2XD-9PTM", Read::Check),
    ] {
        assert_eq!(agree(text), expected, "{text:?}");
    }
}

#[test]
fn every_code_the_host_draws_is_one_the_phone_reads_the_same() {
    // Every byte value through the host's drawing (the phone draws the same way: a byte below 248
    // as its value modulo 31, the rest dropped): what it shows, the phone reads back unchanged.
    for start in 0..=255u8 {
        let next = std::cell::Cell::new(start);
        let random = |buf: &mut [u8]| {
            for byte in buf {
                *byte = next.get();
                next.set(next.get().wrapping_add(1));
            }
        };
        let code = HostCode::generate(&random);
        let shown = code.display();
        assert!(!shown.contains('Z'), "{shown}");
        assert_eq!(agree(&shown), Read::Code(shown.clone()));
    }
}

#[test]
fn both_sides_explain_a_refused_character_alike() {
    // The two messages name the same alphabet and its two never-used letters.
    let host = CodeError::Character.to_string();
    let phone = PairCodeError::Character.to_string();
    for part in [
        "character",
        "codes never use",
        "0-9 and A-Y",
        "never U or Z",
    ] {
        assert!(host.contains(part), "{host}");
        assert!(phone.contains(part), "{phone}");
    }
}
