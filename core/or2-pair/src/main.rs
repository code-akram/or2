use std::process::ExitCode;

fn main() -> ExitCode {
    or2_pair::cli(std::env::args().skip(1), false)
}
