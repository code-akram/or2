//! `scripts/install-or2-pair.sh` against a local fixture release: `OR2_PAIR_RELEASES_BASE` is a
//! `file://` URL of a tree laid out like `https://github.com/code-akram/or2/releases`
//! (`latest/download/<asset>` and `download/vX.Y.Z/<asset>`), a fake `uname` first on `PATH`
//! plays the OS and CPU and a fake `id` the user id. Nothing is fetched from the network and
//! nothing is installed outside the test's temporary directory.
//!
//! It needs `curl` (the only downloader that reads `file://`); without it the tests skip.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use sha2::{Digest, Sha256};

/// The latest release, and an older one.
const LATEST: &str = "9.9.9";
const OLDER: &str = "9.9.8";

const TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

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

/// A temporary world: a fixture release tree, a fake `uname` and `id`, a home, a `TMPDIR`.
struct World {
    dir: tempfile::TempDir,
    uid: u32,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let uid = std::fs::metadata(dir.path()).unwrap().uid();
        let world = Self { dir, uid };
        world.release_at(&world.latest_dir(), LATEST);
        world.release_at(&world.tag_dir(LATEST), LATEST);
        world.release_at(&world.tag_dir(OLDER), OLDER);
        write_executable(
            &world.path().join("fakebin/uname"),
            "#!/bin/sh\ncase \"$1\" in\n-s) echo \"$FAKE_UNAME_S\" ;;\n-m) echo \"$FAKE_UNAME_M\" ;;\n*) exit 1 ;;\nesac\n",
        );
        write_executable(
            &world.path().join("fakebin/id"),
            "#!/bin/sh\n[ \"$1\" = -u ] || exit 1\necho \"$FAKE_UID\"\n",
        );
        std::fs::create_dir_all(world.home()).unwrap();
        std::fs::create_dir_all(world.tmp()).unwrap();
        world
    }

    /// Every released asset in `dir`: a stand-in that prints `version` and its target, and
    /// their SHA256SUMS.
    fn release_at(&self, dir: &Path, version: &str) {
        let mut sums = String::new();
        for target in TARGETS {
            let name = format!("or2-pair-{target}");
            let text = format!("#!/bin/sh\necho 'or2-pair {version}'\necho '{target}' >&2\n");
            write_executable(&dir.join(&name), &text);
            sums.push_str(&format!("{}  {name}\n", hex(text.as_bytes())));
        }
        std::fs::write(dir.join("SHA256SUMS"), sums).unwrap();
    }

    /// Replaces one asset of the release in `dir`, with a checksum that matches.
    fn replace_asset(&self, dir: &Path, target: &str, text: &str) {
        let name = format!("or2-pair-{target}");
        write_executable(&dir.join(&name), text);
        let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
        let sums: String = sums
            .lines()
            .map(|line| {
                if line.ends_with(&format!("  {name}")) {
                    format!("{}  {name}\n", hex(text.as_bytes()))
                } else {
                    format!("{line}\n")
                }
            })
            .collect();
        std::fs::write(dir.join("SHA256SUMS"), sums).unwrap();
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn releases(&self) -> PathBuf {
        self.path().join("releases")
    }

    fn latest_dir(&self) -> PathBuf {
        self.releases().join("latest/download")
    }

    fn tag_dir(&self, version: &str) -> PathBuf {
        self.releases().join(format!("download/v{version}"))
    }

    fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    fn tmp(&self) -> PathBuf {
        self.path().join("tmp")
    }

    fn default_dir(&self) -> PathBuf {
        self.home().join(".local/bin")
    }

    fn command(&self, os: &str, arch: &str, env: &[(&str, &str)]) -> Command {
        let path = format!(
            "{}:{}",
            self.path().join("fakebin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("sh");
        command
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.home())
            .env("TMPDIR", self.tmp())
            .env("FAKE_UNAME_S", os)
            .env("FAKE_UNAME_M", arch)
            .env("FAKE_UID", self.uid.to_string())
            .env(
                "OR2_PAIR_RELEASES_BASE",
                format!("file://{}", self.releases().display()),
            );
        for (key, value) in env {
            command.env(key, value);
        }
        command
    }

    /// Runs the installer as `os`/`arch` with `args`.
    fn install(&self, os: &str, arch: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.command(os, arch, env)
            .arg(script())
            .args(args)
            .output()
            .unwrap()
    }

    /// Runs `text` as the script, piped into `sh` as `curl ... | sh` does.
    fn piped(&self, text: &str) -> Output {
        let mut child = self
            .command("Linux", "x86_64", &[])
            .arg("-s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// The names in `dir` that are not `or2-pair` (staging files), and whatever is in `TMPDIR`.
    fn leftovers(&self, dir: &Path) -> Vec<String> {
        let names = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut found: Vec<String> = names(dir)
            .into_iter()
            .filter(|name| name.starts_with(".or2-pair"))
            .collect();
        found.extend(names(&self.tmp()));
        found
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The version line and target of the binary installed in `dir` (the stand-in prints them).
fn installed(dir: &Path) -> (String, String) {
    let output = Command::new(dir.join("or2-pair"))
        .arg("--version")
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    )
}

#[test]
fn a_matching_checksum_installs_the_binary_for_this_host() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    // The latest release, into ~/.local/bin, created.
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(output.status.success(), "{said}");
    assert_eq!(
        installed(&world.default_dir()),
        (
            format!("or2-pair {LATEST}"),
            "x86_64-unknown-linux-musl".to_owned()
        )
    );
    assert!(said.contains("(the latest release)"), "{said}");
    assert!(said.contains("Checksum OK"), "{said}");
    assert!(
        said.contains(&format!("Installed or2-pair {LATEST}")),
        "{said}"
    );
    assert!(!said.contains("root"), "{said}");
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
    // No staging file beside it, nothing left in TMPDIR.
    assert_eq!(world.leftovers(&world.default_dir()), Vec::<String>::new());

    // Each OS and CPU gets its own binary, from the latest release or a named one; --dir and
    // $OR2_PAIR_INSTALL_DIR.
    for (os, arch, version, target) in [
        ("Linux", "aarch64", None, "aarch64-unknown-linux-musl"),
        (
            "Linux",
            "arm64",
            Some("v9.9.9"),
            "aarch64-unknown-linux-musl",
        ),
        ("Linux", "amd64", Some("9.9.9"), "x86_64-unknown-linux-musl"),
        ("Darwin", "arm64", None, "aarch64-apple-darwin"),
        ("Darwin", "x86_64", Some("v9.9.9"), "x86_64-apple-darwin"),
    ] {
        let dir = world.path().join(format!("bin-{os}-{arch}"));
        let mut args = vec!["--dir", dir.to_str().unwrap()];
        if let Some(version) = version {
            args.extend(["--version", version]);
        }
        let output = world.install(os, arch, &args, &[]);
        assert!(output.status.success(), "{os} {arch}: {}", text(&output));
        assert_eq!(installed(&dir).1, target);
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
    assert_eq!(installed(&env_dir).1, "x86_64-unknown-linux-musl");
    // On PATH: no hint, and the plain name.
    assert!(!said.contains("is not on your PATH"), "{said}");
    assert!(said.ends_with("  or2-pair\n"), "{said}");
}

#[test]
fn version_picks_that_release_and_a_missing_one_is_named() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    for spelling in ["v9.9.8", "9.9.8", "--version=v9.9.8"] {
        let args: Vec<&str> = if spelling.starts_with("--") {
            vec![spelling]
        } else {
            vec!["--version", spelling]
        };
        let output = world.install("Linux", "x86_64", &args, &[]);
        let said = text(&output);
        assert!(output.status.success(), "{spelling}: {said}");
        assert!(said.contains("(v9.9.8)"), "{said}");
        assert_eq!(installed(&world.default_dir()).0, "or2-pair 9.9.8");
    }
    // An upgrade to the latest replaces it.
    let output = world.install("Linux", "x86_64", &[], &[]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(installed(&world.default_dir()).0, "or2-pair 9.9.9");

    // A version that was never released: said so, and the installed one stays.
    let output = world.install("Linux", "x86_64", &["--version", "v1.2.3"], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("version v1.2.3 does not exist"), "{said}");
    assert_eq!(installed(&world.default_dir()).0, "or2-pair 9.9.9");
    // Not a version at all: refused before any download.
    for bad in ["latest", "v1/../../x", "or2-pair-v0.1.0", ""] {
        let output = world.install("Linux", "x86_64", &["--version", bad], &[]);
        let said = text(&output);
        if bad.is_empty() {
            // An empty --version is the latest.
            assert!(output.status.success(), "{said}");
            continue;
        }
        assert!(!output.status.success(), "{bad}: {said}");
        assert!(said.contains("not a version"), "{said}");
        assert!(!said.contains("Downloading"), "{said}");
    }
    // A release whose binary is not the version it is filed under: refused.
    world.replace_asset(
        &world.tag_dir(OLDER),
        "x86_64-unknown-linux-musl",
        "#!/bin/sh\necho 'or2-pair 9.9.7'\n",
    );
    let output = world.install("Linux", "x86_64", &["--version", "v9.9.8"], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("not or2-pair 9.9.8"), "{said}");
    assert_eq!(installed(&world.default_dir()).0, "or2-pair 9.9.9");

    // No release at all yet.
    std::fs::remove_dir_all(world.releases().join("latest")).unwrap();
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("no release found"), "{said}");
    assert_eq!(world.leftovers(&world.default_dir()), Vec::<String>::new());
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    // The binary changed after SHA256SUMS was written.
    let asset = world
        .latest_dir()
        .join("or2-pair-x86_64-unknown-linux-musl");
    let mut bytes = std::fs::read(&asset).unwrap();
    bytes.extend_from_slice(b"# tampered\n");
    std::fs::write(&asset, bytes).unwrap();
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("checksum mismatch for or2-pair-x86_64-unknown-linux-musl"),
        "{said}"
    );
    assert!(said.contains("nothing was installed"), "{said}");
    assert!(!world.default_dir().join("or2-pair").exists());
    assert_eq!(world.leftovers(&world.default_dir()), Vec::<String>::new());

    // SHA256SUMS without this asset's line: refused too.
    std::fs::write(
        world.latest_dir().join("SHA256SUMS"),
        format!("{}  or2-pair-aarch64-apple-darwin\n", "0".repeat(64)),
    )
    .unwrap();
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("lists no or2-pair-x86_64-unknown-linux-musl"),
        "{said}"
    );
    assert!(!world.default_dir().join("or2-pair").exists());

    // A release without this host's binary.
    std::fs::remove_file(
        world
            .tag_dir(OLDER)
            .join("or2-pair-aarch64-unknown-linux-musl"),
    )
    .unwrap();
    let output = world.install("Linux", "aarch64", &["--version", "v9.9.8"], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(
        said.contains("v9.9.8 has no or2-pair-aarch64-unknown-linux-musl"),
        "{said}"
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
        ("Darwin", "ppc", "unsupported CPU architecture: ppc"),
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

#[test]
fn only_https_or_a_local_copy_is_fetched() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    for base in [
        "http://example.invalid/releases",
        "ftp://example.invalid/releases",
    ] {
        let output = world.install("Linux", "x86_64", &[], &[("OR2_PAIR_RELEASES_BASE", base)]);
        let said = text(&output);
        assert!(!output.status.success(), "{said}");
        assert!(said.contains(&format!("refusing {base}")), "{said}");
        assert!(said.contains("https://"), "{said}");
        assert!(!said.contains("Downloading"), "{said}");
    }
}

/// `curl ... | sh` that is cut off at any line runs nothing: no binary, no staging file, nothing
/// left in `TMPDIR`. Only the whole script installs.
#[test]
fn a_truncated_script_installs_nothing() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    let script = std::fs::read_to_string(script()).unwrap();
    let lines: Vec<&str> = script.split_inclusive('\n').collect();
    assert_eq!(lines.last().map(|line| line.trim()), Some("main \"$@\""));
    for cut in 0..lines.len() {
        let prefix = lines[..cut].concat();
        let output = world.piped(&prefix);
        assert!(
            !world.default_dir().join("or2-pair").exists(),
            "cut after line {cut}: {}",
            text(&output)
        );
        assert_eq!(
            world.leftovers(&world.default_dir()),
            Vec::<String>::new(),
            "cut after line {cut}"
        );
        assert!(
            !text(&output).contains("Downloading"),
            "cut after line {cut}"
        );
    }
    let output = world.piped(&script);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(installed(&world.default_dir()).0, "or2-pair 9.9.9");
}

/// Nothing already in the directory is followed: a link at a staging-like name or at
/// `or2-pair` itself leaves its target alone, and a new regular file takes `or2-pair`'s place.
#[test]
fn a_planted_link_is_never_followed() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    let dir = world.default_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let victim = world.path().join("victim");
    std::fs::write(&victim, "precious\n").unwrap();
    let planted = [
        ".or2-pair.new",
        ".or2-pair.XXXXXXXX",
        ".or2-pair.tmp",
        ".or2-pair.new.1",
    ];
    for name in planted {
        std::os::unix::fs::symlink(&victim, dir.join(name)).unwrap();
    }
    std::os::unix::fs::symlink(&victim, dir.join("or2-pair")).unwrap();
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(output.status.success(), "{said}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious\n");
    let meta = std::fs::symlink_metadata(dir.join("or2-pair")).unwrap();
    assert!(meta.file_type().is_file(), "{meta:?}");
    assert_eq!(installed(&dir).0, "or2-pair 9.9.9");
    for name in planted {
        assert_eq!(std::fs::read_link(dir.join(name)).unwrap(), victim);
    }
    let mut left = world.leftovers(&dir);
    left.sort();
    let mut expected: Vec<String> = planted.iter().map(|name| (*name).to_owned()).collect();
    expected.sort();
    assert_eq!(left, expected);
}

/// A checked binary that does not run here replaces nothing.
#[test]
fn a_binary_that_does_not_run_leaves_the_old_one() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    let dir = world.default_dir();
    let old = "#!/bin/sh\necho 'or2-pair 0.0.1'\n";
    write_executable(&dir.join("or2-pair"), old);
    for broken in ["#!/bin/sh\nexit 1\n", "#!/bin/sh\necho 'something else'\n"] {
        world.replace_asset(&world.latest_dir(), "x86_64-unknown-linux-musl", broken);
        let output = world.install("Linux", "x86_64", &[], &[]);
        let said = text(&output);
        assert!(!output.status.success(), "{said}");
        assert!(said.contains("Checksum OK"), "{said}");
        assert!(
            said.contains("does not run on this host; nothing was replaced"),
            "{said}"
        );
        assert_eq!(std::fs::read_to_string(dir.join("or2-pair")).unwrap(), old);
        assert_eq!(world.leftovers(&dir), Vec::<String>::new());
    }
}

/// Destinations that are refused before anything is downloaded.
#[test]
fn an_unusable_destination_is_refused() {
    if !have_curl() {
        return;
    }
    let world = World::new();
    // A directory where the binary would go.
    let dir = world.default_dir();
    std::fs::create_dir_all(dir.join("or2-pair")).unwrap();
    let output = world.install("Linux", "x86_64", &[], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("or2-pair is a directory"), "{said}");
    assert!(!said.contains("Downloading"), "{said}");
    assert!(dir.join("or2-pair").is_dir());

    // A directory anyone can write.
    let open = world.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let output = world.install("Linux", "x86_64", &["--dir", open.to_str().unwrap()], &[]);
    let said = text(&output);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("anyone can write it"), "{said}");
    assert!(!open.join("or2-pair").exists());

    // As root: a notice whose account pairing will be for, and a directory that is not root's
    // (or that group or others may write) is refused.
    if world.uid != 0 {
        let mine = world.path().join("mine");
        let output = world.install(
            "Linux",
            "x86_64",
            &["--dir", mine.to_str().unwrap()],
            &[("FAKE_UID", "0")],
        );
        let said = text(&output);
        assert!(!output.status.success(), "{said}");
        assert!(said.contains("pairs the root account"), "{said}");
        assert!(said.contains("it must belong to root"), "{said}");
        assert!(!said.contains("Downloading"), "{said}");
        assert!(!mine.join("or2-pair").exists());
    }
}
