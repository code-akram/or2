//! Repository tooling, run as `cargo xtask <task>` from `core/` (the alias lives in
//! `core/.cargo/config.toml`) or from the repository root with
//! `cargo run --manifest-path core/Cargo.toml -p xtask -- <task>`.
//!
//! Tasks are Rust, never scripts in another language; generated files are checked in and every
//! generator has a `--check` mode. Nothing here is shipped: the crate is outside the licence
//! data of the Android library.

mod dist;
mod herdr;
mod json;
mod licenses;
mod pom;
mod zip;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub type Result<T> = std::result::Result<T, String>;

pub fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(message.into())
}

/// The repository root (`core/xtask` is two levels below it).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("core/xtask has a grandparent")
        .to_path_buf()
}

const USAGE: &str = "\
Usage: cargo xtask <task> [options]

Tasks:
  gen-herdr-types [--herdr PATH] [--offline] [--check]
      regenerate core/or2-core/src/herdr/{schema.json,generated.rs}
  gen-licenses [--check]
      regenerate the open-source licence data (Android assets, core/or2-pair/THIRD_PARTY.md)
  dist [--target TRIPLE]... [--out DIR] [--expect-version VERSION]
      build the or2-pair release binaries this host can build (Linux: static musl, x86_64 and
      aarch64; macOS: x86_64 and aarch64) into core/target/dist, with SHA256SUMS";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("gen-herdr-types") => herdr::run(&args[1..]).map_err(|e| ("gen-herdr-types", e)),
        Some("gen-licenses") => licenses::run(&args[1..]).map_err(|e| ("gen-licenses", e)),
        Some("dist") => dist::run(&args[1..]).map_err(|e| ("dist", e)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((task, message)) => {
            eprintln!("{task}: {message}");
            ExitCode::FAILURE
        }
    }
}
