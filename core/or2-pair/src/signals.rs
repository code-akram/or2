//! SIGINT, SIGTERM and SIGHUP while a pairing is live (Unix only).
//!
//! The handler only counts: the waiting loop of `run` sees the count and cleans up (removes the
//! temporary key and the state files), so no unsafe work runs in signal context. A second signal
//! while the cleanup is retrying makes it give up after one more attempt. The handlers are
//! installed without `SA_RESTART`, so a blocked system call returns early.

use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

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
