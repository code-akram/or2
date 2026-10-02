//! The bootstrap key and the `authorized_keys` line that carries it.
//!
//! `seed = HKDF-SHA256(ikm = the 11 data characters of K as ASCII uppercase, salt = the pairing
//! id as its 13 ASCII characters, info = "or2-pair/2 bootstrap ed25519")`, 32 bytes, is the
//! Ed25519 secret key (RFC 8032 seed). The host needs only the public half; the phone derives the
//! same key from the code it shows and the id in the QR. Salting with the id binds the key to one
//! run. See `docs/contracts.md`, "The bootstrap key".

use std::path::Path;

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::code::PairCode;
use crate::date::DateTime;
use crate::keyline::KeyLine;

/// The HKDF `info` of the bootstrap key.
pub const INFO: &[u8] = b"or2-pair/2 bootstrap ed25519";

/// How long the bootstrap entry is honoured by the forced command: 5 minutes from the moment it
/// is written.
pub const WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// What `expiry-time` adds to the window, so that sshd's own expiry never ends a pairing the
/// forced command would still accept.
pub const EXPIRY_SLACK: std::time::Duration = std::time::Duration::from_secs(600);

/// The comment of the bootstrap entry, followed by the pairing id: how the sweep finds them.
pub const COMMENT_PREFIX: &str = "or2-pair-bootstrap-";

/// A pairing id: 8 random bytes as exactly 13 lowercase RFC 4648 base32 characters (no padding).
/// Not secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a pairing id is 13 lowercase base32 characters (a-z, 2-7)")]
pub struct BadId;

impl PairingId {
    pub fn generate(random: &dyn Fn(&mut [u8])) -> Self {
        let mut bytes = [0u8; 8];
        random(&mut bytes);
        Self(BASE32_NOPAD.encode(&bytes).to_ascii_lowercase())
    }

    pub fn parse(text: &str) -> Result<Self, BadId> {
        let valid =
            text.len() == 13 && text.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'));
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(BadId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PairingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The bootstrap key's seed. The phone's side of this is the secret key; the host only keeps
/// the public half and zeroizes this at once.
pub fn seed(code: &PairCode, id: &PairingId) -> Zeroizing<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(id.as_str().as_bytes()), code.data());
    let mut seed = Zeroizing::new([0u8; 32]);
    hkdf.expand(INFO, &mut *seed)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    seed
}

/// The bootstrap key's public half as an OpenSSH `ssh-ed25519` key.
pub fn public_key(code: &PairCode, id: &PairingId) -> KeyLine {
    public_key_of_seed(&seed(code, id))
}

fn public_key_of_seed(seed: &[u8; 32]) -> KeyLine {
    use base64::Engine as _;
    let public = SigningKey::from_bytes(seed).verifying_key().to_bytes();
    let mut blob = Vec::with_capacity(51);
    blob.extend_from_slice(&11u32.to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&32u32.to_be_bytes());
    blob.extend_from_slice(&public);
    let line = format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    );
    KeyLine::parse(&line).expect("an Ed25519 public key is a valid key line")
}

// --- sshd's version and the options its authorized_keys understands -------------------------

/// Which options this sshd's `authorized_keys` knows. An option sshd does not know makes it
/// ignore the whole line, so only these three spellings are ever written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// OpenSSH 7.7 or newer (`expiry-time`: OpenSSH 7.7 release notes, 2018-04-02).
    Expiry,
    /// OpenSSH 7.2 up to 7.7 (`restrict`: OpenSSH 7.2 release notes, 2016-02-29).
    Restrict,
    /// Older OpenSSH.
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DialectError {
    #[error("sshd did not show a version banner")]
    NoBanner,
    #[error("the SSH server here is not OpenSSH ({0})")]
    NotOpenSsh(String),
}

/// `(major, minor)` of `SSH-2.0-OpenSSH_9.8p1 Ubuntu-1`, or the software name when it is not
/// OpenSSH.
pub fn openssh_version(banner: &str) -> Result<(u32, u32), DialectError> {
    let banner = banner.trim();
    let software = banner
        .strip_prefix("SSH-")
        .and_then(|rest| rest.split_once('-'))
        .map(|(_, software)| software)
        .ok_or(DialectError::NoBanner)?;
    let software = software.split_whitespace().next().unwrap_or_default();
    let version = software
        .strip_prefix("OpenSSH_")
        .ok_or_else(|| DialectError::NotOpenSsh(software.to_owned()))?;
    let number = |text: &str| -> Option<u32> {
        let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let (major, rest) = version
        .split_once('.')
        .ok_or_else(|| DialectError::NotOpenSsh(software.to_owned()))?;
    match (number(major), number(rest)) {
        (Some(major), Some(minor)) => Ok((major, minor)),
        _ => Err(DialectError::NotOpenSsh(software.to_owned())),
    }
}

/// The options to use for the sshd that sent `banner` (`None`: nothing could be read).
pub fn dialect(banner: Option<&str>) -> Result<Dialect, DialectError> {
    let (major, minor) = openssh_version(banner.ok_or(DialectError::NoBanner)?)?;
    Ok(if (major, minor) >= (7, 7) {
        Dialect::Expiry
    } else if (major, minor) >= (7, 2) {
        Dialect::Restrict
    } else {
        Dialect::Legacy
    })
}

impl Dialect {
    /// What to tell the person when the file itself cannot expire the key.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Expiry => None,
            Self::Restrict | Self::Legacy => Some(
                "This sshd is older than OpenSSH 7.7 and cannot expire the key in the file: or2-pair removes it when it ends, and the pairing command itself stops answering after 5 minutes.",
            ),
        }
    }
}

// --- the command sshd runs, and who runs it ------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExeError {
    #[error(
        "the path of this program ({path}) has {what}, which sshd's login shell would treat specially: pairing runs `<this program> enroll` through it, so the path may only use A-Z a-z 0-9 / . _ + - . Install or2-pair somewhere else (for example ~/.local/bin or ~/.cargo/bin) and run it from there"
    )]
    Character { path: String, what: String },
    #[error("the path of this program is not an absolute path of plain text: {0}")]
    NotAbsolute(String),
}

/// The executable's path as it goes into `command="…"`: absolute and made only of
/// `A-Za-z0-9/._+-`, which needs no quoting in sh, bash, zsh or fish.
pub fn check_exe_path(path: &Path) -> Result<String, ExeError> {
    let Some(text) = path.to_str() else {
        return Err(ExeError::NotAbsolute(path.display().to_string()));
    };
    if !path.is_absolute() {
        return Err(ExeError::NotAbsolute(text.to_owned()));
    }
    match text
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '+' | '-')))
    {
        None => Ok(text.to_owned()),
        Some(c) => Err(ExeError::Character {
            path: text.to_owned(),
            what: if c == ' ' {
                "a space".to_owned()
            } else if c.is_control() {
                "a control character".to_owned()
            } else {
                format!("the character {c:?}")
            },
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the account's login shell ({0}) cannot run commands, so sshd could not start the pairing command; use --manual, or run or2-pair as an account with a working shell"
)]
pub struct ShellError(pub String);

/// Refuses the login shells that cannot run a command (`nologin`, `false`).
pub fn check_login_shell(shell: Option<&str>) -> Result<(), ShellError> {
    let Some(shell) = shell else { return Ok(()) };
    let name = shell.rsplit('/').next().unwrap_or(shell);
    if matches!(name, "nologin" | "false") {
        Err(ShellError(shell.to_owned()))
    } else {
        Ok(())
    }
}

/// The one `authorized_keys` line of a run. `expires` is the host's local wall-clock time.
pub fn line(
    dialect: Dialect,
    exe: &str,
    id: &PairingId,
    key: &KeyLine,
    expires: &DateTime,
) -> String {
    let command = format!("command=\"{exe} enroll {id}\"");
    let options = match dialect {
        Dialect::Expiry => format!(
            "restrict,{command},expiry-time=\"{}\"",
            expires.compact_minutes()
        ),
        Dialect::Restrict => format!("restrict,{command}"),
        Dialect::Legacy => format!(
            "{command},no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-user-rc"
        ),
    };
    format!("{options} {} {COMMENT_PREFIX}{id}", key.openssh())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The derivation vector, computed with OpenSSL, not with this crate. K's data characters
    // `7KQ4M2XD9PT` (full code `7KQ4-M2XD-9PTM`), id `abcdefghijklm`:
    //
    //   openssl kdf -keylen 32 -kdfopt digest:SHA256 -kdfopt key:7KQ4M2XD9PT \
    //     -kdfopt salt:abcdefghijklm -kdfopt "info:or2-pair/2 bootstrap ed25519" \
    //     -kdfopt mode:EXTRACT_AND_EXPAND -binary HKDF | xxd -p -c64
    //
    // then the Ed25519 public key of that seed, from a PKCS#8 DER (the fixed prefix
    // `302e020100300506032b657004220420` followed by the seed) read by `openssl pkey`:
    //
    //   openssl pkey -inform DER -in seed.der -pubout -outform DER | tail -c 32 | xxd -p -c64
    //
    // and the OpenSSH form, checked with `ssh-keygen -lf` (which prints the fingerprint below).
    pub const VECTOR_CODE: &str = "7KQ4-M2XD-9PTM";
    pub const VECTOR_ID: &str = "abcdefghijklm";
    pub const VECTOR_SEED: &str =
        "734c24be4849a8de10227c52bd2530221cd6d2e62062c650886f0db0b99aeed0";
    pub const VECTOR_PUBLIC: &str =
        "02d8bd7aee56213d1bcdd3f38649a9b749904226955d0d460f400ba630e0d628";
    pub const VECTOR_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIALYvXruViE9G83T84ZJqbdJkEImlV0NRg9AC6Yw4NYo";
    pub const VECTOR_FINGERPRINT: &str = "SHA256:SQBrFEwlegpfzBKzGHE7T/RYggIFZfhD6+32GxJok20";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_derivation_matches_the_independent_vector() {
        let code = PairCode::parse(VECTOR_CODE).unwrap();
        let id = PairingId::parse(VECTOR_ID).unwrap();
        assert_eq!(hex(&*seed(&code, &id)), VECTOR_SEED);
        let key = public_key(&code, &id);
        assert_eq!(key.openssh(), VECTOR_KEY);
        assert_eq!(key.fingerprint(), VECTOR_FINGERPRINT);
        let public = SigningKey::from_bytes(&seed(&code, &id)).verifying_key();
        assert_eq!(hex(public.as_bytes()), VECTOR_PUBLIC);
    }

    #[test]
    fn another_id_or_code_gives_an_unrelated_key() {
        let code = PairCode::parse(VECTOR_CODE).unwrap();
        let other_id = PairingId::parse("abcdefghijkln").unwrap();
        let id = PairingId::parse(VECTOR_ID).unwrap();
        assert_ne!(public_key(&code, &id), public_key(&code, &other_id));
        let other = PairCode::parse("0000-0000-0000").unwrap();
        assert_ne!(public_key(&code, &id), public_key(&other, &id));
    }

    #[test]
    fn ids_are_thirteen_lowercase_base32_characters() {
        let id = PairingId::generate(&|buf: &mut [u8]| {
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = i as u8 * 31 + 7;
            }
        });
        assert_eq!(id.as_str().len(), 13);
        assert_eq!(PairingId::parse(id.as_str()).unwrap(), id);
        assert!(PairingId::parse(VECTOR_ID).is_ok());
        for bad in [
            "",
            "abcdefghijkl",
            "abcdefghijklmn",
            "ABCDEFGHIJKLM",
            "abcdefghijkl1",
            "abcdefghijkl8",
            "abcdefghijkl-",
            "abcdefghijkl\n",
            "../../etc/pas",
        ] {
            assert!(PairingId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_version_is_read_from_the_banner() {
        for (banner, version) in [
            ("SSH-2.0-OpenSSH_9.8", (9, 8)),
            ("SSH-2.0-OpenSSH_10.5p1", (10, 5)),
            ("SSH-2.0-OpenSSH_8.9p1 Ubuntu-3ubuntu0.10", (8, 9)),
            ("SSH-1.99-OpenSSH_7.4", (7, 4)),
            ("SSH-2.0-OpenSSH_7.7p1\r\n", (7, 7)),
        ] {
            assert_eq!(openssh_version(banner), Ok(version), "{banner}");
        }
        for banner in [
            "SSH-2.0-dropbear_2022.83",
            "SSH-2.0-OpenSSH_for_Windows_9.5",
            "SSH-2.0-OpenSSH_x.y",
            "SSH-2.0-libssh_0.10.6",
        ] {
            assert!(
                matches!(openssh_version(banner), Err(DialectError::NotOpenSsh(_))),
                "{banner}"
            );
        }
        for banner in ["", "HTTP/1.1 400", "SSH-2.0"] {
            assert_eq!(
                openssh_version(banner),
                Err(DialectError::NoBanner),
                "{banner}"
            );
        }
    }

    #[test]
    fn the_options_follow_the_sshd_version() {
        for (banner, expected) in [
            ("SSH-2.0-OpenSSH_10.5", Dialect::Expiry),
            ("SSH-2.0-OpenSSH_9.8p1", Dialect::Expiry),
            ("SSH-2.0-OpenSSH_7.7", Dialect::Expiry),
            ("SSH-2.0-OpenSSH_7.7p1", Dialect::Expiry),
            ("SSH-2.0-OpenSSH_7.6p1", Dialect::Restrict),
            ("SSH-2.0-OpenSSH_7.2", Dialect::Restrict),
            ("SSH-2.0-OpenSSH_7.1p2", Dialect::Legacy),
            ("SSH-2.0-OpenSSH_6.6.1p1", Dialect::Legacy),
        ] {
            assert_eq!(dialect(Some(banner)), Ok(expected), "{banner}");
        }
        assert_eq!(dialect(None), Err(DialectError::NoBanner));
        assert!(dialect(Some("SSH-2.0-dropbear_2022.83")).is_err());
        assert!(Dialect::Expiry.note().is_none());
        assert!(Dialect::Restrict.note().is_some() && Dialect::Legacy.note().is_some());
    }

    #[test]
    fn the_line_for_each_dialect() {
        let key = KeyLine::parse(VECTOR_KEY).unwrap();
        let id = PairingId::parse(VECTOR_ID).unwrap();
        let expires = DateTime::from_unix(1_782_867_661);
        let tail = format!("{VECTOR_KEY} or2-pair-bootstrap-abcdefghijklm");
        let exe = "/home/dev/.cargo/bin/or2-pair";
        assert_eq!(
            line(Dialect::Expiry, exe, &id, &key, &expires),
            format!(
                "restrict,command=\"{exe} enroll abcdefghijklm\",expiry-time=\"202607010101\" {tail}"
            )
        );
        assert_eq!(
            line(Dialect::Restrict, exe, &id, &key, &expires),
            format!("restrict,command=\"{exe} enroll abcdefghijklm\" {tail}")
        );
        assert_eq!(
            line(Dialect::Legacy, exe, &id, &key, &expires),
            format!(
                "command=\"{exe} enroll abcdefghijklm\",no-pty,no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-user-rc {tail}"
            )
        );
    }

    #[test]
    fn the_executable_path_may_only_use_shell_neutral_characters() {
        for ok in [
            "/usr/local/bin/or2-pair",
            "/home/dev/.cargo/bin/or2-pair",
            "/opt/or2+pair/bin/or2_pair-1.0",
        ] {
            assert_eq!(check_exe_path(Path::new(ok)).as_deref(), Ok(ok));
        }
        for (bad, what) in [
            ("/home/my user/bin/or2-pair", "a space"),
            ("/home/dev/bin/or2$pair", "'$'"),
            ("/home/dev/it's/or2-pair", "'\\''"),
            ("/home/dev/\"q\"/or2-pair", "'\"'"),
            ("/home/dev/a;b/or2-pair", "';'"),
            ("/home/dev/é/or2-pair", "'é'"),
            ("/home/dev/a\nb/or2-pair", "a control character"),
            ("/home/dev/a`b/or2-pair", "'`'"),
            ("/home/dev/a~b/or2-pair", "'~'"),
        ] {
            let error = check_exe_path(Path::new(bad)).unwrap_err().to_string();
            assert!(error.contains(what), "{bad}: {error}");
            assert!(error.contains("~/.local/bin"), "{error}");
        }
        assert!(matches!(
            check_exe_path(Path::new("bin/or2-pair")),
            Err(ExeError::NotAbsolute(_))
        ));
    }

    #[test]
    fn a_login_shell_that_cannot_run_a_command_is_refused() {
        for bad in [
            "/usr/sbin/nologin",
            "/sbin/nologin",
            "/bin/false",
            "/usr/bin/false",
            "nologin",
        ] {
            assert!(check_login_shell(Some(bad)).is_err(), "{bad}");
        }
        for fine in [
            "/bin/bash",
            "/bin/zsh",
            "/usr/bin/fish",
            "/bin/sh",
            "/bin/falsetto",
        ] {
            assert!(check_login_shell(Some(fine)).is_ok(), "{fine}");
        }
        assert!(check_login_shell(None).is_ok());
    }
}
