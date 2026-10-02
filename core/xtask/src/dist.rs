//! `cargo xtask dist`: the release binaries of the `or2-pair` host CLI and their SHA-256 sums.
//!
//! Linux binaries are static (musl) and linked with the toolchain's own `rust-lld`, so neither a
//! C toolchain nor a musl install is needed, and both architectures build on either. macOS
//! binaries are built on macOS, with Apple's linker and SDK: linking Mach-O here would need the
//! SDK (or zig's stand-ins for it, which lack `libiconv` and the SDK version a binary records),
//! and the result could not be run here to check it. So each host builds its own OS's targets,
//! and the release workflow runs this on a Linux and a macOS runner.
//!
//! Each binary is `<out>/or2-pair-<target>`; `<out>/SHA256SUMS` lists the ones built in this run
//! in `sha256sum` format. A binary that can run here is run once and must print
//! `or2-pair <version>`.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::{Result, fail, repo_root};

/// The released targets.
pub const TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

const USAGE: &str =
    "usage: cargo xtask dist [--target <triple>]... [--out <dir>] [--expect-version <version>]";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    targets: Vec<String>,
    out: Option<PathBuf>,
    expect_version: Option<String>,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value; {USAGE}"))
        };
        match arg.as_str() {
            "--target" => options.targets.push(value("--target")?),
            "--out" => options.out = Some(PathBuf::from(value("--out")?)),
            "--expect-version" => options.expect_version = Some(value("--expect-version")?),
            other => return fail(format!("unknown argument {other:?}; {USAGE}")),
        }
    }
    Ok(options)
}

/// The operating system a target's binaries run on (and are built on).
fn target_os(target: &str) -> Option<&'static str> {
    if !TARGETS.contains(&target) {
        None
    } else if target.ends_with("-linux-musl") {
        Some("linux")
    } else {
        Some("macos")
    }
}

/// The targets built when none is named: this host's OS's.
fn default_targets(host_os: &str) -> Result<Vec<String>> {
    let targets: Vec<String> = TARGETS
        .iter()
        .filter(|target| target_os(target) == Some(host_os))
        .map(|target| (*target).to_owned())
        .collect();
    if targets.is_empty() {
        return fail(format!(
            "no release target is built on {host_os}: Linux targets build on Linux, macOS targets on macOS"
        ));
    }
    Ok(targets)
}

/// Checks that `target` is released and can be built on `host_os`.
fn check_target(target: &str, host_os: &str) -> Result<()> {
    match target_os(target) {
        None => fail(format!(
            "{target} is not a release target (they are {})",
            TARGETS.join(", ")
        )),
        Some(os) if os != host_os => fail(format!(
            "{target} is built on {os}, not here ({host_os}): macOS binaries need Apple's linker and SDK, and the Linux ones are built on Linux"
        )),
        Some(_) => Ok(()),
    }
}

/// Whether a binary for `target` runs on this host (same OS and architecture).
fn runs_here(target: &str, host_os: &str, host_arch: &str) -> bool {
    target_os(target) == Some(host_os) && target.starts_with(&format!("{host_arch}-"))
}

/// The environment variable that names a target's linker (`CARGO_TARGET_<TRIPLE>_LINKER`).
fn linker_variable(target: &str) -> String {
    format!(
        "CARGO_TARGET_{}_LINKER",
        target.to_ascii_uppercase().replace('-', "_")
    )
}

/// The extra environment of the build of `target`: strip symbols, keep the builder's paths out
/// of the binary, and for Linux link with `rust-lld` (unless a linker is set already).
fn build_env(
    target: &str,
    repo: &Path,
    cargo_home: &Path,
    linker_set: bool,
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "CARGO_PROFILE_RELEASE_STRIP".to_owned(),
            "symbols".to_owned(),
        ),
        (
            "CARGO_ENCODED_RUSTFLAGS".to_owned(),
            [
                format!("--remap-path-prefix={}=/or2", repo.display()),
                format!("--remap-path-prefix={}=/cargo", cargo_home.display()),
            ]
            .join("\u{1f}"),
        ),
    ];
    if target_os(target) == Some("linux") && !linker_set {
        env.push((linker_variable(target), "rust-lld".to_owned()));
    }
    env
}

/// `sha256sum` format: `<hex>  <name>`, one line per file, by name.
fn sums(entries: &[(String, [u8; 32])]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|(name, digest)| {
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("{hex}  {name}\n")
        })
        .collect();
    lines.sort_by(|a, b| a[66..].cmp(&b[66..]));
    lines.concat()
}

/// The cargo that runs this task (`$CARGO`), else the one on `PATH`.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn run_cargo(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .map_err(|error| format!("{:?}: {error}", command.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        fail(format!("{command:?} failed ({status})"))
    }
}

/// The version of `or2-pair` (the workspace's).
fn crate_version(core: &Path) -> Result<String> {
    let output = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .current_dir(core)
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
        return fail(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata: {error}"))?;
    metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "or2-pair")
        })
        .and_then(|package| package["version"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "cargo metadata names no or2-pair version".to_owned())
}

pub fn run(args: &[String]) -> Result<()> {
    let options = parse(args)?;
    let host_os = std::env::consts::OS;
    let targets = if options.targets.is_empty() {
        default_targets(host_os)?
    } else {
        options.targets.clone()
    };
    for target in &targets {
        check_target(target, host_os)?;
    }
    for variable in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
    ] {
        if std::env::var_os(variable).is_some() {
            return fail(format!(
                "{variable} is set: release builds use only the flags dist sets; unset it"
            ));
        }
    }

    let repo = repo_root();
    let core = repo.join("core");
    let version = crate_version(&core)?;
    if let Some(expected) = &options.expect_version
        && expected != &version
    {
        return fail(format!(
            "or2-pair is version {version}, not {expected} (the tag and the workspace version must agree)"
        ));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .unwrap_or_default();
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| core.join("target"));
    let out = options
        .out
        .clone()
        .unwrap_or_else(|| core.join("target/dist"));
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    // What an earlier run left would not be in this run's SHA256SUMS.
    for entry in std::fs::read_dir(&out)
        .map_err(|error| format!("{}: {error}", out.display()))?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("or2-pair-") || name == "SHA256SUMS" {
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        }
    }

    let mut entries = Vec::new();
    for target in &targets {
        eprintln!("dist: building or2-pair {version} for {target}");
        let mut command = Command::new(cargo());
        command
            .args([
                "build",
                "--release",
                "--locked",
                "-p",
                "or2-pair",
                "--target",
            ])
            .arg(target)
            .current_dir(&core);
        let linker_set = std::env::var_os(linker_variable(target)).is_some();
        for (key, value) in build_env(target, &repo, &cargo_home, linker_set) {
            command.env(key, value);
        }
        run_cargo(&mut command)?;

        let built = target_dir.join(target).join("release/or2-pair");
        let name = format!("or2-pair-{target}");
        let dest = out.join(&name);
        std::fs::copy(&built, &dest)
            .map_err(|error| format!("{} -> {}: {error}", built.display(), dest.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("{}: {error}", dest.display()))?;
        }
        if runs_here(target, host_os, std::env::consts::ARCH) {
            let output = Command::new(&dest)
                .arg("--version")
                .output()
                .map_err(|error| format!("{}: {error}", dest.display()))?;
            let printed = String::from_utf8_lossy(&output.stdout);
            let expected = format!("or2-pair {version}");
            if !output.status.success() || printed.trim() != expected {
                return fail(format!(
                    "{name} --version printed {:?}, not {expected:?}",
                    printed.trim()
                ));
            }
            eprintln!("dist: {name} --version: {expected}");
        } else {
            eprintln!("dist: {name} is for another architecture; not run here");
        }
        let bytes = std::fs::read(&dest).map_err(|error| format!("{}: {error}", dest.display()))?;
        entries.push((name, Sha256::digest(&bytes).into()));
    }
    let text = sums(&entries);
    let sums_path = out.join("SHA256SUMS");
    std::fs::write(&sums_path, &text)
        .map_err(|error| format!("{}: {error}", sums_path.display()))?;
    print!("{text}");
    eprintln!("dist: wrote {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn arguments() {
        assert_eq!(parse(&[]).unwrap(), Options::default());
        assert_eq!(
            parse(&strings(&[
                "--target",
                "x86_64-unknown-linux-musl",
                "--target",
                "aarch64-unknown-linux-musl",
                "--out",
                "d",
                "--expect-version",
                "0.2.0",
            ]))
            .unwrap(),
            Options {
                targets: strings(&["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]),
                out: Some(PathBuf::from("d")),
                expect_version: Some("0.2.0".into()),
            }
        );
        assert!(parse(&strings(&["--target"])).is_err());
        assert!(parse(&strings(&["--bogus"])).is_err());
    }

    #[test]
    fn each_host_builds_its_own_os() {
        assert_eq!(
            default_targets("linux").unwrap(),
            strings(&["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"])
        );
        assert_eq!(
            default_targets("macos").unwrap(),
            strings(&["x86_64-apple-darwin", "aarch64-apple-darwin"])
        );
        assert!(default_targets("windows").is_err());
        assert!(check_target("aarch64-unknown-linux-musl", "linux").is_ok());
        let refused = check_target("aarch64-apple-darwin", "linux").unwrap_err();
        assert!(refused.contains("Apple's linker"), "{refused}");
        assert!(check_target("x86_64-unknown-linux-gnu", "linux").is_err());
    }

    #[test]
    fn only_a_binary_for_this_host_is_run() {
        assert!(runs_here("x86_64-unknown-linux-musl", "linux", "x86_64"));
        assert!(!runs_here("aarch64-unknown-linux-musl", "linux", "x86_64"));
        assert!(runs_here("aarch64-apple-darwin", "macos", "aarch64"));
        assert!(!runs_here("x86_64-apple-darwin", "linux", "x86_64"));
    }

    #[test]
    fn the_build_environment() {
        let env = build_env(
            "aarch64-unknown-linux-musl",
            Path::new("/src/or2"),
            Path::new("/home/dev/.cargo"),
            false,
        );
        assert!(env.contains(&(
            "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER".into(),
            "rust-lld".into()
        )));
        assert!(env.contains(&("CARGO_PROFILE_RELEASE_STRIP".into(), "symbols".into())));
        assert!(env.contains(&(
            "CARGO_ENCODED_RUSTFLAGS".into(),
            "--remap-path-prefix=/src/or2=/or2\u{1f}--remap-path-prefix=/home/dev/.cargo=/cargo"
                .into()
        )));
        // A linker the caller set wins; macOS uses the system's.
        for (target, set) in [
            ("x86_64-unknown-linux-musl", true),
            ("aarch64-apple-darwin", false),
        ] {
            let env = build_env(target, Path::new("/r"), Path::new("/c"), set);
            assert!(
                env.iter().all(|(key, _)| !key.ends_with("_LINKER")),
                "{env:?}"
            );
        }
    }

    #[test]
    fn sums_are_in_sha256sum_format_by_name() {
        let text = sums(&[
            ("or2-pair-b".into(), [0xab; 32]),
            ("or2-pair-a".into(), [0x01; 32]),
        ]);
        assert_eq!(
            text,
            format!(
                "{}  or2-pair-a\n{}  or2-pair-b\n",
                "01".repeat(32),
                "ab".repeat(32)
            )
        );
    }
}
