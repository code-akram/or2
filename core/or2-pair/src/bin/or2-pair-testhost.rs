//! A host for tests of other programs (the Rust and Kotlin end-to-end tests): the real `or2-pair`
//! flow, with the pairing code `K` read from standard input instead of a terminal so a test can
//! feed it.
//!
//! Built only with the `test-support` feature, so `cargo install` and a plain build never see
//! it. It takes the usual options and honours `OR2_PAIR_TEST_HOME`, `OR2_PAIR_TEST_USER`,
//! `OR2_PAIR_TEST_AUTHORIZED_KEYS` (the key file a disposable sshd reads), `OR2_PAIR_TEST_ETC_SSH`
//! (the host key and `sshd_config`) and `OR2_PAIR_TEST_WINDOW_SECS`, which tests point at
//! throwaway files. The forced command it writes into `authorized_keys` names this binary, which
//! also runs `enroll <id>`. The human output is on standard output as for `or2-pair`: the line
//! that starts `or2-pair:2?` is the pairing code.

use std::process::ExitCode;

fn main() -> ExitCode {
    or2_pair::cli(std::env::args().skip(1), true)
}
