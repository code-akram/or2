//! The host's public key, which goes into the pairing code so the phone can trust it from the
//! scan.
//!
//! ED25519 first: from `<etc>/ssh_host_ed25519_key.pub`, else from `ssh-keyscan` of localhost.
//! Only when the host has no ED25519 key are ECDSA and RSA considered (file, then keyscan), in
//! that order. The result says where the key came from.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::keyline::KeyLine;

/// Where `sshd` keeps its host keys.
pub fn default_etc_ssh() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        PathBuf::from(base).join("ssh")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/ssh")
    }
}

/// `ssh-keyscan`, or a stand-in.
pub trait Keyscan {
    /// The scan's standard output for `types` (`ed25519`, `ecdsa,rsa`) on this machine's sshd.
    fn scan(&self, port: u16, types: &str) -> io::Result<String>;
}

/// The real `ssh-keyscan` (an OpenSSH program, run as a subprocess against localhost).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemKeyscan;

impl Keyscan for SystemKeyscan {
    fn scan(&self, port: u16, types: &str) -> io::Result<String> {
        let output = Command::new("ssh-keyscan")
            .args(["-T", "5", "-p", &port.to_string(), "-t", types, "localhost"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyFound {
    pub key: KeyLine,
    /// A file path, or `ssh-keyscan localhost`.
    pub source: String,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "could not read the host's public key: no ED25519, ECDSA or RSA key in {0}, and ssh-keyscan found none (is sshd running?)"
)]
pub struct NoHostKey(pub String);

/// ED25519 from the file, then from keyscan; then (only then) ECDSA and RSA the same way.
pub fn read_host_key(
    etc_ssh: &Path,
    keyscan: &dyn Keyscan,
    ssh_port: u16,
) -> Result<HostKeyFound, NoHostKey> {
    if let Some(found) = from_file(etc_ssh, "ssh-ed25519") {
        return Ok(found);
    }
    if let Some(found) = from_scan(keyscan, ssh_port, "ed25519", &["ssh-ed25519"]) {
        return Ok(found);
    }
    let fallbacks = [
        "ecdsa-sha2-nistp256",
        "ecdsa-sha2-nistp384",
        "ecdsa-sha2-nistp521",
        "ssh-rsa",
    ];
    for algorithm in fallbacks {
        if let Some(found) = from_file(etc_ssh, algorithm) {
            return Ok(found);
        }
    }
    if let Some(found) = from_scan(keyscan, ssh_port, "ecdsa,rsa", &fallbacks) {
        return Ok(found);
    }
    Err(NoHostKey(etc_ssh.display().to_string()))
}

/// The file `sshd` keeps keys of this type in.
fn file_name(algorithm: &str) -> &'static str {
    match algorithm {
        "ssh-ed25519" => "ssh_host_ed25519_key.pub",
        "ssh-rsa" => "ssh_host_rsa_key.pub",
        _ => "ssh_host_ecdsa_key.pub",
    }
}

fn from_file(etc_ssh: &Path, algorithm: &str) -> Option<HostKeyFound> {
    let path = etc_ssh.join(file_name(algorithm));
    let text = fs::read_to_string(&path).ok()?;
    text.lines().find_map(|line| {
        let key = KeyLine::parse(line).ok()?;
        (key.algorithm() == algorithm).then(|| HostKeyFound {
            key,
            source: path.display().to_string(),
        })
    })
}

fn from_scan(
    keyscan: &dyn Keyscan,
    port: u16,
    types: &str,
    preference: &[&str],
) -> Option<HostKeyFound> {
    let output = keyscan.scan(port, types).ok()?;
    let keys: Vec<KeyLine> = output
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            // `<host> <algorithm> <base64>`
            let rest = line.split_once(' ')?.1;
            KeyLine::parse(rest).ok()
        })
        .collect();
    preference
        .iter()
        .find_map(|algorithm| keys.iter().find(|key| key.algorithm() == *algorithm))
        .map(|key| HostKeyFound {
            key: key.clone(),
            source: "ssh-keyscan localhost".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";
    const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
    const RSA: &str = include_str!("../tests/fixtures/rsa.pub");

    struct Scan {
        output: String,
        asked: RefCell<Vec<String>>,
    }

    impl Scan {
        fn new(output: &str) -> Self {
            Self {
                output: output.to_owned(),
                asked: RefCell::default(),
            }
        }
    }

    impl Keyscan for Scan {
        fn scan(&self, _: u16, types: &str) -> io::Result<String> {
            self.asked.borrow_mut().push(types.to_owned());
            Ok(self.output.clone())
        }
    }

    fn etc(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            fs::write(dir.path().join(name), contents).unwrap();
        }
        dir
    }

    #[test]
    fn the_ed25519_file_comes_first_and_keyscan_is_not_asked() {
        let dir = etc(&[
            ("ssh_host_ed25519_key.pub", &format!("{ED25519} root@box\n")),
            ("ssh_host_ecdsa_key.pub", ECDSA),
        ]);
        let scan = Scan::new("");
        let found = read_host_key(dir.path(), &scan, 22).unwrap();
        assert_eq!(found.key.openssh(), ED25519);
        assert!(found.source.ends_with("ssh_host_ed25519_key.pub"));
        assert!(scan.asked.borrow().is_empty());
    }

    #[test]
    fn without_a_file_keyscan_supplies_the_ed25519_key() {
        let dir = etc(&[]);
        let scan = Scan::new(&format!("# localhost:22 SSH-2.0\nlocalhost {ED25519}\n"));
        let found = read_host_key(dir.path(), &scan, 22).unwrap();
        assert_eq!(found.key.openssh(), ED25519);
        assert_eq!(found.source, "ssh-keyscan localhost");
        assert_eq!(*scan.asked.borrow(), ["ed25519"]);
    }

    #[test]
    fn ecdsa_and_rsa_only_when_there_is_no_ed25519_anywhere() {
        // An ED25519 key that keyscan finds beats an ECDSA file.
        let dir = etc(&[("ssh_host_ecdsa_key.pub", ECDSA)]);
        let scan = Scan::new(&format!("localhost {ED25519}\n"));
        assert_eq!(
            read_host_key(dir.path(), &scan, 22).unwrap().key.openssh(),
            ED25519
        );

        // No ED25519 at all: the ECDSA file, then keyscan for the rest.
        let scan = Scan::new("");
        let found = read_host_key(dir.path(), &scan, 22).unwrap();
        assert_eq!(found.key.openssh(), ECDSA);
        assert_eq!(*scan.asked.borrow(), ["ed25519"]);

        let dir = etc(&[]);
        let scan = Scan::new(&format!("localhost {}\nlocalhost {ECDSA}\n", RSA.trim()));
        let found = read_host_key(dir.path(), &scan, 22).unwrap();
        assert_eq!(
            found.key.algorithm(),
            "ecdsa-sha2-nistp256",
            "ECDSA before RSA"
        );
        assert_eq!(*scan.asked.borrow(), ["ed25519", "ecdsa,rsa"]);

        let dir = etc(&[("ssh_host_rsa_key.pub", RSA)]);
        let found = read_host_key(dir.path(), &Scan::new(""), 22).unwrap();
        assert_eq!(found.key.algorithm(), "ssh-rsa");
    }

    #[test]
    fn nothing_found_is_an_error_naming_where_it_looked() {
        let dir = etc(&[("ssh_host_ed25519_key.pub", "garbage\n")]);
        let error = read_host_key(dir.path(), &Scan::new("# nothing\n"), 22).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&dir.path().display().to_string())
        );
    }

    #[test]
    fn a_file_with_the_wrong_kind_of_key_is_not_used() {
        // The ed25519 file holds an ECDSA key: ignored, not trusted by its name.
        let dir = etc(&[("ssh_host_ed25519_key.pub", ECDSA)]);
        assert!(read_host_key(dir.path(), &Scan::new(""), 22).is_err());
    }
}
