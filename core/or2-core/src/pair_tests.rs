//! The pairing code `K`, the parser of the host's code, the bootstrap key derivation, and the
//! phone's client against an in-process SSH server that plays `or2-pair` (every error mapping, the
//! shell-noise rules, the timeouts, cancellation). The client against a real sshd with a forced
//! command is in `tests/pair.rs`.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::server;
use tokio::io::DuplexStream;

use super::*;

const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
const HOST_FINGERPRINT: &str = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI";
const ECDSA_KEY: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
/// A canonical pairing id (the last character carries no stray bit).
const ID: &str = "abcdefghijklm";
const CODE_TEXT: &str = "7KQ4-M2XD-9PTM";

// --- the pairing code K ------------------------------------------------------------------------

#[test]
fn the_check_character_is_the_weighted_sum_mod_31() {
    // 7KQ4M2XD9PT: values 7 19 23 4 20 2 29 13 9 22 26, weights 1..=11:
    // 7 + 38 + 69 + 16 + 100 + 12 + 203 + 104 + 81 + 220 + 286 = 1136 = 36 * 31 + 20, and 20 is `M`.
    let code = PairCode::parse_typed(CODE_TEXT).unwrap();
    assert_eq!(code.display(), "7KQ4-M2XD-9PTM");
    assert_eq!(&code.data()[..], b"7KQ4M2XD9PT");
    assert_eq!(
        PairCode::parse_typed("7KQ4-M2XD-9PTA").unwrap_err(),
        PairCodeError::Check
    );
}

#[test]
fn the_alphabet_is_crockford_base32_without_z() {
    // 31 symbols, one per value modulo the prime 31: no two characters share a check value.
    assert_eq!(ALPHABET.len(), 31);
    assert_eq!(ALPHABET.len(), CHECK_MODULUS as usize);
    for excluded in *b"ILOUZ" {
        assert!(!ALPHABET.contains(&excluded));
    }
    let mut sorted = *ALPHABET;
    sorted.sort_unstable();
    assert_eq!(&sorted, ALPHABET, "digits, then letters, in order");
}

#[test]
fn z_is_not_a_code_character() {
    // `Z` was value 31, the same as `0` modulo 31; codes never contain it and typing it is refused
    // like any other character codes never use, wherever it stands.
    for text in [
        "ZZZZ-ZZZZ-ZZZ0",
        "7KQ4-M2XD-9PTZ",
        "Z000-0000-0000",
        "7kq4-m2xd-9ptz",
    ] {
        assert_eq!(
            PairCode::parse_typed(text).unwrap_err(),
            PairCodeError::Character,
            "{text:?}"
        );
    }
    assert!(PairCodeError::Character.to_string().contains('Z'));
}

#[test]
fn every_single_wrong_character_and_every_neighbour_swap_fails_the_check() {
    // With 31 symbols, one per value modulo 31, there is no exception: every substitution of one
    // character by another code character, in any position (the check character included), and
    // every swap of two different neighbours is a typo.
    for text in [
        CODE_TEXT,
        "0000-0000-0000",
        "YYYY-YYYY-YYYV",
        "0123-4567-89A6",
    ] {
        let code = PairCode::parse_typed(text).unwrap();
        let plain: Vec<u8> = code.0.to_vec();
        let typed = |values: &[u8]| -> String {
            values
                .iter()
                .map(|v| char::from(ALPHABET[usize::from(*v)]))
                .collect()
        };
        for position in 0..CODE_CHARS {
            for value in 0..ALPHABET.len() as u8 {
                if value == plain[position] {
                    continue;
                }
                let mut changed = plain.clone();
                changed[position] = value;
                assert_eq!(
                    PairCode::parse_typed(&typed(&changed)).unwrap_err(),
                    PairCodeError::Check,
                    "{text}: position {position} value {value}"
                );
            }
        }
        for position in 0..CODE_CHARS - 1 {
            if plain[position] == plain[position + 1] {
                continue;
            }
            let mut swapped = plain.clone();
            swapped.swap(position, position + 1);
            assert_eq!(
                PairCode::parse_typed(&typed(&swapped)).unwrap_err(),
                PairCodeError::Check,
                "{text}: swap at {position}"
            );
        }
    }
}

#[test]
fn typed_input_is_read_leniently() {
    for text in [
        "7KQ4-M2XD-9PTM",
        "7kq4-m2xd-9ptm",
        "7KQ4M2XD9PTM",
        " 7KQ4 M2XD 9PTM ",
        "7-K-Q-4-M-2-X-D-9-P-T-M",
    ] {
        assert_eq!(
            PairCode::parse_typed(text).unwrap().display(),
            CODE_TEXT,
            "{text:?}"
        );
    }
    // `I` and `L` read as 1, `O` as 0.
    let with_ones = PairCode::parse_typed("1I1L-0O00-000A").map(|c| c.display());
    let plain = PairCode::parse_typed("1111-0000-000A").map(|c| c.display());
    assert_eq!(with_ones, plain);
    assert!(
        plain.is_ok(),
        "the check of 1111 0000 000 is 10, which is A"
    );
    for (text, expected) in [
        ("", PairCodeError::Length),
        ("7KQ4-M2XD-9PT", PairCodeError::Length),
        ("7KQ4-M2XD-9PTMM", PairCodeError::Length),
        ("7KQ4-M2XD-9PTU", PairCodeError::Character),
        ("7KQ4-M2XD-9PT!", PairCodeError::Character),
        ("7KQ4-M2XD-9PT\u{e9}", PairCodeError::Character),
    ] {
        assert_eq!(
            PairCode::parse_typed(text).unwrap_err(),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn a_generated_code_is_well_formed_and_differs_each_time() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 {
        let code = PairCode::generate();
        let shown = code.display();
        assert_eq!(shown.len(), 14);
        assert_eq!(
            shown.matches('-').count(),
            2,
            "three groups of four: {shown}"
        );
        assert!(
            shown.bytes().all(|b| b == b'-' || ALPHABET.contains(&b)),
            "{shown}"
        );
        assert_eq!(
            PairCode::parse_typed(&shown).unwrap().display(),
            shown,
            "the check character is valid"
        );
        seen.insert(shown);
    }
    assert_eq!(seen.len(), 50);
}

#[test]
fn the_check_character_matches_the_vectors_the_host_crate_tests() {
    // The same five vectors as `or2-pair`'s `code` tests (contract: "The pairing code"). For
    // 7KQ4M2XD9PT: 7 + 2*19 + 3*23 + 4*4 + 5*20 + 6*2 + 7*29 + 8*13 + 9*9 + 10*22 + 11*26 = 1136
    // = 36*31 + 20, and value 20 is `M`. For YYYYYYYYYYY (`Y`, the last symbol, is 30):
    // 30 * 66 = 1980 = 63*31 + 27, and value 27 is `V`.
    for (data, check) in [
        ("7KQ4M2XD9PT", 'M'),
        ("00000000000", '0'),
        ("YYYYYYYYYYY", 'V'),
        ("11111111111", '4'),
        ("0123456789A", '6'),
    ] {
        let code = PairCode::parse_typed(&format!("{data}{check}")).unwrap();
        assert_eq!(
            code.display(),
            format!("{}-{}-{}{check}", &data[..4], &data[4..8], &data[8..])
        );
        assert_eq!(&code.data()[..], data.as_bytes());
    }
}

#[test]
fn each_byte_value_maps_uniformly_onto_the_31_symbols() {
    // 248 = 8 * 31: the bytes below it give every symbol the same share (8 each); the 8 above
    // are dropped, never folded onto some symbols.
    let mut counts = [0usize; 31];
    let mut dropped = 0;
    for byte in 0..=255u8 {
        match random_value(byte) {
            Some(value) => counts[usize::from(value)] += 1,
            None => dropped += 1,
        }
    }
    assert!(counts.iter().all(|count| *count == 8), "{counts:?}");
    assert_eq!(dropped, 8);
    assert_eq!(random_value(247), Some(30));
    assert_eq!(random_value(248), None);
}

#[test]
fn dropped_bytes_are_replaced_by_more_random_bytes() {
    // The first fill is all dropped bytes, the second alternates dropped and kept ones: the code
    // is made from the kept bytes only, in order, after as many fills as it takes.
    let mut fills = 0;
    let code = PairCode::from_random(|pool| {
        fills += 1;
        for (index, byte) in pool.iter_mut().enumerate() {
            *byte = match (fills, index % 2) {
                (1, _) | (_, 0) => 248 + (index % 8) as u8,
                // 31 + v is v modulo 31: the kept bytes are 31, 32, 33, …
                _ => 31 + (index / 2) as u8,
            };
        }
    });
    assert_eq!(fills, 2);
    assert_eq!(&code.data()[..], b"0123456789A");
    assert_eq!(code.display(), "0123-4567-89A6");
}

#[test]
fn generated_characters_are_statistically_uniform_and_never_z() {
    // 20 000 codes x 11 characters over 31 symbols: about 7097 expected each, sigma about 83. A
    // bound of 6 sigma (500) fails only for a broken generator.
    let mut counts = [0usize; 31];
    for _ in 0..20_000 {
        let code = PairCode::generate();
        assert!(!code.display().contains('Z'));
        for value in &code.0[..DATA_CHARS] {
            counts[usize::from(*value)] += 1;
        }
    }
    let expected = 20_000 * DATA_CHARS / 31;
    for (symbol, count) in counts.iter().enumerate() {
        assert!(count.abs_diff(expected) < 500, "symbol {symbol}: {count}");
    }
}

#[test]
fn the_code_is_redacted_in_debug_and_wiped_on_drop() {
    let code = PairCode::parse_typed(CODE_TEXT).unwrap();
    let shown = format!("{code:?}");
    assert_eq!(shown, "PairCode(<redacted>)");
    assert!(!shown.contains("7KQ4"));
    // The buffer is a `Zeroizing`, wiped when the code is dropped; the marker records it.
    fn wiped_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    wiped_on_drop::<PairCode>();
}

// --- the bootstrap key -------------------------------------------------------------------------

/// Computed with the openssl CLI (3.6), not with this code:
///
/// ```text
/// openssl kdf -keylen 32 -kdfopt digest:SHA256 \
///     -kdfopt hexkey:374b51344d325844395054 \                  # "7KQ4M2XD9PT"
///     -kdfopt hexsalt:6162636465666768696a6b6c6d \             # "abcdefghijklm"
///     -kdfopt hexinfo:6f72322d706169722f3220626f6f7473747261702065643235353139 \
///     -kdfopt mode:EXTRACT_AND_EXPAND -binary HKDF | xxd -p -c 64
/// # the seed, then the Ed25519 key it is the RFC 8032 seed of, as PKCS#8 (the fixed 16-byte
/// # prefix of an Ed25519 PrivateKeyInfo, then the seed):
/// printf '302e020100300506032b657004220420<seed hex>' | xxd -r -p |
///     openssl pkey -inform DER -pubout -outform DER | xxd -p -c 64
/// # the last 32 bytes are the public key
/// ```
const VECTOR_SEED: &str = "734c24be4849a8de10227c52bd2530221cd6d2e62062c650886f0db0b99aeed0";
const VECTOR_PUBLIC: &str = "02d8bd7aee56213d1bcdd3f38649a9b749904226955d0d460f400ba630e0d628";

#[test]
fn the_bootstrap_key_matches_the_independently_computed_vector() {
    let code = PairCode::parse_typed(CODE_TEXT).unwrap();
    assert_eq!(&code.data()[..], b"7KQ4M2XD9PT");
    assert_eq!(hex::encode(*code.bootstrap_seed(ID)), VECTOR_SEED);
    let public = code.bootstrap_key(ID).public_key();
    let blob = data_encoding::BASE64
        .decode(public.openssh.split(' ').nth(1).unwrap().as_bytes())
        .unwrap();
    // The SSH wire form: string "ssh-ed25519", then the 32-byte public key as a string.
    assert_eq!(hex::encode(&blob[blob.len() - 32..]), VECTOR_PUBLIC);
    assert_eq!(public.algorithm, "ssh-ed25519");
}

#[test]
fn the_bootstrap_key_depends_on_the_code_and_on_the_id() {
    let code = PairCode::parse_typed(CODE_TEXT).unwrap();
    let other = PairCode::parse_typed("1111-0000-000A").unwrap();
    let key = |code: &PairCode, id: &str| code.bootstrap_public_key(id);
    assert_eq!(key(&code, ID), key(&code, ID));
    assert_ne!(key(&code, ID), key(&other, ID));
    assert_ne!(key(&code, ID), key(&code, "abcdefghijkli"));
    assert!(key(&code, ID).starts_with("ssh-ed25519 "));
    assert_eq!(key(&code, ID).split(' ').count(), 2, "no comment");
}

// --- the parser --------------------------------------------------------------------------------

/// Percent-encodes a value the way the host does: unreserved characters and `:` stay.
fn enc(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn code(fields: &[(&str, &str)]) -> String {
    let query: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={}", enc(value)))
        .collect();
    format!("or2-pair:2?{}", query.join("&"))
}

fn full() -> Vec<(&'static str, &'static str)> {
    vec![
        ("name", "Work Mac"),
        ("user", "alice"),
        ("port", "22"),
        ("a", "100.101.102.103"),
        ("a", "2001:db8::9"),
        ("a", "work-mac.local"),
        ("hk", HOST_KEY),
        ("id", ID),
    ]
}

fn without(
    fields: &[(&'static str, &'static str)],
    key: &str,
) -> Vec<(&'static str, &'static str)> {
    fields.iter().filter(|(k, _)| *k != key).copied().collect()
}

fn replaced<'a>(
    fields: &[(&'static str, &'a str)],
    key: &str,
    value: &'a str,
) -> Vec<(&'static str, &'a str)> {
    fields
        .iter()
        .map(|(k, v)| if *k == key { (*k, value) } else { (*k, *v) })
        .collect()
}

#[test]
fn a_full_code_parses_into_endpoints_a_key_and_an_id() {
    let offer = PairOffer::parse(&code(&full())).unwrap();
    assert_eq!(offer.name, "Work Mac");
    assert_eq!(offer.username, "alice");
    assert_eq!(offer.port, 22);
    let hosts: Vec<_> = offer
        .addresses
        .iter()
        .map(|a| (a.host().to_owned(), a.port()))
        .collect();
    assert_eq!(
        hosts,
        [
            ("100.101.102.103".to_owned(), 22),
            ("2001:db8::9".to_owned(), 22),
            ("work-mac.local".to_owned(), 22)
        ]
    );
    assert_eq!(offer.host_key.fingerprint(), HOST_FINGERPRINT);
    assert_eq!(offer.pairing_id.as_deref(), Some(ID));
}

#[test]
fn a_manual_code_has_no_pairing_id() {
    let offer = PairOffer::parse(&code(&without(&full(), "id"))).unwrap();
    assert_eq!(offer.pairing_id, None);
}

#[test]
fn whitespace_around_a_pasted_code_is_ignored() {
    let text = format!("  \n{}\r\n", code(&full()));
    assert!(PairOffer::parse(&text).is_ok());
}

#[test]
fn percent_escapes_may_be_lower_case_and_values_may_carry_unicode() {
    let text = code(&replaced(&full(), "name", "Büro-Mac")).replace("%C3%BC", "%c3%bc");
    assert_eq!(PairOffer::parse(&text).unwrap().name, "Büro-Mac");
}

#[test]
fn an_ecdsa_host_key_is_accepted_and_a_comment_is_not() {
    assert!(PairOffer::parse(&code(&replaced(&full(), "hk", ECDSA_KEY))).is_ok());
    let with_comment = format!("{HOST_KEY} root@box");
    assert_eq!(
        PairOffer::parse(&code(&replaced(&full(), "hk", &with_comment))).unwrap_err(),
        PairParseError::InvalidField("hk")
    );
}

#[test]
fn refuses_what_is_not_a_pairing_code() {
    for text in [
        "",
        "hello",
        "https://example.org/?a=b",
        "OR2-PAIR:2?x=y",
        "or2-pair",
        "ssh-ed25519 AAAA",
    ] {
        assert_eq!(
            PairOffer::parse(text).unwrap_err(),
            PairParseError::NotPairingCode,
            "{text:?}"
        );
    }
}

#[test]
fn other_versions_are_named_unsupported_with_their_number() {
    for (text, version) in [
        // Version 1 is the old listener code: its fields are never read.
        ("or2-pair:1?name=x&pair=192.0.2.1:5&otp=AAAA", 1),
        ("or2-pair:3?name=x", 3),
        ("or2-pair:10?name=x", 10),
        ("or2-pair:0?x=y", 0),
        ("or2-pair:4294967295?x=y", u32::MAX),
    ] {
        assert_eq!(
            PairOffer::parse(text).unwrap_err(),
            PairParseError::UnsupportedVersion { version },
            "{text}"
        );
    }
    for text in [
        "or2-pair:?name=x",
        "or2-pair:2.0?name=x",
        "or2-pair:v2?name=x",
        "or2-pair:02?name=x",
        "or2-pair:+2?name=x",
        "or2-pair:4294967296?name=x",
        "or2-pair:2",
        "or2-pair:",
    ] {
        assert_eq!(
            PairOffer::parse(text).unwrap_err(),
            PairParseError::Malformed,
            "{text}"
        );
    }
}

#[test]
fn a_code_over_one_kilobyte_is_refused_before_it_is_parsed() {
    let name = "x".repeat(1100);
    let long = replaced(&full(), "name", &name);
    assert_eq!(
        PairOffer::parse(&code(&long)).unwrap_err(),
        PairParseError::TooLong
    );
    // Exactly the limit is not too long (it fails later, for another reason, or parses).
    let text = code(&full());
    let padded = format!(
        "{text}&a={}",
        "b".repeat(MAX_PAYLOAD_BYTES - text.len() - 3)
    );
    assert_eq!(padded.len(), MAX_PAYLOAD_BYTES);
    assert_ne!(
        PairOffer::parse(&padded).unwrap_err(),
        PairParseError::TooLong
    );
}

#[test]
fn every_required_field_is_required_but_the_id() {
    for field in ["name", "user", "port", "a", "hk"] {
        assert_eq!(
            PairOffer::parse(&code(&without(&full(), field))).unwrap_err(),
            PairParseError::MissingField(field),
            "{field}"
        );
    }
}

#[test]
fn single_fields_may_not_repeat_and_unknown_fields_are_refused() {
    for field in ["name", "user", "port", "hk", "id"] {
        let mut fields = full();
        let again = *fields.iter().find(|(k, _)| *k == field).unwrap();
        fields.push(again);
        assert_eq!(
            PairOffer::parse(&code(&fields)).unwrap_err(),
            PairParseError::DuplicateField(field),
            "{field}"
        );
    }
    // Version 1's fields are unknown in version 2.
    for (field, value) in [
        ("extra", "1"),
        ("pair", "192.0.2.1:5000"),
        ("otp", "AAAQEAYEAUDAOCAJBIFQYDIOB4"),
        ("ID", ID),
    ] {
        let mut fields = full();
        fields.push((field, value));
        assert_eq!(
            PairOffer::parse(&code(&fields)).unwrap_err(),
            PairParseError::UnknownField,
            "{field}"
        );
    }
}

#[test]
fn malformed_syntax_is_refused() {
    let good = code(&full());
    for text in [
        format!("{good}&novalue"),
        format!("{good}&"),
        format!("{good}&a=%zz"),
        format!("{good}&a=%4"),
        format!("{good}&a=%FF"),
        format!("{good}&a=x y"),
        format!("{good}#frag"),
        format!("{good}&a=\u{e9}"),
        "or2-pair:2?".to_owned(),
    ] {
        assert_eq!(
            PairOffer::parse(&text).unwrap_err(),
            PairParseError::Malformed,
            "{text:?}"
        );
    }
}

#[test]
fn names_and_users_are_bounded_text_without_controls() {
    let long = "n".repeat(65);
    let ok = "n".repeat(64);
    for (field, value, valid) in [
        ("name", "", false),
        ("name", "   ", false),
        ("name", "a\nb", false),
        ("name", "a\u{7}", false),
        ("name", long.as_str(), false),
        ("name", ok.as_str(), true),
        ("name", " padded ", true),
        ("user", "", false),
        ("user", "bad\tuser", false),
        ("user", "John Smith", true),
        ("user", long.as_str(), false),
    ] {
        let parsed = PairOffer::parse(&code(&replaced(&full(), field, value)));
        assert_eq!(parsed.is_ok(), valid, "{field}={value:?}");
        if !valid {
            let expected: &'static str = if field == "name" { "name" } else { "user" };
            assert_eq!(parsed.unwrap_err(), PairParseError::InvalidField(expected));
        }
    }
}

#[test]
fn ports_are_decimal_one_to_65535() {
    for (value, valid) in [
        ("22", true),
        ("1", true),
        ("65535", true),
        ("0", false),
        ("65536", false),
        ("", false),
        ("-1", false),
        ("+22", false),
        ("22a", false),
        (" 22", false),
        ("0022", true),
        ("123456", false),
        ("0x16", false),
    ] {
        let parsed = PairOffer::parse(&code(&replaced(&full(), "port", value)));
        assert_eq!(parsed.is_ok(), valid, "{value:?}");
    }
}

#[test]
fn addresses_are_names_or_ip_literals_at_most_eight_without_duplicates() {
    for (value, valid) in [
        ("192.0.2.1", true),
        ("host.local", true),
        ("fe80::1", true),
        ("2001:db8::9", true),
        ("a_b-c.d", true),
        ("[::1]", false),
        ("a b", false),
        ("a/b", false),
        ("user@host", false),
        ("host?x", false),
        ("", false),
    ] {
        let mut fields = without(&full(), "a");
        fields.push(("a", value));
        assert_eq!(PairOffer::parse(&code(&fields)).is_ok(), valid, "{value:?}");
    }
    let mut fields = without(&full(), "a");
    fields.push(("a", "10.0.0.1"));
    fields.push(("a", "10.0.0.1"));
    assert_eq!(
        PairOffer::parse(&code(&fields)).unwrap_err(),
        PairParseError::InvalidField("a")
    );
    let many: Vec<String> = (1..=9).map(|i| format!("10.0.0.{i}")).collect();
    let mut fields = without(&full(), "a");
    for address in &many {
        fields.push(("a", address));
    }
    assert_eq!(
        PairOffer::parse(&code(&fields)).unwrap_err(),
        PairParseError::InvalidField("a")
    );
    fields.pop();
    assert_eq!(PairOffer::parse(&code(&fields)).unwrap().addresses.len(), 8);
}

#[test]
fn the_host_key_must_be_a_plain_public_key() {
    for value in [
        "",
        "ssh-ed25519",
        "ssh-ed25519 ",
        "ssh-ed25519 AAAA",
        "ssh-ed25519 !!!!",
        "ssh-dss AAAAB3NzaC1kc3M=",
        "AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7",
        "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7",
        // Two keys on one line.
        &format!("{HOST_KEY} {HOST_KEY}"),
        // A private key marker.
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        // A security-key type is not a host key.
        "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7AAAABHNzaDo=",
    ] {
        assert_eq!(
            PairOffer::parse(&code(&replaced(&full(), "hk", value))).unwrap_err(),
            PairParseError::InvalidField("hk"),
            "{value:?}"
        );
    }
}

#[test]
fn the_id_is_exactly_13_canonical_lowercase_base32_characters() {
    for (value, valid) in [
        (ID, true),
        ("aaaaaaaaaaaaa", true),
        ("zzzzzzzzzzzzy", true),
        // Wrong length.
        ("abcdefghijkl", false),
        ("abcdefghijklmn", false),
        ("", false),
        // Upper case, padding, characters outside the alphabet.
        ("ABCDEFGHIJKLM", false),
        ("abcdefghijkl=", false),
        ("abcdefghijkl1", false),
        ("abcdefghijkl8", false),
        ("abcdefghijkl-", false),
        // The last character carries one stray bit: not the canonical encoding.
        ("abcdefghijkln", false),
        ("zzzzzzzzzzzzz", false),
    ] {
        assert_eq!(is_pairing_id(value), valid, "{value:?}");
        let parsed = PairOffer::parse(&code(&replaced(&full(), "id", value)));
        assert_eq!(parsed.is_ok(), valid, "{value:?}");
        if !valid {
            assert_eq!(parsed.unwrap_err(), PairParseError::InvalidField("id"));
        }
    }
}

// --- the client against an in-process SSH server -----------------------------------------------

/// What the fake `or2-pair` does.
#[derive(Clone)]
enum Step {
    Data(Vec<u8>),
    Stderr(Vec<u8>),
    /// Exit status, EOF and close, as sshd does when the forced command ends.
    Close,
}

#[derive(Default)]
struct Scenario {
    /// After `exec`.
    on_exec: Vec<Step>,
    /// After the request line arrived.
    on_request: Vec<Step>,
    /// Answers `exec` with a failure.
    refuse_exec: bool,
}

#[derive(Default)]
struct Observed {
    commands: Mutex<Vec<String>>,
    requests: Mutex<Vec<u8>>,
    publickey_attempts: AtomicUsize,
    other_attempts: AtomicUsize,
    channel_closes: AtomicUsize,
}

struct Fake {
    bootstrap: russh::keys::PublicKey,
    scenario: Arc<Scenario>,
    observed: Arc<Observed>,
}

impl server::Handler for Fake {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        self.observed.other_attempts.fetch_add(1, Ordering::SeqCst);
        Ok(server::Auth::reject())
    }

    async fn auth_password(&mut self, _: &str, _: &str) -> Result<server::Auth, Self::Error> {
        self.observed.other_attempts.fetch_add(1, Ordering::SeqCst);
        Ok(server::Auth::reject())
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<server::Auth, Self::Error> {
        self.observed
            .publickey_attempts
            .fetch_add(1, Ordering::SeqCst);
        Ok(if key.key_data() == self.bootstrap.key_data() {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }

    async fn channel_open_session(
        &mut self,
        _: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .commands
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(command).into_owned());
        if self.scenario.refuse_exec {
            return session.channel_failure(channel);
        }
        session.channel_success(channel)?;
        play(&self.scenario.on_exec, session, channel)
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let complete = {
            let mut requests = self.observed.requests.lock().unwrap();
            requests.extend_from_slice(data);
            requests.contains(&b'\n')
        };
        if complete {
            play(&self.scenario.on_request, session, channel)?;
        }
        Ok(())
    }

    async fn channel_close(
        &mut self,
        _: russh::ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed.channel_closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn play(
    steps: &[Step],
    session: &mut server::Session,
    channel: russh::ChannelId,
) -> Result<(), russh::Error> {
    for step in steps {
        match step {
            Step::Data(bytes) => session.data(channel, bytes.clone())?,
            Step::Stderr(bytes) => session.extended_data(channel, 1, bytes.clone())?,
            Step::Close => {
                session.exit_status_request(channel, 0)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
        }
    }
    Ok(())
}

/// A transport whose every connection is a pipe to a fresh in-process server.
struct Pipes {
    host: russh::keys::PrivateKey,
    bootstrap: russh::keys::PublicKey,
    scenario: Arc<Scenario>,
    observed: Arc<Observed>,
    connections: AtomicUsize,
}

impl Transport for Pipes {
    type Stream = DuplexStream;

    async fn connect(&self, _: &Endpoint) -> io::Result<DuplexStream> {
        self.connections.fetch_add(1, Ordering::SeqCst);
        let (client, server_end) = tokio::io::duplex(64 * 1024);
        let config = Arc::new(server::Config {
            keys: vec![self.host.clone()],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let handler = Fake {
            bootstrap: self.bootstrap.clone(),
            scenario: Arc::clone(&self.scenario),
            observed: Arc::clone(&self.observed),
        };
        tokio::spawn(async move {
            if let Ok(session) = server::run_stream(config, server_end, handler).await {
                let _ = session.await;
            }
        });
        Ok(client)
    }
}

/// A transport that always fails to connect.
struct Failing(io::ErrorKind);

impl Transport for Failing {
    type Stream = DuplexStream;

    async fn connect(&self, _: &Endpoint) -> io::Result<DuplexStream> {
        Err(io::Error::from(self.0))
    }
}

struct Setup {
    transport: Arc<Pipes>,
    offer: PairOffer,
    code: PairCode,
    phone: ClientKey,
}

fn hello_line(id: &str) -> Vec<u8> {
    format!("{{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"{id}\"}}\n").into_bytes()
}

fn hello() -> Step {
    Step::Data(hello_line(ID))
}

fn data(text: &str) -> Step {
    Step::Data(text.as_bytes().to_vec())
}

impl Setup {
    fn new(scenario: Scenario) -> Self {
        let host = ClientKey::generate_ed25519("");
        let code = PairCode::parse_typed(CODE_TEXT).unwrap();
        let bootstrap = russh::keys::PublicKey::from_openssh(&code.bootstrap_public_key(ID))
            .expect("the bootstrap public key is an OpenSSH key");
        let transport = Arc::new(Pipes {
            host: host.private_key().clone(),
            bootstrap,
            scenario: Arc::new(scenario),
            observed: Arc::default(),
            connections: AtomicUsize::new(0),
        });
        let offer = PairOffer {
            name: "workstation".into(),
            username: "dev".into(),
            port: 22,
            addresses: vec![
                Endpoint::new("workstation.local", 22).unwrap(),
                Endpoint::new("198.51.100.7", 22).unwrap(),
            ],
            host_key: HostKey::from_openssh(&host.public_key().openssh).unwrap(),
            pairing_id: Some(ID.into()),
        };
        Self {
            transport,
            offer,
            code,
            phone: ClientKey::generate_ed25519("phone"),
        }
    }

    fn phone_line(&self) -> String {
        self.phone.public_key().openssh
    }

    fn fingerprint(&self) -> String {
        self.phone.public_key().fingerprint
    }

    /// An `ok` verdict for the phone's key.
    fn ok(&self) -> Step {
        Step::Data(
            format!(
                "{{\"v\":2,\"ok\":true,\"user\":\"dev\",\"fingerprint\":\"{}\"}}\n",
                self.fingerprint()
            )
            .into_bytes(),
        )
    }

    async fn enroll(&self, step: Duration) -> Result<PairResult, PairError> {
        pair_enroll(
            &self.transport,
            &self.offer,
            &self.code,
            &self.phone_line(),
            "OnePlus",
            PairTiming { step },
        )
        .await
    }

    fn observed(&self) -> &Observed {
        &self.transport.observed
    }
}

/// Plenty for work in memory, short enough that the timeout tests do not drag.
const STEP: Duration = Duration::from_secs(5);
const SHORT: Duration = Duration::from_millis(400);

fn scenario(on_exec: Vec<Step>, on_request: Vec<Step>) -> Scenario {
    Scenario {
        on_exec,
        on_request,
        refuse_exec: false,
    }
}

impl Setup {
    /// The same host, phone and code with a fake host that runs `scenario` instead.
    fn running(self, scenario: Scenario) -> Self {
        Self {
            transport: Arc::new(Pipes {
                scenario: Arc::new(scenario),
                host: self.transport.host.clone(),
                bootstrap: self.transport.bootstrap.clone(),
                observed: Arc::default(),
                connections: AtomicUsize::new(0),
            }),
            ..self
        }
    }
}

/// A setup whose fake host runs `scenario`.
fn with(scenario: Scenario) -> Setup {
    Setup::new(Scenario::default()).running(scenario)
}

/// A setup whose fake host says hello and answers the request with `verdict(setup)`, then exits.
fn answering(verdict: impl FnOnce(&Setup) -> Step) -> Setup {
    let setup = Setup::new(Scenario::default());
    let step = verdict(&setup);
    setup.running(scenario(vec![hello()], vec![step, Step::Close]))
}

#[tokio::test]
async fn pairs_with_one_handshake_one_authentication_and_the_documented_exchange() {
    let setup = answering(|setup| setup.ok());
    let result = setup.enroll(STEP).await.unwrap();
    assert_eq!(
        result,
        PairResult {
            username: "dev".into(),
            fingerprint: setup.fingerprint(),
        }
    );
    let observed = setup.observed();
    assert_eq!(*observed.commands.lock().unwrap(), ["or2-pair"]);
    // The request: the key without its comment, the device label, one line.
    let key = setup.phone_line();
    let key = key.split(' ').take(2).collect::<Vec<_>>().join(" ");
    assert_eq!(
        String::from_utf8(observed.requests.lock().unwrap().clone()).unwrap(),
        format!("{{\"v\":2,\"key\":\"{key}\",\"device\":\"OnePlus\"}}\n")
    );
    // One authentication: the bootstrap key, once. No password, no probing with other methods.
    assert_eq!(observed.publickey_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(observed.other_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(setup.transport.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn every_refusal_reason_has_its_own_error() {
    for (reason, expected) in [
        ("expired", PairError::Expired),
        ("gone", PairError::Gone),
        ("key", PairError::KeyNotAccepted),
        ("failed", PairError::HostFailed),
        ("request", PairError::Refused),
        ("something-new", PairError::Refused),
        ("", PairError::Refused),
    ] {
        let setup = answering(|_| {
            data(&format!(
                "{{\"v\":2,\"ok\":false,\"reason\":\"{reason}\"}}\n"
            ))
        });
        assert_eq!(setup.enroll(STEP).await.unwrap_err(), expected, "{reason}");
    }
    // A refusal without a reason at all.
    let setup = answering(|_| data("{\"v\":2,\"ok\":false}\n"));
    assert_eq!(setup.enroll(STEP).await.unwrap_err(), PairError::Refused);
}

#[tokio::test]
async fn shell_noise_before_the_hello_is_skipped_within_four_kilobytes() {
    let ok = |s: &Setup| s.ok();
    let noise_of = |size: usize| -> Vec<u8> {
        let mut noise = vec![b'x'; size - 1];
        noise.push(b'\n');
        noise
    };
    for (name, steps) in [
        (
            "a banner",
            vec![data("Welcome to workstation\nLast login: never\n"), hello()],
        ),
        (
            "noise on stderr",
            vec![Step::Stderr(b"warning: something\n".to_vec()), hello()],
        ),
        ("an empty line", vec![data("\n\n"), hello()]),
        (
            "the hello split over chunks",
            vec![
                data("motd\n{\"v\":2,\"he"),
                data("llo\":\"or2-pair\",\"id\":\"abcd"),
                data("efghijklm\"}\n"),
            ],
        ),
        (
            "a carriage return before the newline",
            vec![data(&format!(
                "{}\r\n",
                String::from_utf8(hello_line(ID)).unwrap().trim_end()
            ))],
        ),
        (
            "exactly 4096 bytes of noise",
            vec![Step::Data(noise_of(NOISE_LIMIT)), hello()],
        ),
    ] {
        let setup = Setup::new(Scenario::default());
        let verdict = ok(&setup);
        let setup = setup.running(scenario(steps, vec![verdict, Step::Close]));
        assert!(setup.enroll(STEP).await.is_ok(), "{name}");
    }
    for (name, steps) in [
        (
            "4097 bytes of noise",
            vec![Step::Data(noise_of(NOISE_LIMIT + 1)), hello()],
        ),
        (
            "a long line without a newline",
            vec![Step::Data(vec![b'x'; 5000])],
        ),
        (
            "a hello that is not at the start of its line",
            vec![
                Step::Data([b"prompt> ".to_vec(), hello_line(ID)].concat()),
                Step::Close,
            ],
        ),
        ("silence and exit", vec![Step::Close]),
        (
            "output that is not the hello",
            vec![data("sh: or2-pair: not found\n"), Step::Close],
        ),
    ] {
        let setup = with(scenario(steps, vec![]));
        assert_eq!(
            setup.enroll(STEP).await.unwrap_err(),
            PairError::NotOr2Pair,
            "{name}"
        );
        assert!(
            setup.observed().requests.lock().unwrap().is_empty(),
            "{name}: nothing was sent"
        );
    }
}

#[tokio::test]
async fn a_refused_exec_is_not_or2_pair() {
    let setup = with(Scenario {
        refuse_exec: true,
        ..Scenario::default()
    });
    assert_eq!(setup.enroll(STEP).await.unwrap_err(), PairError::NotOr2Pair);
}

#[tokio::test]
async fn a_hello_for_another_run_or_another_program_is_a_protocol_error() {
    let long = format!(
        "{{\"v\":2,\"hello\":\"or2-pair\",\"id\":\"{}\"}}\n",
        "x".repeat(300)
    );
    for (name, line) in [
        (
            "another id",
            String::from_utf8(hello_line("bcdefghijklmn")).unwrap(),
        ),
        (
            "another program",
            "{\"v\":2,\"hello\":\"other\",\"id\":\"abcdefghijklm\"}\n".to_owned(),
        ),
        ("not JSON", "{\"v\":2,\"hello\": nope\n".to_owned()),
        (
            "a missing id",
            "{\"v\":2,\"hello\":\"or2-pair\"}\n".to_owned(),
        ),
        ("a hello over 256 bytes", long),
    ] {
        let setup = with(scenario(vec![data(&line)], vec![]));
        assert_eq!(
            setup.enroll(STEP).await.unwrap_err(),
            PairError::Protocol,
            "{name}"
        );
        assert!(setup.observed().requests.lock().unwrap().is_empty());
    }
    // A hello of version 3 is not a hello this app skips to: it is noise, then nothing.
    let setup = with(scenario(
        vec![
            data("{\"v\":3,\"hello\":\"or2-pair\",\"id\":\"abcdefghijklm\"}\n"),
            Step::Close,
        ],
        vec![],
    ));
    assert_eq!(setup.enroll(STEP).await.unwrap_err(), PairError::NotOr2Pair);
}

#[tokio::test]
async fn a_bad_verdict_is_a_protocol_error_and_a_missing_one_is_a_lost_connection() {
    let big = format!(
        "{{\"v\":2,\"ok\":false,\"reason\":\"{}\"}}\n",
        "x".repeat(600)
    );
    for (name, text) in [
        ("not JSON", "ok\n".to_owned()),
        (
            "version 3",
            "{\"v\":3,\"ok\":false,\"reason\":\"gone\"}\n".to_owned(),
        ),
        (
            "no version",
            "{\"ok\":false,\"reason\":\"gone\"}\n".to_owned(),
        ),
        (
            "no user",
            "{\"v\":2,\"ok\":true,\"fingerprint\":\"F\"}\n".to_owned(),
        ),
        ("over 512 bytes", big),
    ] {
        let setup = answering(|_| data(&text));
        assert_eq!(
            setup.enroll(STEP).await.unwrap_err(),
            PairError::Protocol,
            "{name}"
        );
    }
    // Another key's fingerprint: the host installed something else.
    let setup = answering(|_| {
        data("{\"v\":2,\"ok\":true,\"user\":\"dev\",\"fingerprint\":\"SHA256:other\"}\n")
    });
    assert_eq!(setup.enroll(STEP).await.unwrap_err(), PairError::Protocol);
    // The command exits after the hello without answering.
    let setup = with(scenario(vec![hello()], vec![Step::Close]));
    assert_eq!(
        setup.enroll(STEP).await.unwrap_err(),
        PairError::ConnectionLost
    );
    // The verdict may share a packet with what follows it, and lines may end in CRLF.
    let setup = answering(|s| {
        let Step::Data(mut bytes) = s.ok() else {
            unreachable!()
        };
        bytes.insert(bytes.len() - 1, b'\r');
        Step::Data(bytes)
    });
    assert!(setup.enroll(STEP).await.is_ok());
}

#[tokio::test]
async fn a_different_code_is_refused_at_authentication_and_nothing_runs() {
    let setup = with(scenario(vec![hello()], vec![]));
    let wrong = PairCode::parse_typed("1111-0000-000A").unwrap();
    let result = pair_enroll(
        &setup.transport,
        &setup.offer,
        &wrong,
        &setup.phone_line(),
        "OnePlus",
        PairTiming { step: STEP },
    )
    .await;
    assert_eq!(result.unwrap_err(), PairError::BootstrapRefused);
    let observed = setup.observed();
    assert_eq!(observed.publickey_attempts.load(Ordering::SeqCst), 1);
    assert!(observed.commands.lock().unwrap().is_empty());
    assert!(observed.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn another_host_key_ends_the_connection_before_authentication() {
    let setup = with(scenario(vec![hello()], vec![]));
    // The server presents `host`; the code pinned another ed25519 key, then an ECDSA key the
    // server does not have at all.
    for pinned in [
        HostKey::from_openssh(HOST_KEY).unwrap(),
        HostKey::from_openssh(ECDSA_KEY).unwrap(),
    ] {
        let offer = PairOffer {
            host_key: pinned,
            ..setup.offer.clone()
        };
        let result = pair_enroll(
            &setup.transport,
            &offer,
            &setup.code,
            &setup.phone_line(),
            "OnePlus",
            PairTiming { step: STEP },
        )
        .await;
        assert_eq!(result.unwrap_err(), PairError::HostKeyMismatch);
    }
    let observed = setup.observed();
    assert_eq!(observed.publickey_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(observed.other_attempts.load(Ordering::SeqCst), 0);
    assert!(observed.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_silent_host_times_out_at_each_step() {
    // No hello after the exec.
    let setup = with(scenario(vec![], vec![]));
    let started = Instant::now();
    assert_eq!(setup.enroll(SHORT).await.unwrap_err(), PairError::TimedOut);
    assert!(started.elapsed() >= SHORT && started.elapsed() < SHORT * 5);
    // A hello and then no verdict.
    let setup = with(scenario(vec![hello()], vec![]));
    assert_eq!(setup.enroll(SHORT).await.unwrap_err(), PairError::TimedOut);
    assert!(
        !setup.observed().requests.lock().unwrap().is_empty(),
        "the request was sent"
    );
    // Noise that never ends in a hello is also a wait for the hello.
    let setup = with(scenario(vec![data("loading...\n")], vec![]));
    assert_eq!(setup.enroll(SHORT).await.unwrap_err(), PairError::TimedOut);
}

#[tokio::test]
async fn an_unreachable_host_is_unreachable_and_a_silent_one_times_out() {
    let setup = Setup::new(Scenario::default());
    for (kind, expected) in [
        (io::ErrorKind::ConnectionRefused, PairError::Unreachable),
        (io::ErrorKind::HostUnreachable, PairError::Unreachable),
        (io::ErrorKind::TimedOut, PairError::TimedOut),
    ] {
        let result = pair_enroll(
            &Arc::new(Failing(kind)),
            &setup.offer,
            &setup.code,
            &setup.phone_line(),
            "OnePlus",
            PairTiming { step: STEP },
        )
        .await;
        assert_eq!(result.unwrap_err(), expected, "{kind:?}");
    }
}

#[tokio::test]
async fn cancelling_the_pairing_closes_the_channel_through_the_connection() {
    // The host says hello and then waits for the verdict that never comes; the caller gives up.
    let setup = with(scenario(vec![hello()], vec![]));
    let outcome = tokio::time::timeout(Duration::from_millis(500), setup.enroll(STEP)).await;
    assert!(outcome.is_err(), "still waiting for the verdict");
    let observed = setup.observed();
    let deadline = Instant::now() + Duration::from_secs(5);
    while observed.channel_closes.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the channel was never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_finished_pairing_closes_its_channel_when_the_host_has_not() {
    // The host answers but leaves its channel open: the phone closes it.
    let setup = Setup::new(Scenario::default());
    let verdict = setup.ok();
    let setup = setup.running(scenario(vec![hello()], vec![verdict]));
    assert!(setup.enroll(STEP).await.is_ok());
    let observed = setup.observed();
    let deadline = Instant::now() + Duration::from_secs(5);
    while observed.channel_closes.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the channel was never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn inputs_are_checked_before_anything_is_connected() {
    let setup = with(scenario(vec![hello()], vec![]));
    let enroll = |offer: &PairOffer, key: &str, device: &str| {
        let transport = Arc::clone(&setup.transport);
        let (offer, key, device) = (offer.clone(), key.to_owned(), device.to_owned());
        let code = PairCode::parse_typed(CODE_TEXT).unwrap();
        async move {
            pair_enroll(
                &transport,
                &offer,
                &code,
                &key,
                &device,
                PairTiming { step: STEP },
            )
            .await
        }
    };
    let key = setup.phone_line();
    let manual = PairOffer {
        pairing_id: None,
        ..setup.offer.clone()
    };
    assert_eq!(
        enroll(&manual, &key, "d").await,
        Err(PairError::NoPairingId)
    );
    let bad_id = PairOffer {
        pairing_id: Some("not-an-id".into()),
        ..setup.offer.clone()
    };
    assert_eq!(
        enroll(&bad_id, &key, "d").await,
        Err(PairError::InvalidOffer)
    );
    let no_address = PairOffer {
        addresses: vec![],
        ..setup.offer.clone()
    };
    assert_eq!(
        enroll(&no_address, &key, "d").await,
        Err(PairError::InvalidOffer)
    );
    for bad in ["", "nonsense", "ssh-dss AAAAB3NzaC1kc3M="] {
        assert_eq!(
            enroll(&setup.offer, bad, "d").await,
            Err(PairError::InvalidKey),
            "{bad:?}"
        );
    }
    let long = "d".repeat(65);
    for bad in ["", "  ", "a\nb", long.as_str()] {
        assert_eq!(
            enroll(&setup.offer, &key, bad).await,
            Err(PairError::InvalidDevice),
            "{bad:?}"
        );
    }
    assert_eq!(setup.transport.connections.load(Ordering::SeqCst), 0);
}
