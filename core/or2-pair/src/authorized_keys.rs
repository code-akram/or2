//! `~/.ssh/authorized_keys`: where a confirmed key is appended, and the care taken doing it.
//!
//! - `~/.ssh` is created with mode 0700 and the file with 0600 when missing (permissions are
//!   Unix-only; on Windows the inherited ACLs apply),
//! - the file is backed up before it is touched (`authorized_keys.or2-backup-<stamp>`, mode
//!   0600), once per run, only when it exists and is not empty,
//! - a key already present (same algorithm and key data, whatever its options or comment) is
//!   not added again, and then nothing is changed at all, not even a backup,
//! - the line is `no-agent-forwarding,no-X11-forwarding <algorithm> <key> or2-<device>-<date>`,
//!   rebuilt from the validated key, so nothing from the phone but the key itself is written,
//! - a missing final newline is repaired first, so the new line never joins the last one.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::date::DateTime;
use crate::keyline::KeyLine;

/// The options every key added by or2 carries.
pub const OPTIONS: &str = "no-agent-forwarding,no-X11-forwarding";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    /// The line was appended. `backup` is the copy of the previous file, if there was one.
    Added {
        created: bool,
        backup: Option<PathBuf>,
    },
    /// The key was already authorized; the file is untouched.
    AlreadyPresent,
}

pub fn ssh_dir(home: &Path) -> PathBuf {
    home.join(".ssh")
}

pub fn path(home: &Path) -> PathBuf {
    ssh_dir(home).join("authorized_keys")
}

/// A device label as it appears in the key's comment and on screen: ASCII letters, digits,
/// `.`, `_` and `-` only (anything else becomes `-`, runs collapse), at most 32 characters.
/// `None` when nothing is left.
pub fn sanitize_device(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let out: String = out.trim_matches('-').chars().take(32).collect();
    let out = out.trim_matches('-').to_owned();
    (!out.is_empty()).then_some(out)
}

/// The line that is appended.
pub fn line_for(key: &KeyLine, device: &str, date: &str) -> String {
    format!("{OPTIONS} {} or2-{device}-{date}", key.openssh())
}

/// Whether some line of `contents` carries `key` (any options before it, any comment after it).
pub fn contains(contents: &[u8], key: &KeyLine) -> bool {
    let text = String::from_utf8_lossy(contents);
    let blob = key.base64();
    text.lines().any(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return false;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        tokens
            .windows(2)
            .any(|pair| pair[0] == key.algorithm() && pair[1] == blob)
    })
}

/// Appends the key, as the module docs describe.
pub fn add(home: &Path, key: &KeyLine, device: &str, now: DateTime) -> io::Result<Added> {
    let dir = ssh_dir(home);
    if !dir.exists() {
        create_private_dir(&dir)?;
    }
    let file = path(home);
    let existing = match fs::read(&file) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if existing
        .as_deref()
        .is_some_and(|bytes| contains(bytes, key))
    {
        return Ok(Added::AlreadyPresent);
    }
    let backup = match existing.as_deref() {
        Some(bytes) if !bytes.is_empty() => Some(write_backup(&dir, bytes, now)?),
        _ => None,
    };
    let mut out = open_append(&file)?;
    let mut text = String::new();
    if existing
        .as_deref()
        .is_some_and(|b| b.last().is_some_and(|last| *last != b'\n'))
    {
        text.push('\n');
    }
    text.push_str(&line_for(key, device, &now.date()));
    text.push('\n');
    out.write_all(text.as_bytes())?;
    out.sync_all()?;
    Ok(Added::Added {
        created: existing.is_none(),
        backup,
    })
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir(dir)
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn open_append(file: &Path) -> io::Result<fs::File> {
    private_options().create(true).append(true).open(file)
}

fn write_backup(dir: &Path, contents: &[u8], now: DateTime) -> io::Result<PathBuf> {
    for attempt in 0..100 {
        let suffix = if attempt == 0 {
            String::new()
        } else {
            format!("-{attempt}")
        };
        let backup = dir.join(format!(
            "authorized_keys.or2-backup-{}{suffix}",
            now.stamp()
        ));
        match private_options().write(true).create_new(true).open(&backup) {
            Ok(mut out) => {
                out.write_all(contents)?;
                out.sync_all()?;
                return Ok(backup);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("could not find a free backup name"))
}

/// Whether the key could be added right now, without adding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writable {
    Yes,
    /// Not possible; why, for the check's message.
    No(String),
}

/// Checks that `authorized_keys` (or, when it does not exist, `~/.ssh` or `~`) can be written,
/// and reports what `sshd`'s StrictModes would object to. Changes nothing.
pub fn writable(home: &Path) -> (Writable, Vec<String>) {
    let mut notes = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(home)
            && meta.permissions().mode() & 0o022 != 0
        {
            notes.push(
                "your home directory is writable by others: sshd (StrictModes) will ignore authorized_keys until you run chmod go-w ~"
                    .to_owned(),
            );
        }
        if let Ok(meta) = fs::metadata(ssh_dir(home))
            && meta.permissions().mode() & 0o022 != 0
        {
            notes.push("~/.ssh is writable by others: run chmod 700 ~/.ssh".to_owned());
        }
    }
    let file = path(home);
    let result = if file.exists() {
        match OpenOptions::new().append(true).open(&file) {
            Ok(_) => Writable::Yes,
            Err(error) => Writable::No(format!("{} is not writable ({error})", file.display())),
        }
    } else if ssh_dir(home).exists() {
        if dir_writable(&ssh_dir(home)) {
            Writable::Yes
        } else {
            Writable::No(format!("{} is not writable", ssh_dir(home).display()))
        }
    } else if dir_writable(home) {
        Writable::Yes
    } else {
        Writable::No(format!("{} is not writable", home.display()))
    };
    (result, notes)
}

fn dir_writable(dir: &Path) -> bool {
    fs::metadata(dir).is_ok_and(|meta| meta.is_dir() && !meta.permissions().readonly())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBCJz8goXA2qGjRTNHhOwsljhOuCXG/+2B/zTJH/brc5";
    const OTHER: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAc39XUWT33SvSLy6vA7I83+XgmwnHmYtMQRjLeaZ2U7";

    fn key(line: &str) -> KeyLine {
        KeyLine::parse(line).unwrap()
    }

    fn at() -> DateTime {
        DateTime::from_unix(1_782_867_661)
    }

    #[test]
    fn creates_the_directory_and_file_with_private_permissions() {
        let home = tempfile::tempdir().unwrap();
        let added = add(home.path(), &key(ED25519), "Pixel-8", at()).unwrap();
        assert_eq!(
            added,
            Added::Added {
                created: true,
                backup: None
            }
        );
        let contents = fs::read_to_string(path(home.path())).unwrap();
        assert_eq!(
            contents,
            format!("no-agent-forwarding,no-X11-forwarding {ED25519} or2-Pixel-8-2026-07-01\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&ssh_dir(home.path())), 0o700);
            assert_eq!(mode(&path(home.path())), 0o600);
        }
    }

    #[test]
    fn appends_after_a_backup_and_keeps_what_was_there() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let before = format!("# mine\n{OTHER} me@laptop\n");
        fs::write(path(home.path()), &before).unwrap();
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
        let Added::Added { created, backup } = added else {
            panic!("{added:?}")
        };
        assert!(!created);
        let backup = backup.unwrap();
        assert_eq!(
            backup.file_name().unwrap(),
            "authorized_keys.or2-backup-20260701-010101"
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), before);
        let after = fs::read_to_string(path(home.path())).unwrap();
        assert!(after.starts_with(&before));
        assert_eq!(after.lines().count(), 3);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_missing_final_newline_is_repaired_before_appending() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), OTHER).unwrap();
        add(home.path(), &key(ED25519), "phone", at()).unwrap();
        let after = fs::read_to_string(path(home.path())).unwrap();
        let lines: Vec<_> = after.lines().collect();
        assert_eq!(lines[0], OTHER);
        assert!(lines[1].starts_with(OPTIONS));
    }

    #[test]
    fn a_duplicate_key_changes_nothing_even_with_options_or_a_comment() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        let existing = format!("command=\"/bin/true\",no-pty {ED25519} old-comment\n");
        fs::write(path(home.path()), &existing).unwrap();
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
        assert_eq!(added, Added::AlreadyPresent);
        assert_eq!(fs::read_to_string(path(home.path())).unwrap(), existing);
        let names: Vec<_> = fs::read_dir(ssh_dir(home.path())).unwrap().collect();
        assert_eq!(names.len(), 1, "no backup for a no-op");
    }

    #[test]
    fn adding_twice_adds_once() {
        let home = tempfile::tempdir().unwrap();
        add(home.path(), &key(ED25519), "phone", at()).unwrap();
        assert_eq!(
            add(home.path(), &key(ED25519), "phone", at()).unwrap(),
            Added::AlreadyPresent
        );
        assert_eq!(
            fs::read_to_string(path(home.path()))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn a_commented_out_copy_does_not_count() {
        let contents = format!("# {ED25519}\n");
        assert!(!contains(contents.as_bytes(), &key(ED25519)));
        assert!(contains(
            format!("{OPTIONS} {ED25519} c\n").as_bytes(),
            &key(ED25519)
        ));
        assert!(!contains(format!("{OTHER}\n").as_bytes(), &key(ED25519)));
    }

    #[test]
    fn an_empty_existing_file_gets_no_backup() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), "").unwrap();
        let added = add(home.path(), &key(ED25519), "phone", at()).unwrap();
        assert_eq!(
            added,
            Added::Added {
                created: false,
                backup: None
            }
        );
    }

    #[test]
    fn two_runs_in_one_second_do_not_overwrite_each_others_backup() {
        const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBJKzE3AhnQL0fsk77dWQaJJj+GHelMX7ge+TZ2xgJ/Y3O5aMMlnH6Hn/tnucj94OezBBvEj2VIcJGu8ABqUkhFA=";
        let home = tempfile::tempdir().unwrap();
        fs::create_dir(ssh_dir(home.path())).unwrap();
        fs::write(path(home.path()), format!("{OTHER}\n")).unwrap();
        let Added::Added {
            backup: Some(first),
            ..
        } = add(home.path(), &key(ED25519), "a", at()).unwrap()
        else {
            panic!()
        };
        let Added::Added {
            backup: Some(second),
            ..
        } = add(home.path(), &key(ECDSA), "b", at()).unwrap()
        else {
            panic!()
        };
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(&first).unwrap().lines().count(), 1);
        assert_eq!(fs::read_to_string(&second).unwrap().lines().count(), 2);
    }

    #[test]
    fn a_device_label_is_reduced_to_safe_ascii() {
        for (raw, safe) in [
            ("Pixel 8", Some("Pixel-8")),
            ("  Pixel   8  ", Some("Pixel-8")),
            ("a\nb", Some("a-b")),
            ("a\x1b[31mb", Some("a-31mb")),
            ("Büro-Phone", Some("B-ro-Phone")),
            ("../../etc", Some("..-..-etc")),
            ("---", None),
            ("", None),
            ("\n\t", None),
            ("é", None),
            ("my_phone.1", Some("my_phone.1")),
        ] {
            assert_eq!(sanitize_device(raw).as_deref(), safe, "{raw:?}");
        }
        let long = "x".repeat(80);
        assert_eq!(sanitize_device(&long).unwrap().len(), 32);
    }

    #[test]
    fn checks_report_writability_and_strict_modes_trouble() {
        let home = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (result, notes) = writable(home.path());
        assert_eq!(result, Writable::Yes);
        assert!(notes.is_empty(), "{notes:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o775)).unwrap();
            let (_, notes) = writable(home.path());
            assert_eq!(notes.len(), 1);
            assert!(notes[0].contains("StrictModes"));
            // A read-only existing file is reported, not changed.
            fs::set_permissions(home.path(), fs::Permissions::from_mode(0o755)).unwrap();
            fs::create_dir(ssh_dir(home.path())).unwrap();
            fs::write(path(home.path()), "").unwrap();
            fs::set_permissions(path(home.path()), fs::Permissions::from_mode(0o400)).unwrap();
            let (result, _) = writable(home.path());
            // Running as root can write a 0400 file; everyone else cannot.
            if !fs::OpenOptions::new()
                .append(true)
                .open(path(home.path()))
                .is_ok()
            {
                assert!(matches!(result, Writable::No(_)));
            }
        }
    }
}
