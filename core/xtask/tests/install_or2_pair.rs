//! `scripts/install-or2-pair.sh` against a local fixture release: the download base and the
//! release list are `file://` URLs (`OR2_PAIR_DOWNLOAD_BASE`, `OR2_PAIR_RELEASES_URL`), and a
//! fake `uname` first on `PATH` plays the OS and CPU. Nothing is fetched from the network and
//! nothing is installed outside the test's temporary directory.
//!
//! It needs `curl` (the only downloader that reads `file://`); without it the tests skip.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const TAG: &str = "or2-pair-v9.9.9";

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install-or2-pair.sh")
}

fn have_curl() -> bool {
    let found = Command::new("curl")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("SKIP: curl is not installed (the installer test reads file:// URLs with it)");
    }
    found
}

fn write_executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A temporary world: a fixture release, a fake `uname`, a home.
struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        // Every released asset: a stand-in that prints its version and its target.
        let mut sums = String::new();
        for target in [
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
        ] {
            let name = format!("or2-pair-{target}");
            let text = format!("#!/bin/sh\necho 'or2-pair 9.9.9'\necho '{target}' >&2\n");
            write_executable(&world.release().join(TAG).join(&name), &text);
            sums.push_str(&format!("{}  {name}\n", hex(text.as_bytes())));
        }
        std::fs::write(world.release().join(TAG).join("SHA256SUMS"), sums).unwrap();
        // The newest or2-pair release after an app release with another tag.
        std::fs::write(
            world.release().join("releases.json"),
            format!(
                "[\n  {{\n    \"tag_name\": \"v2.0.0\",\n    \"name\": \"or2 2.0.0\"\n  }},\n  {{\n    \"tag_name\": \"{TAG}\",\n    \"prerelease\": false\n  }},\n  {{\n    \"tag_name\": \"or2-pair-v0.0.1\"\n  }}\n]\n"
            ),
        )
        .unwrap();
        write_executable(
            &world.path().join("fakebin/uname"),
            "#!/bin/sh\ncase \"$1\" in\n-s) echo \"$FAKE_UNAME_S\" ;;\n-m) echo \"$FAKE_UNAME_M\" ;;\n*) exit 1 ;;\nesac\n",
        );
        std::fs::create_dir_all(world.home()).unwrap();
        world
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn release(&self) -> PathBuf {
        self.path().join("release")
    }

    fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    fn default_dir(&self) -> PathBuf {
        self.home().join(".local/bin")
    }

    fn asset(&self, target: &str) -> PathBuf {
        self.release().join(TAG).join(format!("or2-pair-{target}"))
    }

    /// Runs the installer as `os`/`arch` with `args`.
    fn install(&self, os: &str, arch: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.path().join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("sh");
        command
            .arg(script())
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home())
            .env("FAKE_UNAME_S", os)
            .env("FAKE_UNAME_M", arch)
            .env(
                "OR2_PAIR_DOWNLOAD_BASE",
                format!("file://{}", self.release().display()),
            )
            .env(
                "OR2_PAIR_RELEASES_URL",
                format!("file://{}", self.release().join("releases.json").display()),
            );
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The target of the binary installed in `dir` (the stand-in prints it).
fn installed_target(dir: &Path) -> String {
    let output = Command::new(dir.join("or2-pair"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "or2-pair 9.9.9\n");
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

#[test]
fn a_matching_checksum_installs_the_binary_for_this_host() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    // The newest or2-pair release, into ~/.local/bin, created.
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(output.status.success(), "{said}");
    assert_eq!(
        installed_target(&world.default_dir()),
        "x86_64-unknown-linux-musl"
    );
    assert!(said.contains(&format!("({TAG})")), "{said}");
    assert!(said.contains("Checksum OK"), "{said}");
    assert!(said.contains("Installed or2-pair 9.9.9"), "{said}");
    // Not on PATH: the hint, and the full path to run.
    assert!(said.contains("is not on your PATH"), "{said}");
    assert!(
        said.contains(&format!(
            "  {}/or2-pair",
            world.default_dir().canonicalize().unwrap().display()
        )),
        "{said}"
    );
    let mode = std::fs::metadata(world.default_dir().join("or2-pair"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    // No temporary file left beside it.
    let names: Vec<_> = std::fs::read_dir(world.default_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["or2-pair"]);

    // Each OS and CPU gets its own binary; --version in its three spellings, --dir, and
    // $OR2_PAIR_INSTALL_DIR.
    for (os, arch, version, target) in [
        ("Linux", "aarch64", "9.9.9", "aarch64-unknown-linux-musl"),
        ("Linux", "arm64", "v9.9.9", "aarch64-unknown-linux-musl"),
        ("Linux", "amd64", TAG, "x86_64-unknown-linux-musl"),
        ("Darwin", "arm64", "v9.9.9", "aarch64-apple-darwin"),
        ("Darwin", "x86_64", "v9.9.9", "x86_64-apple-darwin"),
    ] {
        let dir = world.path().join(format!("bin-{os}-{arch}"));
        let output = world.install(
            os,
            arch,
            &["--version", version, "--dir", dir.to_str().unwrap()],
            &[],
        );
        assert!(output.status.success(), "{os} {arch}: {}", text(&output));
        assert_eq!(installed_target(&dir), target);
    }
    let env_dir = world.path().join("from-env");
    let on_path = format!("{}:/usr/bin:/bin", world.path().join("fakebin").display());
    let output = world.install(
        "Linux",
        "x86_64",
        &[],
        &[
            ("OR2_PAIR_INSTALL_DIR", env_dir.to_str().unwrap()),
            ("PATH", &format!("{}:{on_path}", env_dir.display())),
        ],
    );
    let said = text(&output);
    assert!(output.status.success(), "{said}");
    assert_eq!(installed_target(&env_dir), "x86_64-unknown-linux-musl");
    // On PATH: no hint, and the plain name.
    assert!(!said.contains("is not on your PATH"), "{said}");
    assert!(said.ends_with("  or2-pair\n"), "{said}");
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    // The binary changed after SHA256SUMS was written.
    let asset = world.asset("x86_64-unknown-linux-musl");
    let mut bytes = std::fs::read(&asset).unwrap();
    bytes.extend_from_slice(b"# tampered\n");
    std::fs::write(&asset, bytes).unwrap();
    let output = world.install("Linux", "x86_64", &["--version", "9.9.9"], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("checksum mismatch for or2-pair-x86_64-unknown-linux-musl"),
        "{said}"
    );
    assert!(said.contains("nothing was installed"), "{said}");
    assert!(!world.default_dir().join("or2-pair").exists());

    // SHA256SUMS without this asset's line: refused too.
    std::fs::write(
        world.release().join(TAG).join("SHA256SUMS"),
        format!("{}  or2-pair-aarch64-apple-darwin\n", "0".repeat(64)),
    )
    .unwrap();
    let output = world.install("Linux", "x86_64", &["--version", "9.9.9"], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("lists no or2-pair-x86_64-unknown-linux-musl"),
        "{said}"
    );
    assert!(!world.default_dir().join("or2-pair").exists());

    // A release that is not there.
    let output = world.install("Linux", "aarch64", &["--version", "1.2.3"], &[]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("could not download"),
        "{}",
        text(&output)
    );
}

#[test]
fn an_unsupported_os_or_cpu_is_refused_before_any_download() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    for (os, arch, what) in [
        ("Linux", "riscv64", "unsupported CPU architecture: riscv64"),
        ("Linux", "armv7l", "unsupported CPU architecture: armv7l"),
        ("FreeBSD", "amd64", "unsupported operating system: FreeBSD"),
    ] {
        let output = world.install(os, arch, &[], &[]);
        let said = text(&output);
        assert!(!output.status.success(), "{said}");
        assert!(said.contains(what), "{said}");
        assert!(said.contains("cargo install"), "{said}");
        assert!(!said.contains("Downloading"), "{said}");
    }
    assert!(!world.default_dir().exists());

    // A bad argument is a usage error.
    let output = world.install("Linux", "x86_64", &["--bogus"], &[]);
    assert!(!output.status.success());
    assert!(text(&output).contains("Usage:"));
}
