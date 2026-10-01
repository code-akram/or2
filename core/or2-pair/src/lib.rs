//! `or2-pair`: pair a phone with this host in one command and one QR scan.
//!
//! A host-side tool, deliberately separate from `or2-core` (which carries the app's SSH, mosh
//! and terminal stack and needs none of it here). See `docs/contracts.md`, "Easy pair", and
//! `docs/pairing.md`.
//!
//! - [`checks`]: report sshd, `authorized_keys`, tmux/herdr/mosh-server and firewall hints,
//! - [`addresses`], [`hostkey`]: what the phone needs to reach and recognise this host,
//! - [`payload`], [`qr`]: the pairing code and its terminal QR,
//! - [`exchange`], [`authorized_keys`], [`confirm`]: the one-shot listener, the on-host
//!   confirmation and the append,
//! - [`net`]: every socket, behind a small trait,
//! - [`run`]: the whole flow, with its environment injected.

pub mod addresses;
pub mod args;
pub mod authorized_keys;
pub mod checks;
pub mod confirm;
pub mod date;
pub mod exchange;
pub mod hostkey;
pub mod keyline;
pub mod net;
pub mod payload;
pub mod qr;
pub mod run;

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::PathBuf;

use crate::addresses::Iface;
use crate::checks::Platform;
use crate::confirm::StdinConfirm;
use crate::date::DateTime;
use crate::hostkey::SystemKeyscan;
use crate::net::StdNet;
use crate::run::{Env, Exit, RunError};

/// The operating system's interface addresses (IPv4 only is used downstream).
pub fn system_interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|iface| Iface {
            name: iface.name.clone(),
            ip: iface.ip(),
        })
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn user_name() -> Option<String> {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok())
        .filter(|user| !user.is_empty())
}

/// Runs the tool for real: this process's environment, the system's sockets, the terminal.
pub fn run_main(options: &args::Options) -> Result<Exit, RunError> {
    let home = home_dir().ok_or_else(|| {
        RunError::Output(std::io::Error::other(
            "cannot find your home directory (HOME is not set)",
        ))
    })?;
    let path: Option<OsString> = std::env::var_os("PATH");
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    let color = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").map_or(true, |term| term != "dumb");
    let random = |buf: &mut [u8]| {
        use rand::Rng;
        rand::rng().fill_bytes(buf);
    };
    let now = DateTime::now;
    let env = Env {
        version: env!("CARGO_PKG_VERSION"),
        program_dirs: checks::program_dirs(path.as_deref(), &home),
        home,
        user: user_name(),
        hostname: Some(hostname),
        etc_ssh: hostkey::default_etc_ssh(),
        interfaces: system_interfaces(),
        platform: Platform::current(),
        net: &StdNet,
        keyscan: &SystemKeyscan,
        confirm: &StdinConfirm,
        can_ask: StdinConfirm::available(),
        color,
        random: &random,
        now: &now,
        window: exchange::WINDOW,
        on_ready: None,
    };
    run::run(options, &env, &mut std::io::stdout().lock())
}
