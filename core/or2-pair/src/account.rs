//! The one account `or2-pair` pairs for: the user this process runs as, and that user's home.
//!
//! The confirmation names a login, the QR tells the phone to connect as that login, and the
//! key lands in some `~/.ssh/authorized_keys`. Those three must be the same account, so they
//! all come from here, from the operating system's account database for the *effective* user,
//! never from `$HOME`/`$USER` (which `sudo` and friends leave pointing at someone else) and never
//! from a flag. `--user` may only repeat this name; it cannot choose another account.
//!
//! This exists on Unix only. Elsewhere there is no lookup at all (the login and profile
//! directory of such a process are plain environment variables, not an account database), so
//! nothing there decides whose key file to write, and `or2-pair` writes none there: see
//! [`Account::login_only`].

use std::path::PathBuf;

/// The effective user of this process and the home directory the account database gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The login name: what the phone connects as and what the confirmation shows.
    pub name: String,
    /// Whose `.ssh/authorized_keys` receives the key.
    pub home: PathBuf,
    /// The numeric user id files in the home must belong to (0 where the platform has none; no
    /// file is written there).
    pub uid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("cannot tell which account this process runs as: {0}")]
    Unknown(String),
}

impl Account {
    /// An account with an explicit name and home, owned by the effective user. For tests and for
    /// tools that run in a throwaway home; the real binary uses [`Account::current`].
    pub fn new(name: impl Into<String>, home: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            home: home.into(),
            uid: effective_uid(),
        }
    }

    /// The account of the effective user, from the account database.
    #[cfg(unix)]
    pub fn current() -> Result<Self, AccountError> {
        use std::ffi::CStr;
        use std::os::unix::ffi::OsStringExt;

        let uid = effective_uid();
        let mut buffer = vec![0u8; 1024];
        loop {
            // SAFETY: `getpwuid_r` fills `passwd` with pointers into `buffer`, which outlives
            // every use of them below; `result` is set to `passwd` or null.
            let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let code = unsafe {
                libc::getpwuid_r(
                    uid,
                    &mut passwd,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if code == libc::ERANGE && buffer.len() < (1 << 20) {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if code != 0 || result.is_null() || passwd.pw_name.is_null() || passwd.pw_dir.is_null()
            {
                return Err(AccountError::Unknown(format!(
                    "user id {uid} has no entry in the account database"
                )));
            }
            // SAFETY: both pointers are non-null, NUL-terminated strings inside `buffer`.
            let (name, dir) = unsafe {
                (
                    CStr::from_ptr(passwd.pw_name).to_bytes().to_vec(),
                    CStr::from_ptr(passwd.pw_dir).to_bytes().to_vec(),
                )
            };
            let name = String::from_utf8(name)
                .map_err(|_| AccountError::Unknown("the login name is not UTF-8".into()))?;
            if name.is_empty() || dir.is_empty() {
                return Err(AccountError::Unknown(format!(
                    "the account database entry for user id {uid} is incomplete"
                )));
            }
            return Ok(Self {
                name,
                home: PathBuf::from(std::ffi::OsString::from_vec(dir)),
                uid,
            });
        }
    }

    /// An account of which only the login is known, on a target where `or2-pair` installs no
    /// keys (so no home is needed and none is guessed). The login is whatever the person passed
    /// with `--user`; it is shown and shipped in the code and decides nothing about any file:
    /// [`crate::authorized_keys::add`] refuses an account without a home.
    pub fn login_only(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            home: PathBuf::new(),
            uid: effective_uid(),
        }
    }

    /// Whether `requested` (the `--user` flag) names this account.
    pub fn is_named(&self, requested: &str) -> bool {
        if cfg!(windows) {
            self.name.eq_ignore_ascii_case(requested)
        } else {
            self.name == requested
        }
    }
}

#[cfg(unix)]
pub fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
pub fn effective_uid() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_current_account_comes_from_the_account_database_not_the_environment() {
        let account = Account::current().unwrap();
        assert_eq!(account.uid, effective_uid());
        assert!(!account.name.is_empty());
        assert!(account.home.is_absolute(), "{:?}", account.home);
        // `id -un` reads the same database for the effective user.
        if let Ok(output) = std::process::Command::new("id").arg("-un").output()
            && output.status.success()
        {
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), account.name);
        }
    }

    #[test]
    fn only_the_accounts_own_name_is_accepted() {
        let account = Account::new("alice", "/home/alice");
        assert!(account.is_named("alice"));
        assert!(!account.is_named("bob"));
        assert!(!account.is_named("Alice") || cfg!(windows));
        assert!(!account.is_named(""));
    }
}
