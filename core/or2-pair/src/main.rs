use std::process::ExitCode;

use or2_pair::args::{self, Parsed};

fn main() -> ExitCode {
    let options = match args::parse(std::env::args().skip(1)) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::Help) => {
            print!("{}", args::HELP);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("or2-pair {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("or2-pair: {error}");
            return ExitCode::from(2);
        }
    };
    match or2_pair::run_main(&options) {
        Ok(exit) => ExitCode::from(u8::try_from(exit.code()).unwrap_or(1)),
        Err(error) => {
            eprintln!("\nor2-pair: {error}");
            ExitCode::FAILURE
        }
    }
}
