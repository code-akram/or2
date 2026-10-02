//! SIGINT, SIGTERM and SIGHUP while a pairing is live (Unix only), and the children started
//! before that.
//!
//! The handler only counts: the waiting loop of `run` sees the count and cleans up (removes the
//! temporary key and the state files), so no unsafe work runs in signal context. A second signal
//! while the cleanup is retrying makes it give up after one more attempt. The handlers are
//! installed without `SA_RESTART`, so a blocked system call returns early.
//!
//! Before that (the checks), an ending signal ends the process as it always would, but a child in
//! a process group of its own (the login-shell check) is killed first: [`spawn_group`].

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

static RECEIVED: AtomicU32 = AtomicU32::new(0);

extern "C" fn handler(_: libc::c_int) {
    RECEIVED.fetch_add(1, Ordering::SeqCst);
}

/// Starts counting the three signals instead of dying from them.
pub fn install() -> io::Result<()> {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: `action` is fully initialised before the call and `handler` is an
        // async-signal-safe `extern "C"` function (one atomic add).
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// How many of the three signals have arrived since [`install`].
pub fn received() -> u32 {
    RECEIVED.load(Ordering::SeqCst)
}

// --- children in a process group of their own ---------------------------------------------------

/// The signals that end the process while such a child runs.
const ENDING: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

/// The process groups to kill on an ending signal (0: a free slot). A fixed table, so that the
/// handler needs nothing but atomics.
static GROUPS: [AtomicI32; 8] = [const { AtomicI32::new(0) }; 8];

/// How many children are being started: their group is not known yet, so an ending signal is
/// held ([`PENDING`]) and acted on as soon as it is.
static STARTING: AtomicU32 = AtomicU32::new(0);

/// An ending signal that arrived while a child was being started.
static PENDING: AtomicI32 = AtomicI32::new(0);

/// The previous actions of the ending signals, and how many guards live (the first installs the
/// handler, the last puts these back).
type Installed = (usize, Vec<(libc::c_int, libc::sigaction)>);
static INSTALLED: Mutex<Installed> = Mutex::new((0, Vec::new()));

fn installed() -> std::sync::MutexGuard<'static, Installed> {
    INSTALLED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn kill_groups() {
    for slot in &GROUPS {
        let group = slot.load(Ordering::SeqCst);
        if group > 0 {
            // SAFETY: `kill` is async-signal-safe and has no memory preconditions.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
    }
}

/// Ends the process with `signal`'s default action: delivered at once, or (in a handler, where
/// it is blocked) as soon as the handler returns.
fn die_by_default(signal: libc::c_int) {
    // SAFETY: `sigaction`, `sigemptyset` and `raise` are async-signal-safe; `action` is fully
    // initialised.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::null_mut());
        libc::raise(signal);
    }
}

extern "C" fn end_groups(signal: libc::c_int) {
    kill_groups();
    if STARTING.load(Ordering::SeqCst) > 0 {
        // A child whose group is not known yet: its starter ends the process once it is.
        PENDING.store(signal, Ordering::SeqCst);
        return;
    }
    die_by_default(signal);
}

/// Installs [`end_groups`] for the ending signals that are not ignored, for the first guard.
fn acquire() {
    let mut installed = installed();
    installed.0 += 1;
    if installed.0 > 1 {
        return;
    }
    for signal in ENDING {
        // SAFETY: `old` is filled by `sigaction`; `action` is fully initialised and
        // `end_groups` is async-signal-safe (atomics, `kill`, `sigaction`, `raise`).
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut old) != 0
                || old.sa_sigaction == libc::SIG_IGN
            {
                // An ignored signal stays ignored: it ends nothing.
                continue;
            }
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = end_groups as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            if libc::sigaction(signal, &action, std::ptr::null_mut()) == 0 {
                installed.1.push((signal, old));
            }
        }
    }
}

/// Puts the previous actions back with the last guard.
fn release() {
    let mut installed = installed();
    installed.0 -= 1;
    if installed.0 > 0 {
        return;
    }
    for (signal, old) in installed.1.drain(..) {
        // SAFETY: `old` is what `sigaction` reported before.
        unsafe { libc::sigaction(signal, &old, std::ptr::null_mut()) };
    }
}

/// While it lives, SIGINT, SIGTERM, SIGHUP or SIGQUIT (unless ignored) kill the child's whole
/// process group, then end this process as their default action would. See [`spawn_group`].
#[derive(Debug)]
pub struct GroupGuard {
    slot: Option<usize>,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            GROUPS[slot].store(0, Ordering::SeqCst);
        }
        release();
    }
}

/// Starts `command` in a process group of its own and returns it with a [`GroupGuard`]: keep
/// the guard until the child is done with. A child in its own group does not get the terminal's
/// Ctrl-C, so without the guard it would outlive an `or2-pair` that Ctrl-C ends (fix check of
/// the v2 fixes: the login-shell check's shell, killed only at its time limit, was orphaned). A
/// signal that arrives while the child is being started is acted on as soon as its group is
/// known.
pub fn spawn_group(command: &mut Command) -> io::Result<(Child, GroupGuard)> {
    command.process_group(0);
    acquire();
    STARTING.fetch_add(1, Ordering::SeqCst);
    let spawned = command.spawn();
    let group = spawned
        .as_ref()
        .ok()
        .and_then(|child| libc::pid_t::try_from(child.id()).ok());
    let slot = group.and_then(|group| {
        GROUPS.iter().position(|slot| {
            slot.compare_exchange(0, group, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        })
    });
    STARTING.fetch_sub(1, Ordering::SeqCst);
    let guard = GroupGuard { slot };
    let pending = PENDING.swap(0, Ordering::SeqCst);
    if pending != 0 {
        kill_groups();
        if let Some(group) = group {
            // SAFETY: `kill` has no memory preconditions; the group is the child's own.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        die_by_default(pending);
    }
    spawned.map(|child| (child, guard))
}
