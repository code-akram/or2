//! Where the pairing code `K` is typed: one line from standard input.
//!
//! The binary asks only when standard input is a terminal (so `yes | or2-pair` or a script cannot
//! answer for the person); the `test-support` host reads the same line from a pipe. The code is
//! not echoed (on a terminal, echo is off while it is typed: [`EchoOff`]), not written to a file
//! and not logged, and every buffer that held it is zeroized (the line is read from descriptor 0
//! directly, not through `std`'s buffered stdin, whose buffer could not be wiped).

use std::io::{self, IsTerminal};

use zeroize::Zeroizing;

/// The longest line taken (a code is 12 characters plus separators).
pub const MAX_LINE: usize = 256;

/// A source of typed lines.
pub trait CodePrompt {
    /// One line without its terminator; `None` at the end of input (Ctrl-D, a closed pipe).
    fn read_line(&self) -> Option<Zeroizing<String>>;
}

/// Standard input.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stdin;

impl Stdin {
    /// Whether there is a person to ask: standard input must be a terminal.
    pub fn available() -> bool {
        io::stdin().is_terminal()
    }
}

impl CodePrompt for Stdin {
    fn read_line(&self) -> Option<Zeroizing<String>> {
        #[cfg(unix)]
        let _quiet = EchoOff::new(0);
        read_line_from_stdin()
    }
}

#[cfg(unix)]
pub use echo::EchoOff;

/// Terminal echo switched off while the code is typed, and back on whatever happens.
#[cfg(unix)]
mod echo {
    use std::cell::UnsafeCell;
    use std::mem::MaybeUninit;
    use std::os::fd::RawFd;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    /// The settings to put back from a signal handler (which can only use what is already in
    /// memory): written before [`SAVED_FD`] names a descriptor, read only while it does.
    struct Saved(UnsafeCell<MaybeUninit<libc::termios>>);

    // SAFETY: written only by `EchoOff::new` before `SAVED_FD` is set (a `SeqCst` store), read
    // only after a `SeqCst` load of a descriptor; nothing writes it while it is read.
    unsafe impl Sync for Saved {}

    static SAVED: Saved = Saved(UnsafeCell::new(MaybeUninit::uninit()));
    static SAVED_FD: AtomicI32 = AtomicI32::new(-1);

    /// Whether this guard's [`stop`] handles SIGTSTP (it was not ignored), so that [`resume`]
    /// installs it again after the stop.
    static STOP_HANDLED: AtomicBool = AtomicBool::new(false);

    /// The signals that end the process while the code is typed.
    const ENDING: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];

    /// Installs `handler` for `signal` (no flags: a blocked read returns early); the previous
    /// action goes into `old` when asked for.
    ///
    /// # Safety
    ///
    /// `handler` must be async-signal-safe.
    unsafe fn install(
        signal: libc::c_int,
        handler: extern "C" fn(libc::c_int),
        old: *mut libc::sigaction,
    ) -> bool {
        // SAFETY: `action` is fully initialised; `sigaction` and `sigemptyset` are
        // async-signal-safe, so this may run in a handler too.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            libc::sigaction(signal, &action, old) == 0
        }
    }

    /// Gives `signal` its default action and raises it: delivered when the handler returns.
    ///
    /// # Safety
    ///
    /// Only from a handler for `signal` (which is blocked while it runs).
    unsafe fn reraise_by_default(signal: libc::c_int) {
        // SAFETY: `sigaction`, `sigemptyset` and `raise` are async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
            libc::raise(signal);
        }
    }

    /// A signal that ends the process: puts the terminal back and lets the signal do what it
    /// would have done.
    extern "C" fn restore_and_reraise(signal: libc::c_int) {
        let fd = SAVED_FD.swap(-1, Ordering::SeqCst);
        // SAFETY: `tcsetattr` is async-signal-safe; `SAVED` holds valid settings whenever
        // `SAVED_FD` named a descriptor.
        unsafe {
            if fd >= 0 {
                libc::tcsetattr(fd, libc::TCSANOW, (*SAVED.0.get()).as_ptr());
            }
            reraise_by_default(signal);
        }
    }

    /// SIGTSTP (Ctrl-Z): puts the terminal back for whatever runs while this process is stopped,
    /// and stops. The prompt is still waiting: [`resume`] switches echo off again.
    extern "C" fn stop(signal: libc::c_int) {
        let fd = SAVED_FD.load(Ordering::SeqCst);
        // SAFETY: as in `restore_and_reraise`.
        unsafe {
            if fd >= 0 {
                libc::tcsetattr(fd, libc::TCSANOW, (*SAVED.0.get()).as_ptr());
            }
            reraise_by_default(signal);
        }
    }

    /// SIGCONT: the process goes on with the prompt, so echo goes off again (what the terminal
    /// has now, without `ECHO` and `ECHONL`), and SIGTSTP is handled again (the stop gave it its
    /// default action). Nothing once the guard is gone.
    extern "C" fn resume(_: libc::c_int) {
        let fd = SAVED_FD.load(Ordering::SeqCst);
        if fd < 0 {
            return;
        }
        // SAFETY: `tcgetattr`, `tcsetattr` and `sigaction` are async-signal-safe; `stop` is
        // async-signal-safe.
        unsafe {
            let mut settings: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut settings) == 0 {
                settings.c_lflag &= !(libc::ECHO | libc::ECHONL);
                libc::tcsetattr(fd, libc::TCSANOW, &settings);
            }
            if STOP_HANDLED.load(Ordering::SeqCst) {
                install(libc::SIGTSTP, stop, std::ptr::null_mut());
            }
        }
    }

    /// While this lives, the terminal on `fd` does not echo what is typed (neither the
    /// characters nor the newline). Dropping it (on success, an error or a panic) puts the
    /// settings back; a signal that ends or stops the process puts them back first, and when a
    /// stopped process goes on (SIGCONT) echo goes off again for the rest of the line. Nothing
    /// happens when `fd` is not a terminal.
    pub struct EchoOff {
        fd: RawFd,
        original: Option<libc::termios>,
        handlers: Vec<(libc::c_int, libc::sigaction)>,
    }

    impl EchoOff {
        pub fn new(fd: RawFd) -> Self {
            let mut guard = Self {
                fd,
                original: None,
                handlers: Vec::new(),
            };
            // SAFETY: `termios` is plain old data that `tcgetattr` fills.
            let mut original: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
                return guard;
            }
            // SAFETY: see `Saved`; no handler reads it before `SAVED_FD` is set below.
            unsafe { (*SAVED.0.get()).write(original) };
            SAVED_FD.store(fd, Ordering::SeqCst);
            let handled = ENDING
                .iter()
                .map(|signal| (*signal, restore_and_reraise as extern "C" fn(libc::c_int)))
                .chain([
                    (libc::SIGTSTP, stop as extern "C" fn(libc::c_int)),
                    (libc::SIGCONT, resume as extern "C" fn(libc::c_int)),
                ]);
            for (signal, handler) in handled {
                // SAFETY: `old` is filled by `sigaction`; every handler here is
                // async-signal-safe.
                unsafe {
                    let mut old: libc::sigaction = std::mem::zeroed();
                    if libc::sigaction(signal, std::ptr::null(), &mut old) != 0
                        || (old.sa_sigaction == libc::SIG_IGN && signal != libc::SIGCONT)
                    {
                        // An ignored signal stays ignored (an ignored SIGCONT still resumes
                        // the process, so echo must go off again then too).
                        continue;
                    }
                    if signal == libc::SIGTSTP {
                        STOP_HANDLED.store(true, Ordering::SeqCst);
                    }
                    if install(signal, handler, std::ptr::null_mut()) {
                        guard.handlers.push((signal, old));
                    } else if signal == libc::SIGTSTP {
                        STOP_HANDLED.store(false, Ordering::SeqCst);
                    }
                }
            }
            let mut quiet = original;
            quiet.c_lflag &= !(libc::ECHO | libc::ECHONL);
            // SAFETY: `quiet` is a valid copy of the terminal's settings.
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
            guard.original = Some(original);
            guard
        }

        /// Whether echo was switched off (the descriptor is a terminal).
        pub fn active(&self) -> bool {
            self.original.is_some()
        }
    }

    impl Drop for EchoOff {
        fn drop(&mut self) {
            // First, so that a SIGCONT arriving now does not switch echo off again.
            SAVED_FD.store(-1, Ordering::SeqCst);
            STOP_HANDLED.store(false, Ordering::SeqCst);
            if let Some(original) = &self.original {
                // SAFETY: `original` came from `tcgetattr` on this descriptor.
                unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, original) };
            }
            for (signal, old) in self.handlers.drain(..) {
                // SAFETY: `old` is what `sigaction` reported before.
                unsafe { libc::sigaction(signal, &old, std::ptr::null_mut()) };
            }
        }
    }
}

#[cfg(unix)]
fn read_line_from_stdin() -> Option<Zeroizing<String>> {
    use std::fs::File;
    use std::io::Read;
    use std::mem::ManuallyDrop;
    use std::os::fd::FromRawFd;

    // SAFETY: descriptor 0 is open for the life of the process; `ManuallyDrop` keeps this
    // `File` from closing it.
    let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });
    let mut line = Zeroizing::new(Vec::<u8>::new());
    // One byte at a time, so that nothing after the newline is consumed (a pipe may hold the
    // next line already).
    let mut chunk = Zeroizing::new([0u8; 1]);
    // Whether a line ended with its newline (an empty one is then an empty answer) or input
    // ended first.
    let mut complete = false;
    loop {
        let count = match stdin.read(&mut *chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if let Some(end) = chunk[..count].iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&chunk[..end]);
            complete = true;
            break;
        }
        line.extend_from_slice(&chunk[..count]);
        if line.len() > MAX_LINE {
            break;
        }
    }
    if line.is_empty() && !complete {
        return None;
    }
    // Lossy: anything that is not text cannot be a code and fails its check.
    let text = Zeroizing::new(String::from_utf8_lossy(&line).into_owned());
    Some(text)
}

#[cfg(not(unix))]
fn read_line_from_stdin() -> Option<Zeroizing<String>> {
    let mut line = Zeroizing::new(String::new());
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    use super::*;

    /// A pseudo-terminal: the side a person types into, and the terminal the program reads.
    fn pty() -> (File, File) {
        // SAFETY: plain calls on a descriptor this function owns; `ptsname_r` writes at most
        // `name.len()` bytes.
        unsafe {
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            assert!(master >= 0);
            let master = OwnedFd::from_raw_fd(master);
            assert_eq!(libc::grantpt(master.as_raw_fd()), 0);
            assert_eq!(libc::unlockpt(master.as_raw_fd()), 0);
            let mut name = [0 as libc::c_char; 128];
            assert_eq!(
                libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()),
                0
            );
            let slave = libc::open(
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            );
            assert!(slave >= 0);
            (File::from(master), File::from_raw_fd(slave))
        }
    }

    fn echoes(fd: RawFd) -> bool {
        // SAFETY: as in `EchoOff::new`.
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::tcgetattr(fd, &mut settings) }, 0);
        settings.c_lflag & libc::ECHO != 0
    }

    /// Whatever the terminal sent back to the typing side so far.
    fn echoed(master: &File) -> Vec<u8> {
        // SAFETY: `master` is open; non-blocking so an empty buffer is not a wait.
        unsafe {
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        while let Ok(count) = (&*master).read(&mut buf) {
            if count == 0 {
                break;
            }
            out.extend_from_slice(&buf[..count]);
        }
        out
    }

    #[test]
    fn echo_is_off_while_the_code_is_typed_and_back_on_after() {
        // Review of the v2 integration: the terminal echoed K, into scrollback and recordings.
        let (mut master, slave) = pty();
        assert!(echoes(slave.as_raw_fd()));
        {
            let _quiet = EchoOff::new(slave.as_raw_fd());
            assert!(!echoes(slave.as_raw_fd()));
            master.write_all(b"7KQ4-M2XD-9PTM\n").unwrap();
            let mut line = [0u8; 15];
            (&slave).read_exact(&mut line).unwrap();
            assert_eq!(&line, b"7KQ4-M2XD-9PTM\n");
            assert!(echoed(&master).is_empty(), "nothing was echoed");
        }
        assert!(echoes(slave.as_raw_fd()), "echo is back after the guard");
        // And it echoes again (the terminal is usable as before).
        master.write_all(b"x\n").unwrap();
        let mut line = [0u8; 2];
        (&slave).read_exact(&mut line).unwrap();
        assert!(!echoed(&master).is_empty());
    }

    #[test]
    fn a_panic_while_typing_puts_echo_back() {
        let (_master, slave) = pty();
        let fd = slave.as_raw_fd();
        let result = std::panic::catch_unwind(|| {
            let _quiet = EchoOff::new(fd);
            assert!(!echoes(fd));
            panic!("an error while the code is typed");
        });
        assert!(result.is_err());
        assert!(echoes(fd));
    }

    #[test]
    fn something_that_is_not_a_terminal_is_left_alone() {
        let file = tempfile::tempfile().unwrap();
        let guard = EchoOff::new(file.as_raw_fd());
        assert!(!guard.active());
    }
}
