//! The pairing-code parser (table tests) and the exchange client against a scripted host.

use std::io;
use std::sync::Mutex;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::*;

const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
const HOST_FINGERPRINT: &str = "SHA256:PK/nvGiFusFK9/6Qf8dSOX99mI5XQYiMAML2JHCjgQI";
const ECDSA_KEY: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
const PHONE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
/// base32 of the bytes 0x00..0x0f.
const OTP_TEXT: &str = "AAAQEAYEAUDAOCAJBIFQYDIOB4";

fn otp() -> Otp {
    Otp::from_bytes(core::array::from_fn(|i| i as u8))
}

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
    format!("or2-pair:1?{}", query.join("&"))
}

fn full() -> Vec<(&'static str, &'static str)> {
    vec![
        ("name", "Work Mac"),
        ("user", "alice"),
        ("port", "22"),
        ("a", "100.101.102.103"),
        ("a", "192.168.1.20"),
        ("a", "work-mac.local"),
        ("hk", HOST_KEY),
        ("pair", "192.168.1.20:41234"),
        ("otp", OTP_TEXT),
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
fn a_full_code_parses_into_endpoints_and_a_key() {
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
            ("192.168.1.20".to_owned(), 22),
            ("work-mac.local".to_owned(), 22)
        ]
    );
    assert_eq!(offer.host_key.fingerprint(), HOST_FINGERPRINT);
    let exchange = offer.exchange.unwrap();
    assert_eq!(
        exchange.endpoints,
        [Endpoint::new("192.168.1.20", 41234).unwrap()]
    );
    assert_eq!(exchange.otp.0.as_slice(), &(0..16).collect::<Vec<u8>>()[..]);
}

#[test]
fn a_code_made_with_no_listen_has_no_exchange() {
    let fields = without(&without(&full(), "pair"), "otp");
    let offer = PairOffer::parse(&code(&fields)).unwrap();
    assert!(offer.exchange.is_none());
}

#[test]
fn several_pair_addresses_keep_their_order_and_ipv6_needs_brackets() {
    let mut fields = without(&full(), "pair");
    fields.push(("pair", "10.0.0.5:5000"));
    fields.push(("pair", "[fd00::1]:5000"));
    let offer = PairOffer::parse(&code(&fields)).unwrap();
    let endpoints = offer.exchange.unwrap().endpoints;
    assert_eq!(endpoints[0], Endpoint::new("10.0.0.5", 5000).unwrap());
    assert_eq!(endpoints[1], Endpoint::new("fd00::1", 5000).unwrap());
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
        "OR2-PAIR:1?x=y",
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
fn versions_other_than_one_are_named_unsupported_not_malformed() {
    for text in ["or2-pair:2?name=x", "or2-pair:10?name=x", "or2-pair:0?x=y"] {
        assert_eq!(
            PairOffer::parse(text).unwrap_err(),
            PairParseError::UnsupportedVersion
        );
    }
    for text in [
        "or2-pair:?name=x",
        "or2-pair:1.1?name=x",
        "or2-pair:v1?name=x",
        "or2-pair:1",
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
fn every_required_field_is_required() {
    for (field, expected) in [
        ("name", PairParseError::MissingField("name")),
        ("user", PairParseError::MissingField("user")),
        ("port", PairParseError::MissingField("port")),
        ("a", PairParseError::MissingField("a")),
        ("hk", PairParseError::MissingField("hk")),
        ("pair", PairParseError::MissingField("pair")),
        ("otp", PairParseError::MissingField("otp")),
    ] {
        assert_eq!(
            PairOffer::parse(&code(&without(&full(), field))).unwrap_err(),
            expected,
            "{field}"
        );
    }
}

#[test]
fn single_fields_may_not_repeat_and_unknown_fields_are_refused() {
    for field in ["name", "user", "port", "hk", "otp"] {
        let mut fields = full();
        let again = *fields.iter().find(|(k, _)| *k == field).unwrap();
        fields.push(again);
        let expected = match field {
            "name" => "name",
            "user" => "user",
            "port" => "port",
            "hk" => "hk",
            _ => "otp",
        };
        assert_eq!(
            PairOffer::parse(&code(&fields)).unwrap_err(),
            PairParseError::DuplicateField(expected)
        );
    }
    let mut fields = full();
    fields.push(("extra", "1"));
    assert_eq!(
        PairOffer::parse(&code(&fields)).unwrap_err(),
        PairParseError::UnknownField
    );
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
        "or2-pair:1?".to_owned(),
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
        ("192.168.1.1", true),
        ("host.local", true),
        ("fe80::1", true),
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
fn the_otp_is_exactly_26_canonical_base32_characters() {
    for (value, valid) in [
        (OTP_TEXT, true),
        ("aaaqeayeaudaocajbifqydiob4", false),
        ("AAAQEAYEAUDAOCAJBIFQYDIOB", false),
        ("AAAQEAYEAUDAOCAJBIFQYDIOB4A", false),
        ("AAAQEAYEAUDAOCAJBIFQYDIOB=", false),
        ("AAAQEAYEAUDAOCAJBIFQYDIOB1", false),
        ("AAAQEAYEAUDAOCAJBIFQYDIOB8", false),
        // The last character carries two bits too many: not the canonical encoding.
        ("AAAQEAYEAUDAOCAJBIFQYDIOB5", false),
        ("", false),
    ] {
        let parsed = PairOffer::parse(&code(&replaced(&full(), "otp", value)));
        assert_eq!(parsed.is_ok(), valid, "{value:?}");
    }
}

#[test]
fn pair_addresses_are_ip_literals_with_a_listener_port() {
    for (value, valid) in [
        ("192.168.1.20:41234", true),
        ("[fd00::1]:5000", true),
        ("100.64.0.1:1", true),
        ("host.local:5000", false),
        ("192.168.1.20", false),
        ("192.168.1.20:0", false),
        ("192.168.1.20:65536", false),
        ("192.168.1.20:x", false),
        ("0.0.0.0:5000", false),
        ("[::]:5000", false),
        ("224.0.0.1:5000", false),
        ("255.255.255.255:5000", false),
        ("fd00::1:5000", false),
        ("[192.168.1.1]:5000", false),
        ("[fd00::1]5000", false),
        (":5000", false),
        ("", false),
    ] {
        let parsed = PairOffer::parse(&code(&replaced(&full(), "pair", value)));
        assert_eq!(parsed.is_ok(), valid, "{value:?}");
    }
    let mut fields = without(&full(), "pair");
    for port in 1..=5 {
        fields.push((
            "pair",
            [
                "10.0.0.1:1",
                "10.0.0.1:2",
                "10.0.0.1:3",
                "10.0.0.1:4",
                "10.0.0.1:5",
            ][port - 1],
        ));
    }
    assert_eq!(
        PairOffer::parse(&code(&fields)).unwrap_err(),
        PairParseError::InvalidField("pair")
    );
}

#[test]
fn pair_and_otp_come_together() {
    let only_otp = without(&full(), "pair");
    assert_eq!(
        PairOffer::parse(&code(&only_otp)).unwrap_err(),
        PairParseError::MissingField("pair")
    );
}

#[test]
fn debug_output_never_contains_the_password() {
    let offer = PairOffer::parse(&code(&full())).unwrap();
    let shown = format!("{offer:?} {:?}", offer.exchange.as_ref().unwrap().otp);
    assert!(!shown.contains(OTP_TEXT), "{shown}");
    assert!(shown.contains("redacted"));
    assert!(!shown.contains("[0, 1, 2"), "{shown}");
}

#[test]
fn the_macs_match_an_independent_hmac_sha256() {
    // Computed with an independent HMAC-SHA256 implementation: key 00..0f, nonce 20..3f.
    //   request: b"or2-pair/3 request\0" + nonce + key line
    //   verdict: b"or2-pair/3 verdict\0" + [ok] + lp(reason) + lp(nonce) + lp(fingerprint)
    //   with lp(x) = big-endian u16 length of x, then x
    let nonce: Vec<u8> = (0x20..0x40).collect();
    assert_eq!(
        STANDARD.encode(otp().request_mac(&nonce, PHONE_KEY)),
        "XLn9M1XO1Mx6S7YuGYz9NfVoz9LYtdTtPq0L4MbcUsw="
    );
    for (ok, reason, fingerprint, expected) in [
        (
            true,
            "",
            "SHA256:abc",
            "+eZyKeIYHHI/zf3VGTkfgL6oZO1bDySyNjDL31Z0iak=",
        ),
        (
            false,
            "declined",
            "SHA256:abc",
            "kv1uJPEuHGLlwbkD7ehkR0JRChE2ST3oeztTcxBO18Q=",
        ),
        (
            false,
            "authentication",
            "",
            "fCheMVf7/YA4f+s7BLO3+5kR3hvTNzHOTqD2oCz9ksY=",
        ),
    ] {
        assert_eq!(
            STANDARD.encode(otp().verdict_mac(&nonce, ok, reason, fingerprint)),
            expected,
            "{ok} {reason} {fingerprint}"
        );
    }
}

#[test]
fn no_two_outcomes_share_a_verdict_mac() {
    // Review of 6afa42e: v2 gave `ok:true` and `ok:false,reason:"ok"` the same MAC. Every
    // distinct (ok, reason, nonce, fingerprint) tuple, including ones that would collide under a
    // separator-only encoding, must now have its own.
    let nonce = [9u8; 32];
    let mut seen = std::collections::HashSet::new();
    for (ok, reason, fingerprint) in [
        (true, "", "SHA256:abc"),
        (false, "ok", "SHA256:abc"),
        (false, "", "SHA256:abc"),
        (false, "declined", "SHA256:abc"),
        (false, "declinedSHA256:abc", ""),
        (false, "declined\0SHA256:abc", ""),
        (false, "declined\0", "SHA256:abc"),
        (false, "", "declined\0SHA256:abc"),
        (true, "", "ok\0SHA256:abc"),
        (true, "ok", "SHA256:abc"),
    ] {
        assert!(
            seen.insert(otp().verdict_mac(&nonce, ok, reason, fingerprint)),
            "{ok} {reason:?} {fingerprint:?}"
        );
    }
}

#[test]
fn the_request_and_verdict_domains_are_distinct() {
    // No key text makes a request MAC equal a verdict MAC (and vice versa): the domains differ.
    let nonce = [9u8; 32];
    let verdict = otp().verdict_mac(&nonce, true, "", "SHA256:abc");
    let request = otp().request_mac(&nonce, "\x01\0\0\0\x20SHA256:abc");
    assert_ne!(verdict, request);
    assert!(!otp().verify_verdict(&nonce, true, "", "SHA256:abc", &request));
}

#[test]
fn a_wiped_password_no_longer_matches() {
    let nonce = [7u8; 32];
    let before = otp().request_mac(&nonce, PHONE_KEY);
    let mut wiped = otp();
    wiped.wipe();
    assert_ne!(wiped.request_mac(&nonce, PHONE_KEY), before);
    assert!(!wiped.verify_verdict(
        &nonce,
        true,
        "",
        "SHA256:x",
        &otp().verdict_mac(&nonce, true, "", "SHA256:x")
    ));
}

// --- the exchange --------------------------------------------------------------------------------

/// Hands out pre-made duplex streams, one per connect, to the endpoints in the order asked.
struct Pipes {
    streams: Mutex<Vec<DuplexStream>>,
    asked: Mutex<Vec<Endpoint>>,
}

impl Pipes {
    fn new(streams: Vec<DuplexStream>) -> Arc<Self> {
        Arc::new(Self {
            streams: Mutex::new(streams),
            asked: Mutex::default(),
        })
    }
}

impl Transport for Pipes {
    type Stream = DuplexStream;

    async fn connect(&self, endpoint: &Endpoint) -> io::Result<DuplexStream> {
        self.asked.lock().unwrap().push(endpoint.clone());
        self.streams
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

fn offer() -> PairOffer {
    PairOffer::parse(&code(&full())).unwrap()
}

fn quick() -> PairTiming {
    PairTiming {
        step: Duration::from_secs(10),
        verdict: Duration::from_secs(125),
    }
}

/// What a scripted host does after sending its hello.
#[derive(Clone)]
enum Host {
    /// Answers as the real host does: `None` is a success, `Some(reason)` a refusal, each with
    /// the verdict MAC the real password gives for this nonce and the key that was sent.
    Verdict(Option<&'static str>),
    /// Replies to whatever came with this raw text.
    Raw(&'static str),
    /// Sends the hello, reads the request, then says nothing for this long.
    Silent(u64),
    /// Closes right after reading the request.
    Hangup,
}

struct Seen {
    request: serde_json::Value,
    mac_ok: bool,
}

fn host(script: Host, hello: String) -> (DuplexStream, tokio::task::JoinHandle<Option<Seen>>) {
    let (client, server) = tokio::io::duplex(4096);
    let task = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.write_all(hello.as_bytes()).await.ok()?;
        let mut line = String::new();
        server.read_line(&mut line).await.ok()?;
        let request: serde_json::Value = serde_json::from_str(&line).ok()?;
        let nonce = STANDARD.decode(NONCE_B64).unwrap();
        let key = request["key"].as_str()?.to_owned();
        let expected = otp().request_mac(&nonce, &key);
        let mac_ok = request["mac"].as_str() == Some(STANDARD.encode(expected).as_str());
        let seen = Seen { request, mac_ok };
        match script {
            Host::Verdict(reason) => {
                let reply = signed(reason, &nonce, &fingerprint_of(&key).ok()?);
                server.write_all(reply.as_bytes()).await.ok()?;
            }
            Host::Raw(raw) => {
                server.write_all(raw.as_bytes()).await.ok()?;
            }
            Host::Silent(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
            Host::Hangup => {}
        }
        Some(seen)
    });
    (client, task)
}

/// 32 bytes 0x20..0x3f in standard base64.
const NONCE_B64: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";

fn hello() -> String {
    format!("{{\"v\":3,\"nonce\":\"{NONCE_B64}\"}}\n")
}

/// The nonce of [`hello`].
fn nonce() -> Vec<u8> {
    STANDARD.decode(NONCE_B64).unwrap()
}

/// A verdict line as the real host writes it: `None` for success, `Some(reason)` for a refusal.
fn signed(reason: Option<&str>, nonce: &[u8], fingerprint: &str) -> String {
    let mac = otp().verdict_mac(
        nonce,
        reason.is_none(),
        reason.unwrap_or_default(),
        fingerprint,
    );
    match reason {
        None => format!("{{\"ok\":true,\"mac\":\"{}\"}}\n", STANDARD.encode(mac)),
        Some(reason) => format!(
            "{{\"ok\":false,\"reason\":\"{reason}\",\"mac\":\"{}\"}}\n",
            STANDARD.encode(mac)
        ),
    }
}

/// [`signed`] for [`PHONE_KEY`] and [`hello`]'s nonce, as a `&'static str` for [`Host::Raw`].
fn signed_for_phone(reason: Option<&str>) -> &'static str {
    let fingerprint = fingerprint_of(PHONE_KEY).unwrap();
    Box::leak(signed(reason, &nonce(), &fingerprint).into_boxed_str())
}

async fn run(script: Host, hello: String) -> (Result<(), PairError>, Option<Seen>) {
    let (client, task) = host(script, hello);
    let result = converse(client, &otp(), PHONE_KEY, "Pixel", quick()).await;
    (result, task.await.unwrap())
}

#[tokio::test(start_paused = true)]
async fn a_good_exchange_sends_the_key_and_a_mac_the_host_can_verify() {
    let (result, seen) = run(Host::Verdict(None), hello()).await;
    assert_eq!(result, Ok(()));
    let seen = seen.unwrap();
    assert!(seen.mac_ok);
    assert_eq!(seen.request["v"], 3);
    assert_eq!(seen.request["key"], PHONE_KEY);
    assert_eq!(seen.request["device"], "Pixel");
    // The password itself is nowhere in the request.
    assert!(!seen.request.to_string().contains(OTP_TEXT));
}

#[tokio::test(start_paused = true)]
async fn a_responder_that_does_not_know_the_password_cannot_forge_a_verdict() {
    // Finding 5: an active attacker on the path replies to whatever the phone sent, without the
    // password and without the real host. Neither a success nor a refusal may be taken for one.
    let other_nonce = [3u8; 32];
    let fingerprint = fingerprint_of(PHONE_KEY).unwrap();
    let other_key = fingerprint_of(HOST_KEY).unwrap();
    // The request MAC the phone itself sent, replayed back as a verdict.
    let reflected = STANDARD.encode(otp().request_mac(&nonce(), PHONE_KEY));
    let forgeries: Vec<(&str, String)> = vec![
        ("success, no proof", "{\"ok\":true}\n".to_owned()),
        (
            "refusal, no proof",
            "{\"ok\":false,\"reason\":\"declined\"}\n".to_owned(),
        ),
        (
            "wrong-code refusal, no proof",
            "{\"ok\":false,\"reason\":\"authentication\"}\n".to_owned(),
        ),
        (
            "success, garbage proof",
            "{\"ok\":true,\"mac\":\"AAAA\"}\n".to_owned(),
        ),
        (
            "success, proof that is not base64",
            "{\"ok\":true,\"mac\":\"!!!\"}\n".to_owned(),
        ),
        (
            "a refusal's proof on a success",
            signed(Some("declined"), &nonce(), &fingerprint)
                .replace("\"ok\":false,\"reason\":\"declined\"", "\"ok\":true"),
        ),
        (
            "a success's proof on a refusal",
            signed(None, &nonce(), &fingerprint)
                .replace("\"ok\":true", "\"ok\":false,\"reason\":\"declined\""),
        ),
        (
            "a proof replayed from another exchange",
            signed(None, &other_nonce, &fingerprint),
        ),
        (
            "a proof about another key",
            signed(None, &nonce(), &other_key),
        ),
        (
            "the phone's own request MAC sent back",
            format!("{{\"ok\":true,\"mac\":\"{reflected}\"}}\n"),
        ),
    ];
    for (what, forged) in forgeries {
        let forged: &'static str = Box::leak(forged.into_boxed_str());
        let (result, _) = run(Host::Raw(forged), hello()).await;
        assert_eq!(
            result,
            Err(PairError::HostNotAuthenticated),
            "{what}: {forged}"
        );
    }
    // And the real thing, for contrast.
    let (result, _) = run(Host::Raw(signed_for_phone(None)), hello()).await;
    assert_eq!(result, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn a_success_proof_cannot_be_relabelled_as_a_refusal_or_the_reverse() {
    // Review of 6afa42e: with the v2 MAC, `ok:true` and `ok:false,reason:"ok"` shared a MAC, so a
    // proxy could turn a signed success into a "refusal" (and the phone spent its code although the
    // key was installed). The verdict encoding is now unambiguous and the shape must match it.
    let fingerprint = fingerprint_of(PHONE_KEY).unwrap();
    let success = signed(None, &nonce(), &fingerprint);
    let mac = success
        .split("\"mac\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();
    let refusal = signed(Some("declined"), &nonce(), &fingerprint);
    let refusal_mac = refusal
        .split("\"mac\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap()
        .to_owned();
    let forgeries = [
        // The reproduced substitution: a success's MAC under a refusal with reason "ok".
        format!("{{\"ok\":false,\"reason\":\"ok\",\"mac\":\"{mac}\"}}\n"),
        // The same with no reason at all, and with the empty reason.
        format!("{{\"ok\":false,\"mac\":\"{mac}\"}}\n"),
        format!("{{\"ok\":false,\"reason\":\"\",\"mac\":\"{mac}\"}}\n"),
        // A refusal's MAC under a success that carries the refusal's reason, or any reason.
        format!("{{\"ok\":true,\"reason\":\"declined\",\"mac\":\"{refusal_mac}\"}}\n"),
        format!("{{\"ok\":true,\"reason\":\"ok\",\"mac\":\"{mac}\"}}\n"),
        // A refusal's MAC under another reason.
        format!("{{\"ok\":false,\"reason\":\"busy\",\"mac\":\"{refusal_mac}\"}}\n"),
    ];
    for forged in forgeries {
        let forged: &'static str = Box::leak(forged.into_boxed_str());
        let (result, _) = run(Host::Raw(forged), hello()).await;
        assert_eq!(result, Err(PairError::HostNotAuthenticated), "{forged}");
    }
    // Fields that were not MAC'd are not tolerated either.
    let extra: &'static str =
        Box::leak(format!("{{\"ok\":true,\"mac\":\"{mac}\",\"extra\":1}}\n").into_boxed_str());
    let (result, _) = run(Host::Raw(extra), hello()).await;
    assert_eq!(result, Err(PairError::Protocol));
    let (result, _) = run(Host::Raw(signed_for_phone(None)), hello()).await;
    assert_eq!(result, Ok(()));
}

#[tokio::test(start_paused = true)]
async fn each_refusal_reason_has_its_own_error() {
    for (reason, refusal) in [
        ("declined", Refusal::Declined),
        ("authentication", Refusal::AuthenticationFailed),
        ("key", Refusal::KeyNotAccepted),
        ("timeout", Refusal::TimedOut),
        ("request", Refusal::BadRequest),
        ("failed", Refusal::HostFailed),
        ("busy", Refusal::Other),
        ("who knows", Refusal::Other),
    ] {
        let (result, _) = run(Host::Verdict(Some(reason)), hello()).await;
        assert_eq!(result, Err(PairError::Refused(refusal)), "{reason}");
    }
    // A refusal with no reason at all is signed over the empty verdict.
    let fingerprint = fingerprint_of(PHONE_KEY).unwrap();
    let mac = STANDARD.encode(otp().verdict_mac(&nonce(), false, "", &fingerprint));
    let bare: &'static str =
        Box::leak(format!("{{\"ok\":false,\"mac\":\"{mac}\"}}\n").into_boxed_str());
    let (result, _) = run(Host::Raw(bare), hello()).await;
    assert_eq!(result, Err(PairError::Refused(Refusal::Other)));
}

#[tokio::test(start_paused = true)]
async fn a_bad_hello_is_a_protocol_error() {
    for text in [
        // Older hosts (version 1 had no authenticated verdict, 2 an ambiguous one) and a newer one.
        "{\"v\":1,\"nonce\":\"ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=\"}\n".to_owned(),
        "{\"v\":2,\"nonce\":\"ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=\"}\n".to_owned(),
        "{\"v\":4,\"nonce\":\"ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=\"}\n".to_owned(),
        "{\"v\":3,\"nonce\":\"AAAA\"}\n".to_owned(),
        "{\"v\":3,\"nonce\":\"!!!\"}\n".to_owned(),
        "{\"v\":3}\n".to_owned(),
        "not json\n".to_owned(),
        "\n".to_owned(),
        // Longer than the bound, never ending.
        "x".repeat(HELLO_LIMIT + 1),
        // Longer than the bound with the newline in it.
        format!("{}\n", "x".repeat(HELLO_LIMIT + 1)),
    ] {
        let (client, _task) = host(Host::Hangup, text.clone());
        let result = converse(client, &otp(), PHONE_KEY, "Pixel", quick()).await;
        assert_eq!(
            result,
            Err(PairError::Protocol),
            "{}",
            &text[..text.len().min(40)]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_garbled_verdict_is_a_protocol_error_and_a_hangup_is_a_lost_connection() {
    for raw in [
        "nope\n",
        "{\"reason\":\"x\"}\n",
        &format!("{}\n", "x".repeat(REPLY_LIMIT + 1)),
    ] {
        let (result, _) = run(
            Host::Raw(Box::leak(raw.to_owned().into_boxed_str())),
            hello(),
        )
        .await;
        assert_eq!(result, Err(PairError::Protocol));
    }
    let (result, _) = run(Host::Hangup, hello()).await;
    assert_eq!(result, Err(PairError::ConnectionLost));
}

#[tokio::test(start_paused = true)]
async fn a_silent_host_times_out_at_each_step() {
    // No hello within 10 s.
    let (client, _server) = tokio::io::duplex(64);
    let started = tokio::time::Instant::now();
    let result = converse(client, &otp(), PHONE_KEY, "Pixel", quick()).await;
    assert_eq!(result, Err(PairError::TimedOut));
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    // A hello, then no verdict: the wait is the verdict limit, not the step limit.
    let (client, task) = host(Host::Silent(1000), hello());
    let started = tokio::time::Instant::now();
    let result = converse(client, &otp(), PHONE_KEY, "Pixel", quick()).await;
    assert_eq!(result, Err(PairError::TimedOut));
    assert_eq!(started.elapsed(), Duration::from_secs(125));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_slow_confirmation_inside_the_window_still_succeeds() {
    let (client, server) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        let mut server = BufReader::new(server);
        server.write_all(hello().as_bytes()).await.unwrap();
        let mut line = String::new();
        server.read_line(&mut line).await.unwrap();
        tokio::time::sleep(Duration::from_secs(100)).await;
        server
            .write_all(signed_for_phone(None).as_bytes())
            .await
            .unwrap();
    });
    assert_eq!(
        converse(client, &otp(), PHONE_KEY, "Pixel", quick()).await,
        Ok(())
    );
}

#[tokio::test(start_paused = true)]
async fn submit_validates_before_it_connects() {
    let transport = Pipes::new(vec![]);
    let bare = PairOffer::parse(&code(&without(&without(&full(), "pair"), "otp"))).unwrap();
    assert_eq!(
        submit_key(&transport, &bare, PHONE_KEY, "Pixel", quick()).await,
        Err(PairError::NoExchange)
    );
    for key in [
        "",
        "garbage",
        "ssh-ed25519 AAAA",
        "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7AAAABHNzaDo=",
    ] {
        assert_eq!(
            submit_key(&transport, &offer(), key, "Pixel", quick()).await,
            Err(PairError::InvalidKey),
            "{key:?}"
        );
    }
    for device in ["", "  ", "a\nb", &"d".repeat(65)] {
        assert_eq!(
            submit_key(&transport, &offer(), PHONE_KEY, device, quick()).await,
            Err(PairError::InvalidDevice)
        );
    }
    assert!(transport.asked.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn submit_sends_a_clean_key_without_its_comment_through_the_transport() {
    let (client, task) = host(Host::Verdict(None), hello());
    let transport = Pipes::new(vec![client]);
    let result = submit_key(
        &transport,
        &offer(),
        &format!("{PHONE_KEY} phone@pixel\n"),
        " Pixel 8 ",
        quick(),
    )
    .await;
    assert_eq!(result, Ok(()));
    let seen = task.await.unwrap().unwrap();
    assert_eq!(seen.request["key"], PHONE_KEY);
    assert_eq!(seen.request["device"], "Pixel 8");
    assert!(seen.mac_ok);
    assert_eq!(
        *transport.asked.lock().unwrap(),
        [Endpoint::new("192.168.1.20", 41234).unwrap()]
    );
}

#[tokio::test(start_paused = true)]
async fn submit_with_no_reachable_address_is_unreachable_and_a_hang_is_timed_out() {
    let result = submit_key(&Pipes::new(vec![]), &offer(), PHONE_KEY, "Pixel", quick()).await;
    assert_eq!(result, Err(PairError::Unreachable));

    struct Hang;
    impl Transport for Hang {
        type Stream = DuplexStream;
        async fn connect(&self, _: &Endpoint) -> io::Result<DuplexStream> {
            std::future::pending().await
        }
    }
    let result = submit_key(&Arc::new(Hang), &offer(), PHONE_KEY, "Pixel", quick()).await;
    assert_eq!(result, Err(PairError::TimedOut));
}

#[tokio::test(start_paused = true)]
async fn submit_races_the_pair_addresses_and_uses_the_one_that_answers() {
    let mut fields = without(&full(), "pair");
    fields.push(("pair", "10.0.0.1:1111"));
    fields.push(("pair", "10.0.0.2:2222"));
    let offer = PairOffer::parse(&code(&fields)).unwrap();

    struct Second(Mutex<Option<DuplexStream>>);
    impl Transport for Second {
        type Stream = DuplexStream;
        async fn connect(&self, endpoint: &Endpoint) -> io::Result<DuplexStream> {
            if endpoint.port() == 1111 {
                return Err(io::Error::from(io::ErrorKind::HostUnreachable));
            }
            Ok(self.0.lock().unwrap().take().unwrap())
        }
    }
    let (client, task) = host(Host::Verdict(None), hello());
    let transport = Arc::new(Second(Mutex::new(Some(client))));
    assert_eq!(
        submit_key(&transport, &offer, PHONE_KEY, "Pixel", quick()).await,
        Ok(())
    );
    assert!(task.await.unwrap().unwrap().mac_ok);
}
