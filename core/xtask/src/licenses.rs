//! `cargo xtask gen-licenses`: generates or2's open-source licence data.
//!
//! Outputs (all checked in; `--check` fails when one is stale):
//!
//!   android/app/src/main/assets/licenses/rust.json     Rust crates linked into libor2_ffi.so for
//!                                                      aarch64-linux-android, plus the Zig-built
//!                                                      libghostty-vt and the Rust standard library
//!   android/app/src/main/assets/licenses/android.json  the release runtime classpath (Gradle)
//!   android/app/src/main/assets/licenses/notices.md    copy of THIRD_PARTY_NOTICES.md
//!   android/app/src/main/assets/licenses/COPYING       copy of LICENSE (GPL-3.0 text)
//!   core/or2-pair/THIRD_PARTY.md                       crates linked into the or2-pair CLI (only
//!                                                      when that package exists in the workspace)
//!
//! Sources: `cargo metadata --locked --offline` (crate set, licence expressions, repositories) and
//! the licence/notice files inside each crate's source in the local cargo registry or git checkout;
//! `android/app/gradle.lockfile` (strict lock: every artifact of releaseRuntimeClasspath) and each
//! artifact's POM from the offline Gradle cache. A crate that ships no licence file falls back to
//! the SPDX standard text in core/xtask/licenses/spdx/ and is flagged `fallback`. Nothing is
//! downloaded.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::json::{dumps, string};
use crate::pom::parse_pom;
use crate::zip::Archive;
use crate::{Result, fail, repo_root};

pub const USAGE: &str = "\
Usage: cargo xtask gen-licenses [--check]
  --check  write nothing; fail if a generated file differs from a regeneration

Needs cargo (registry and git sources already fetched: it runs `cargo metadata --locked
--offline`) and, for android.json, the Gradle cache of a build that resolved the locked
dependencies. No network, no extra tools. Never edit the generated files.";

const ANDROID_TARGET: &str = "aarch64-linux-android";
const FFI_PACKAGE: &str = "or2-ffi";
const CLI_PACKAGE: &str = "or2-pair";
const GENERATOR: &str = "scripts/gen-licenses.sh";

/// The Ghostty commit libghostty-vt-sys builds (its build.rs `GHOSTTY_COMMIT`). The texts under
/// core/xtask/licenses/ghostty/ were read from that commit; if the pin moves the generator stops
/// until the components below are re-read.
const GHOSTTY_COMMIT: &str = "22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018";
const MAX_TEXT_BYTES: u64 = 200_000;

struct Dirs {
    root: PathBuf,
    core: PathBuf,
    assets: PathBuf,
    spdx: PathBuf,
    ghostty: PathBuf,
    libyuv: PathBuf,
    lockfile: PathBuf,
}

impl Dirs {
    fn new() -> Self {
        let root = repo_root();
        let data = root.join("core/xtask/licenses");
        Self {
            core: root.join("core"),
            assets: root.join("android/app/src/main/assets/licenses"),
            spdx: data.join("spdx"),
            ghostty: data.join("ghostty"),
            libyuv: data.join("libyuv"),
            lockfile: root.join("android/app/gradle.lockfile"),
            root,
        }
    }
}

pub fn run(args: &[String]) -> Result<()> {
    let check = match args {
        [] => false,
        [flag] if flag == "--check" => true,
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let dirs = Dirs::new();
    let generated = outputs(&dirs)?;
    let mut stale = Vec::new();
    for (path, content) in &generated {
        let current = fs::read_to_string(path).ok();
        if current.as_deref() != Some(content.as_str()) {
            stale.push(path);
            if !check {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    let relative = |p: &&PathBuf| {
        p.strip_prefix(&dirs.root)
            .unwrap_or(p)
            .display()
            .to_string()
    };
    if check {
        if !stale.is_empty() {
            let names: Vec<_> = stale.iter().map(relative).collect();
            return fail(format!(
                "stale: {} (run cargo xtask gen-licenses)",
                names.join(", ")
            ));
        }
        println!("gen-licenses: {} files up to date", generated.len());
    } else {
        println!(
            "gen-licenses: wrote {} of {} files",
            stale.len(),
            generated.len()
        );
    }
    Ok(())
}

// --- text helpers ---------------------------------------------------------------------------

fn normalise(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", unified.trim_matches('\n'))
}

fn read_text(path: &Path) -> Result<String> {
    let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if data.len() as u64 > MAX_TEXT_BYTES {
        return fail(format!(
            "{} is {} bytes; refusing to embed a licence file that large",
            path.display(),
            data.len()
        ));
    }
    Ok(normalise(&String::from_utf8_lossy(&data)))
}

/// Whether a file name looks like a licence or notice file: `LICENSE`, `LICENCE-MIT`, `COPYING.txt`,
/// `NOTICE`, `UNLICENSE`, `COPYRIGHT ...` (case-insensitive; a `-`, `_`, `.` or space starts the
/// suffix).
fn is_license_file_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "license",
        "licence",
        "copying",
        "notice",
        "unlicense",
        "copyright",
    ]
    .iter()
    .any(|stem| {
        lower
            .strip_prefix(stem)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_', '.', ' ']))
    })
}

/// Licence texts deduplicated by content; packages reference them by id.
#[derive(Default)]
struct Texts {
    by_id: BTreeMap<String, String>,
}

impl Texts {
    fn add(&mut self, text: &str) -> String {
        let text = normalise(text);
        let digest = Sha256::digest(text.as_bytes());
        let id: String = digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..12]
            .to_string();
        self.by_id.insert(id.clone(), text);
        id
    }
}

fn spdx_text(dirs: &Dirs, identifier: &str) -> Result<String> {
    let path = dirs.spdx.join(format!("{identifier}.txt"));
    if !path.is_file() {
        return fail(format!(
            "no standard text for SPDX id {identifier:?}: add core/xtask/licenses/spdx/{identifier}.txt"
        ));
    }
    read_text(&path)
}

/// The licence identifiers in an SPDX expression, without operators and parentheses.
fn spdx_ids(expression: &str) -> Vec<&str> {
    expression
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .filter(|t| !t.is_empty() && !matches!(t.to_uppercase().as_str(), "OR" | "AND" | "WITH"))
        .collect()
}

/// One licence text of a package: where it came from and its id in the shared text table.
struct TextRef {
    id: String,
    file: String,
}

/// One row of a licence document.
struct Entry {
    name: String,
    /// Maven artifacts only.
    title: Option<String>,
    version: String,
    license: String,
    repository: String,
    source: &'static str,
    texts: Vec<TextRef>,
    fallback: bool,
    note: Option<String>,
}

impl Entry {
    fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), json!(self.name));
        if let Some(title) = &self.title {
            map.insert("title".into(), json!(title));
        }
        map.insert("version".into(), json!(self.version));
        map.insert("license".into(), json!(self.license));
        map.insert("repository".into(), json!(self.repository));
        map.insert("source".into(), json!(self.source));
        let texts: Vec<Value> = self
            .texts
            .iter()
            .map(|t| json!({"id": t.id, "file": t.file}))
            .collect();
        map.insert("texts".into(), Value::Array(texts));
        if self.fallback {
            map.insert("fallback".into(), json!(true));
        }
        if let Some(note) = &self.note {
            map.insert("note".into(), json!(note));
        }
        Value::Object(map)
    }
}

// --- Rust -----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Package {
    id: String,
    name: String,
    version: String,
    source: Option<String>,
    license: Option<String>,
    license_file: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    manifest_path: PathBuf,
    links: Option<String>,
}

fn run_command(command: &mut Command, cwd: &Path) -> Result<String> {
    let output = command
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if !output.status.success() {
        return fail(format!(
            "{:?} failed:\n{}",
            command,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn cargo_metadata(dirs: &Dirs) -> Result<Metadata> {
    let text = run_command(
        Command::new("cargo").args(["metadata", "--locked", "--offline", "--format-version", "1"]),
        &dirs.core,
    )?;
    serde_json::from_str(&text).map_err(|e| format!("cargo metadata: {e}"))
}

/// The `(name, version)` pairs a `cargo tree --prefix none` listing names. Lines look like
/// `serde v1.0.229`, `serde_derive v1.0.229 (proc-macro)` or `or2-core v0.1.0 (/path)`.
fn parse_tree(tree: &str) -> Vec<(String, String)> {
    let mut linked = Vec::new();
    for line in tree.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(name), Some(version)) = (fields.next(), fields.next())
            && let Some(version) = version.strip_prefix('v')
        {
            let pair = (name.to_string(), version.to_string());
            if !linked.contains(&pair) {
                linked.push(pair);
            }
        }
    }
    linked
}

/// The non-member packages in `linked`, sorted by name and version. Names a member shares with a
/// listed line (the project's own crates) are skipped; any other unknown package is an error.
fn select_packages(meta: &Metadata, linked: &[(String, String)]) -> Result<Vec<Package>> {
    let is_member = |p: &Package| meta.workspace_members.contains(&p.id);
    let mut by_key: HashMap<(&str, &str), &Package> = HashMap::new();
    for package in meta.packages.iter().filter(|p| !is_member(p)) {
        by_key.insert((&package.name, &package.version), package);
    }
    let missing: Vec<_> = linked
        .iter()
        .filter(|(name, version)| {
            !by_key.contains_key(&(name.as_str(), version.as_str()))
                && !meta
                    .packages
                    .iter()
                    .any(|m| is_member(m) && &m.name == name)
        })
        .collect();
    if !missing.is_empty() {
        return fail(format!(
            "cargo tree names packages missing from cargo metadata: {missing:?}"
        ));
    }
    let mut packages: Vec<Package> = linked
        .iter()
        .filter_map(|(name, version)| by_key.get(&(name.as_str(), version.as_str())))
        .map(|p| (*p).clone())
        .collect();
    packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(packages)
}

/// The packages `cargo tree` says `root_name` links for `target` (normal edges only).
///
/// Build-script and dev dependencies ship no code and are left out; proc-macro crates stay
/// because their output is compiled into the binary. `cargo tree` resolves the features of that
/// one package, which `cargo metadata` (workspace-wide features) over-reports. Workspace members
/// are the project itself and are not listed. Returns None when the package does not exist.
fn linked_packages(
    dirs: &Dirs,
    meta: &Metadata,
    root_name: &str,
    target: &str,
) -> Result<Option<Vec<Package>>> {
    let exists = meta
        .packages
        .iter()
        .any(|p| p.name == root_name && meta.workspace_members.contains(&p.id));
    if !exists {
        return Ok(None);
    }
    let tree = run_command(
        Command::new("cargo").args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            root_name,
            "--target",
            target,
            "-e",
            "normal",
            "--prefix",
            "none",
        ]),
        &dirs.core,
    )?;
    select_packages(meta, &parse_tree(&tree)).map(Some)
}

fn source_kind(package: &Package) -> Result<&'static str> {
    let source = package.source.as_deref().unwrap_or("");
    if source.starts_with("git+") {
        Ok("git")
    } else if source.contains("crates.io") {
        Ok("crates.io")
    } else {
        fail(format!(
            "{} {} has an unexpected source {source:?}",
            package.name, package.version
        ))
    }
}

fn sorted_license_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = read_dir
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_license_file_name)
        })
        .collect();
    found.sort();
    found
}

fn license_files(package: &Package) -> Result<Vec<PathBuf>> {
    let root = package
        .manifest_path
        .parent()
        .ok_or("manifest path without a directory")?;
    let mut candidates = vec![root.to_path_buf()];
    if source_kind(package)? == "git" {
        // Workspace members of a git checkout keep one LICENSE at the repository root.
        candidates.extend(
            root.ancestors()
                .skip(1)
                .filter(|p| p.components().any(|c| c.as_os_str() == "checkouts"))
                .take(3)
                .map(Path::to_path_buf),
        );
    }
    for directory in candidates {
        let found = sorted_license_files(&directory);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

/// Every file below `root` (directories first-seen order does not matter: the caller sorts).
fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read_dir) = fs::read_dir(root) else {
        return;
    };
    for entry in read_dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        out.push(path.clone());
        if kind.is_dir() {
            walk(&path, out);
        }
    }
}

fn crate_entry(dirs: &Dirs, package: &Package, texts: &mut Texts) -> Result<Entry> {
    let license = package
        .license
        .clone()
        .filter(|l| !l.is_empty())
        .or_else(|| package.license_file.clone().filter(|l| !l.is_empty()))
        .unwrap_or_default();
    let repository = package
        .repository
        .clone()
        .filter(|l| !l.is_empty())
        .or_else(|| package.homepage.clone().filter(|l| !l.is_empty()))
        .unwrap_or_default();
    let mut entry = Entry {
        name: package.name.clone(),
        title: None,
        version: package.version.clone(),
        license,
        repository,
        source: source_kind(package)?,
        texts: Vec::new(),
        fallback: false,
        note: None,
    };
    let files = license_files(package)?;
    let root = package
        .manifest_path
        .parent()
        .ok_or("manifest path without a directory")?;
    for f in &files {
        entry.texts.push(TextRef {
            id: texts.add(&read_text(f)?),
            file: f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        });
    }
    let links = package.links.as_deref().is_some_and(|l| !l.is_empty());
    if links || package.name.ends_with("-sys") {
        // A crate that bundles C sources keeps the licences of the bundled code in subdirectories.
        let mut all = Vec::new();
        walk(root, &mut all);
        all.sort();
        for f in all {
            let Ok(relative) = f.strip_prefix(root) else {
                continue;
            };
            let parts: Vec<_> = relative.components().collect();
            let in_excluded_dir = parts[..parts.len() - 1].iter().any(|c| {
                matches!(
                    c.as_os_str().to_str(),
                    Some("tests" | "test" | "examples" | "docs")
                )
            });
            if f.is_file()
                && !files.contains(&f)
                && parts.len() <= 5
                && f.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_license_file_name)
                && !in_excluded_dir
            {
                let text_id = texts.add(&read_text(&f)?);
                if entry.texts.iter().all(|t| t.id != text_id) {
                    entry.texts.push(TextRef {
                        id: text_id,
                        file: relative.display().to_string(),
                    });
                }
            }
        }
    }
    if files.is_empty() {
        let ids = spdx_ids(&entry.license);
        if ids.is_empty() {
            return fail(format!(
                "{} {} declares no licence and ships no licence file",
                package.name, package.version
            ));
        }
        for identifier in ids {
            entry.texts.push(TextRef {
                id: texts.add(&spdx_text(dirs, identifier)?),
                file: format!("{identifier} (standard text)"),
            });
        }
        entry.fallback = true;
        entry.note = Some(
            "The crate ships no licence file; the SPDX standard text for its declared licence is shown."
                .into(),
        );
    }
    if entry.license.is_empty() {
        return fail(format!(
            "{} {} declares no licence expression",
            package.name, package.version
        ));
    }
    Ok(entry)
}

/// Checks the Ghostty commit `libghostty-vt-sys` builds against the one the checked-in
/// component texts were read from.
fn ghostty_pin(packages: &[Package]) -> Result<()> {
    let Some(sys) = packages.iter().find(|p| p.name == "libghostty-vt-sys") else {
        return Ok(());
    };
    let build_rs = sys
        .manifest_path
        .parent()
        .ok_or("manifest path without a directory")?
        .join("build.rs");
    let source =
        fs::read_to_string(&build_rs).map_err(|e| format!("{}: {e}", build_rs.display()))?;
    let commit = parse_ghostty_commit(&source)
        .ok_or_else(|| format!("cannot read GHOSTTY_COMMIT from {}", build_rs.display()))?;
    if commit != GHOSTTY_COMMIT {
        return fail(format!(
            "libghostty-vt-sys now builds Ghostty {commit}, not {GHOSTTY_COMMIT}: re-read the \
             Ghostty component licences, update core/xtask/licenses/ghostty/ and GHOSTTY_COMMIT in \
             core/xtask/src/licenses.rs"
        ));
    }
    Ok(())
}

/// The 40-hex commit in `GHOSTTY_COMMIT: &str = "..."` of a build script.
fn parse_ghostty_commit(build_rs: &str) -> Option<&str> {
    const MARKER: &str = "GHOSTTY_COMMIT: &str = \"";
    let rest = &build_rs[build_rs.find(MARKER)? + MARKER.len()..];
    let commit = rest.get(..40)?;
    (rest[40..].starts_with('"')
        && commit
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    .then_some(commit)
}

/// Components compiled into libghostty-vt's static archive by `zig build` (not Cargo crates).
fn zig_entries(dirs: &Dirs, texts: &mut Texts) -> Result<Vec<Entry>> {
    let mut text = |name: &str, file: &str| -> Result<TextRef> {
        Ok(TextRef {
            id: texts.add(&read_text(&dirs.ghostty.join(name))?),
            file: file.to_string(),
        })
    };
    let ghostty = text("ghostty.txt", "LICENSE")?;
    let highway = [
        text("highway-apache.txt", "LICENSE")?,
        text("highway-bsd3.txt", "LICENSE-BSD3")?,
    ];
    let simdutf_mit = text("simdutf-mit.txt", "MIT (copyright line reconstructed)")?;
    let uucode = text("uucode.txt", "LICENSE.md")?;
    let hoehrmann = text("hoehrmann.txt", "MIT (reconstructed)")?;
    let zig = text("zig.txt", "LICENSE")?;
    let mut standard = |identifier: &str| -> Result<TextRef> {
        Ok(TextRef {
            id: texts.add(&spdx_text(dirs, identifier)?),
            file: format!("{identifier} (standard text)"),
        })
    };
    let simdutf_apache = standard("Apache-2.0")?;
    let unicode = standard("Unicode-3.0")?;

    let zig_entry =
        |name: &str, version: &str, license: &str, repository: &str, texts: Vec<TextRef>| Entry {
            name: name.to_string(),
            title: None,
            version: version.to_string(),
            license: license.to_string(),
            repository: repository.to_string(),
            source: "zig",
            texts,
            fallback: false,
            note: None,
        };
    let mut entries = vec![
        zig_entry(
            "Ghostty (libghostty-vt)",
            &format!("commit {}", &GHOSTTY_COMMIT[..12]),
            "MIT",
            "https://github.com/ghostty-org/ghostty",
            vec![ghostty],
        ),
        zig_entry(
            "Highway (in libghostty-vt)",
            "1.2.0 (commit 66486a10623f)",
            "Apache-2.0 OR BSD-3-Clause",
            "https://github.com/google/highway",
            highway.into(),
        ),
        zig_entry(
            "simdutf (in libghostty-vt)",
            "5.2.8",
            "Apache-2.0 OR MIT",
            "https://github.com/simdutf/simdutf",
            vec![simdutf_apache, simdutf_mit],
        ),
        zig_entry(
            "uucode (in libghostty-vt)",
            "0.2.0 (commit 2826a37a4562)",
            "MIT",
            "https://github.com/jacobsandlund/uucode",
            vec![uucode, unicode],
        ),
        zig_entry(
            "Bjoern Hoehrmann's UTF-8 decoder (in libghostty-vt)",
            "n/a",
            "MIT",
            "http://bjoern.hoehrmann.de/utf-8/decoder/dfa",
            vec![hoehrmann],
        ),
        zig_entry(
            "Zig compiler runtime (in libghostty-vt)",
            "0.16.0",
            "MIT",
            "https://github.com/ziglang/zig",
            vec![zig],
        ),
    ];
    entries[0].note = Some(format!(
        "Built from Ghostty commit {GHOSTTY_COMMIT} by libghostty-rs with Zig 0.16.0 and statically linked."
    ));
    entries[2].fallback = true;
    entries[2].note = Some("Vendored in Ghostty's pkg/simdutf as a single header; its licence files are not in the tree, so the standard texts are shown.".into());
    entries[3].note = Some("Unicode tables generated from the Unicode Character Database, under the Unicode License v3.".into());
    entries[4].fallback = true;
    entries[4].note = Some("Ghostty's UTF8Decoder.zig is based on this decoder; the licence text is the MIT standard text with the author's copyright line.".into());
    entries[5].note = Some(
        "compiler_rt and standard-library code that zig build links into the static archive."
            .into(),
    );
    Ok(entries)
}

fn rust_std_entry(dirs: &Dirs, texts: &mut Texts) -> Result<Entry> {
    let mit = spdx_text(dirs, "MIT")?.replace(
        "<year> <copyright holders>",
        "The Rust Project Contributors",
    );
    Ok(Entry {
        name: "Rust standard library".into(),
        title: None,
        version: "core, alloc, std, compiler-builtins, libunwind".into(),
        license: "MIT OR Apache-2.0".into(),
        repository: "https://github.com/rust-lang/rust".into(),
        source: "rustup",
        texts: vec![
            TextRef {
                id: texts.add(&spdx_text(dirs, "Apache-2.0")?),
                file: "LICENSE-APACHE (standard text)".into(),
            },
            TextRef {
                id: texts.add(&mit),
                file: "LICENSE-MIT (standard text)".into(),
            },
        ],
        fallback: true,
        note: Some("Statically linked into every Rust binary; the toolchain's COPYRIGHT file lists its vendored parts.".into()),
    })
}

/// SPDX id to text id, for the in-app view of vendored notices.
fn standard_map(dirs: &Dirs, texts: &mut Texts) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    for identifier in ["Apache-2.0", "MIT"] {
        map.insert(
            identifier.into(),
            json!(texts.add(&spdx_text(dirs, identifier)?)),
        );
    }
    Ok(map)
}

/// Entries for the crates, the Zig-built libghostty-vt components and the Rust standard library.
fn linked_entries(dirs: &Dirs, packages: &[Package], texts: &mut Texts) -> Result<Vec<Entry>> {
    let mut entries = packages
        .iter()
        .map(|p| crate_entry(dirs, p, texts))
        .collect::<Result<Vec<_>>>()?;
    if packages.iter().any(|p| p.name == "libghostty-vt-sys") {
        ghostty_pin(packages)?;
        entries.extend(zig_entries(dirs, texts)?);
    }
    entries.push(rust_std_entry(dirs, texts)?);
    entries.sort_by_key(|e| (e.name.to_lowercase(), e.version.clone()));
    Ok(entries)
}

fn rust_document(dirs: &Dirs, packages: &[Package], target: &str) -> Result<String> {
    let mut texts = Texts::default();
    let entries = linked_entries(dirs, packages, &mut texts)?;
    let standard = standard_map(dirs, &mut texts)?;
    Ok(dump_document(
        vec![
            ("schema", json!(1)),
            ("group", json!("rust")),
            ("target", json!(target)),
            ("generator", json!(GENERATOR)),
            ("standard", Value::Object(standard)),
        ],
        &entries,
        &texts,
    ))
}

// --- Android --------------------------------------------------------------------------------

/// The SPDX identifier for a licence name as Maven POMs spell it.
fn spdx_by_pom_name(name: &str) -> Option<&'static str> {
    Some(match name.trim().to_lowercase().as_str() {
        "apache-2.0"
        | "apache 2.0"
        | "apache license 2.0"
        | "apache license, version 2.0"
        | "apache license v2.0"
        | "the apache license, version 2.0"
        | "the apache software license, version 2.0" => "Apache-2.0",
        "mit" | "mit license" | "the mit license" => "MIT",
        "lgpl, version 2.1" | "lgpl-2.1-or-later" | "gnu lesser general public license" => {
            "LGPL-2.1-or-later"
        }
        _ => return None,
    })
}

/// A POM that lists a second licence for code bundled in the artifact (not an alternative) is
/// matched by the licence URL and applied with AND, with the project's own text. camera-core
/// bundles libyuv in its native image-processing library and says so with a "BSD License" entry.
struct Bundled {
    identifier: &'static str,
    path: PathBuf,
    label: &'static str,
}

fn bundled_licence(dirs: &Dirs, url: &str) -> Option<Bundled> {
    const LIBYUV: &str = "https://chromium.googlesource.com/libyuv/libyuv/";
    url.starts_with(LIBYUV).then(|| Bundled {
        identifier: "BSD-3-Clause",
        path: dirs.libyuv.join("LICENSE"),
        label: "libyuv/LICENSE (bundled libyuv)",
    })
}

/// The SPDX expression of an artifact: its own licences are alternatives (`OR`), each bundled
/// licence is an additional obligation (`AND`).
fn license_expression(resolved: &[&str], bundled: &[&str]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for identifier in resolved {
        if !unique.contains(identifier) {
            unique.push(identifier);
        }
    }
    let mut expression = unique.join(" OR ");
    for identifier in bundled {
        expression = if expression.contains(" OR ") {
            format!("({expression}) AND {identifier}")
        } else {
            format!("{expression} AND {identifier}")
        };
    }
    expression
}

fn gradle_home() -> PathBuf {
    match std::env::var_os("GRADLE_USER_HOME") {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".gradle"),
    }
}

/// `(group, artifact, version)` of every lockfile line that lists `configuration`, sorted.
fn lockfile_artifacts(lockfile: &str, configuration: &str) -> Vec<(String, String, String)> {
    let mut found = Vec::new();
    for line in lockfile.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("empty=") {
            continue;
        }
        let (coordinate, configurations) = line.split_once('=').unwrap_or((line, ""));
        if configurations.split(',').any(|c| c == configuration) {
            let mut parts = coordinate.splitn(3, ':');
            if let (Some(group), Some(artifact), Some(version)) =
                (parts.next(), parts.next(), parts.next())
            {
                found.push((group.to_string(), artifact.to_string(), version.to_string()));
            }
        }
    }
    found.sort();
    found
}

fn cache_dir(group: &str, artifact: &str, version: &str) -> PathBuf {
    gradle_home()
        .join("caches/modules-2/files-2.1")
        .join(group)
        .join(artifact)
        .join(version)
}

/// `<dir>/<hash>/<artifact>-<version><suffix>` for the first suffix that exists (first hash
/// directory in name order).
fn find_in_cache(group: &str, artifact: &str, version: &str, suffixes: &[&str]) -> Option<PathBuf> {
    let directory = cache_dir(group, artifact, version);
    let mut hashes: Vec<PathBuf> = fs::read_dir(&directory)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    hashes.sort();
    suffixes.iter().find_map(|suffix| {
        hashes
            .iter()
            .map(|hash| hash.join(format!("{artifact}-{version}{suffix}")))
            .find(|candidate| candidate.is_file())
    })
}

struct PomInfo {
    name: String,
    url: String,
    licences: Vec<crate::pom::PomLicense>,
    packaging: String,
}

/// The POM of an artifact, inheriting licences from its parent chain.
fn read_pom(group: &str, artifact: &str, version: &str, depth: usize) -> Result<PomInfo> {
    let Some(path) = find_in_cache(group, artifact, version, &[".pom"]) else {
        return fail(format!(
            "no POM for {group}:{artifact}:{version} in the Gradle cache ({})",
            cache_dir(group, artifact, version).display()
        ));
    };
    let xml = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pom = parse_pom(&xml).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut licences = pom.licenses;
    if licences.is_empty()
        && depth < 5
        && let Some((g, a, v)) = &pom.parent
    {
        licences = read_pom(g, a, v, depth + 1)?.licences;
    }
    Ok(PomInfo {
        name: pom.name,
        url: pom.url,
        licences,
        packaging: pom.packaging,
    })
}

/// LICENSE/NOTICE files at the root (or META-INF) of the artifact's own jar/aar.
fn archive_notices(group: &str, artifact: &str, version: &str) -> Result<Vec<(String, String)>> {
    let Some(path) = find_in_cache(group, artifact, version, &[".aar", ".jar"]) else {
        return Ok(Vec::new());
    };
    let data = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let archive = Archive::open(&data).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut names: Vec<_> = archive.entries.iter().collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    let mut found = Vec::new();
    for entry in names {
        let (parent, base) = match entry.name.rsplit_once('/') {
            Some((parent, base)) => (parent, base),
            None => ("", entry.name.as_str()),
        };
        if matches!(parent, "" | "META-INF")
            && is_license_file_name(base)
            && !entry.name.ends_with('/')
            && entry.size <= MAX_TEXT_BYTES
        {
            let body = archive.read(entry)?;
            found.push((
                entry.name.clone(),
                normalise(&String::from_utf8_lossy(&body)),
            ));
        }
    }
    Ok(found)
}

fn android_document(dirs: &Dirs) -> Result<String> {
    let mut texts = Texts::default();
    let mut entries = Vec::new();
    let lockfile = fs::read_to_string(&dirs.lockfile)
        .map_err(|e| format!("{}: {e}", dirs.lockfile.display()))?;
    for (group, artifact, version) in lockfile_artifacts(&lockfile, "releaseRuntimeClasspath") {
        let pom = read_pom(&group, &artifact, &version, 0)?;
        if pom.packaging == "pom" {
            continue; // a BOM or aggregator: no code ships
        }
        if pom.licences.is_empty() {
            return fail(format!(
                "{group}:{artifact}:{version} declares no licence in its POM or its parents"
            ));
        }
        let (mut resolved, mut bundled, mut entry_texts) = (Vec::new(), Vec::new(), Vec::new());
        for lic in &pom.licences {
            if let Some(extra) = bundled_licence(dirs, &lic.url) {
                bundled.push(extra.identifier);
                entry_texts.push(TextRef {
                    id: texts.add(&read_text(&extra.path)?),
                    file: extra.label.to_string(),
                });
                continue;
            }
            let Some(identifier) = spdx_by_pom_name(&lic.name) else {
                return fail(format!(
                    "{group}:{artifact}:{version}: unknown POM licence {:?} ({}); classify it in spdx_by_pom_name",
                    lic.name, lic.url
                ));
            };
            resolved.push(identifier);
            entry_texts.push(TextRef {
                id: texts.add(&spdx_text(dirs, identifier)?),
                file: format!("{identifier} (standard text)"),
            });
        }
        // The library's own notices, when it ships any, come after the standard texts.
        for (member, body) in archive_notices(&group, &artifact, &version)? {
            entry_texts.push(TextRef {
                id: texts.add(&body),
                file: member,
            });
        }
        let mut entry = Entry {
            name: format!("{group}:{artifact}"),
            title: Some(if !pom.name.is_empty() && !pom.name.contains("${") {
                pom.name.clone()
            } else {
                artifact.clone()
            }),
            version,
            license: license_expression(&resolved, &bundled),
            repository: pom.url.clone(),
            source: "maven",
            texts: Vec::new(),
            fallback: false,
            note: None,
        };
        for t in entry_texts {
            if entry.texts.iter().all(|seen| seen.id != t.id) {
                entry.texts.push(t);
            }
        }
        entries.push(entry);
    }
    let standard = standard_map(dirs, &mut texts)?;
    Ok(dump_document(
        vec![
            ("schema", json!(1)),
            ("group", json!("android")),
            ("configuration", json!("releaseRuntimeClasspath")),
            ("generator", json!(GENERATOR)),
            ("standard", Value::Object(standard)),
        ],
        &entries,
        &texts,
    ))
}

// --- output ---------------------------------------------------------------------------------

/// The licence JSON shape: header keys in the given order, then one package per line, then the
/// text table (sorted by id).
fn dump_document(head: Vec<(&str, Value)>, packages: &[Entry], texts: &Texts) -> String {
    let mut lines = vec!["{".to_string()];
    for (key, value) in head {
        lines.push(format!("{}: {},", string(key), dumps(&value, None, false)));
    }
    lines.push("\"packages\": [".into());
    let packages: Vec<String> = packages
        .iter()
        .map(|p| dumps(&p.to_value(), None, false))
        .collect();
    lines.push(packages.join(",\n"));
    lines.push("],".into());
    lines.push("\"texts\": {".into());
    let table: Vec<String> = texts
        .by_id
        .iter()
        .map(|(k, v)| format!("{}: {}", string(k), string(v)))
        .collect();
    lines.push(table.join(",\n"));
    lines.push("}".into());
    lines.push("}".into());
    lines.join("\n") + "\n"
}

fn third_party_markdown(dirs: &Dirs, packages: &[Package], package_name: &str) -> Result<String> {
    let mut texts = Texts::default();
    let entries = linked_entries(dirs, packages, &mut texts)?;
    // Text id -> the entries using it, in first-use order.
    let mut used_by: Vec<(String, Vec<String>)> = Vec::new();
    for e in &entries {
        for t in &e.texts {
            let label = format!("{} {} ({})", e.name, e.version, t.file);
            match used_by.iter_mut().find(|(id, _)| *id == t.id) {
                Some((_, users)) => users.push(label),
                None => used_by.push((t.id.clone(), vec![label])),
            }
        }
    }
    let mut order: Vec<&(String, Vec<String>)> = used_by.iter().collect();
    order.sort_by_key(|(_, users)| users[0].to_lowercase());
    let numbers: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(n, (id, _))| (id.as_str(), n + 1))
        .collect();
    let mut out: Vec<String> = vec![
        format!("# Third-party software in {package_name}"),
        String::new(),
        format!("Generated by `{GENERATOR}`; do not edit. The crates and components linked into the"),
        format!("`{package_name}` binary and the licence texts they ship; the numbers refer to the texts below. A"),
        "`standard` text is the SPDX text for a declared licence the package does not ship itself. or2's".into(),
        "own code is GPL-3.0-or-later (see `LICENSE`).".into(),
        String::new(),
        "| Name | Version | Licence | Source | Texts |".into(),
        "|---|---|---|---|---|".into(),
    ];
    for e in &entries {
        let refs: Vec<String> = e
            .texts
            .iter()
            .map(|t| numbers[t.id.as_str()].to_string())
            .collect();
        let source = if e.repository.is_empty() {
            e.source
        } else {
            &e.repository
        };
        out.push(format!(
            "| {} | {} | {} | {} | {} |",
            e.name,
            e.version,
            e.license,
            source,
            refs.join(", ")
        ));
    }
    out.push(String::new());
    out.push("## Licence texts".into());
    out.push(String::new());
    for (id, users) in order {
        out.push(format!("### {}", numbers[id.as_str()]));
        out.push(String::new());
        out.push(format!("Used by: {}", users.join("; ")));
        out.push(String::new());
        out.push("```text".into());
        out.push(texts.by_id[id].trim_end_matches('\n').to_string());
        out.push("```".into());
        out.push(String::new());
    }
    Ok(out.join("\n").trim_end_matches('\n').to_string() + "\n")
}

fn outputs(dirs: &Dirs) -> Result<Vec<(PathBuf, String)>> {
    let mut result = Vec::new();
    let meta = cargo_metadata(dirs)?;
    let Some(ffi) = linked_packages(dirs, &meta, FFI_PACKAGE, ANDROID_TARGET)? else {
        return fail(format!("workspace has no {FFI_PACKAGE} package"));
    };
    result.push((
        dirs.assets.join("rust.json"),
        rust_document(dirs, &ffi, ANDROID_TARGET)?,
    ));
    result.push((dirs.assets.join("android.json"), android_document(dirs)?));
    let read = |name: &str| {
        let path = dirs.root.join(name);
        fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
    };
    result.push((
        dirs.assets.join("notices.md"),
        read("THIRD_PARTY_NOTICES.md")?,
    ));
    result.push((dirs.assets.join("COPYING"), read("LICENSE")?));
    if let Some(cli) = linked_packages(dirs, &meta, CLI_PACKAGE, "all")? {
        result.push((
            dirs.core.join(CLI_PACKAGE).join("THIRD_PARTY.md"),
            third_party_markdown(dirs, &cli, CLI_PACKAGE)?,
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(id: &str, name: &str, version: &str) -> Package {
        Package {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            license: Some("MIT".into()),
            license_file: None,
            repository: None,
            homepage: None,
            manifest_path: PathBuf::from("/x/Cargo.toml"),
            links: None,
        }
    }

    #[test]
    fn license_file_names() {
        for name in [
            "LICENSE",
            "license",
            "LICENCE-MIT",
            "LICENSE-APACHE",
            "COPYING",
            "COPYING.LESSER",
            "NOTICE",
            "NOTICE.md",
            "UNLICENSE",
            "COPYRIGHT",
            "License.txt",
            "LICENSE_2.0",
            "LICENSE MIT",
        ] {
            assert!(is_license_file_name(name), "{name}");
        }
        for name in [
            "LICENSES",
            "licensed.rs",
            "Cargo.toml",
            "NOTICES",
            "README.md",
            "",
        ] {
            assert!(!is_license_file_name(name), "{name}");
        }
    }

    #[test]
    fn spdx_ids_drop_operators_and_parentheses() {
        assert_eq!(spdx_ids("MIT OR Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(
            spdx_ids("(MIT OR Apache-2.0) AND Unicode-3.0"),
            ["MIT", "Apache-2.0", "Unicode-3.0"]
        );
        assert_eq!(
            spdx_ids("GPL-2.0 with Classpath-exception-2.0"),
            ["GPL-2.0", "Classpath-exception-2.0"]
        );
        assert!(spdx_ids("").is_empty());
    }

    #[test]
    fn expression_keeps_alternatives_and_ands_bundled_code() {
        assert_eq!(license_expression(&["Apache-2.0"], &[]), "Apache-2.0");
        assert_eq!(
            license_expression(&["Apache-2.0", "MIT", "Apache-2.0"], &[]),
            "Apache-2.0 OR MIT"
        );
        assert_eq!(
            license_expression(&["Apache-2.0"], &["BSD-3-Clause"]),
            "Apache-2.0 AND BSD-3-Clause"
        );
        assert_eq!(
            license_expression(&["Apache-2.0", "MIT"], &["BSD-3-Clause"]),
            "(Apache-2.0 OR MIT) AND BSD-3-Clause"
        );
    }

    #[test]
    fn bundled_licence_is_matched_by_url_prefix() {
        let dirs = Dirs::new();
        let hit = bundled_licence(
            &dirs,
            "https://chromium.googlesource.com/libyuv/libyuv/+/refs/heads/main/LICENSE",
        );
        assert_eq!(hit.map(|b| b.identifier), Some("BSD-3-Clause"));
        assert!(
            bundled_licence(&dirs, "https://www.apache.org/licenses/LICENSE-2.0.txt").is_none()
        );
    }

    #[test]
    fn pom_licence_names_are_classified() {
        assert_eq!(
            spdx_by_pom_name("The Apache Software License, Version 2.0"),
            Some("Apache-2.0")
        );
        assert_eq!(spdx_by_pom_name("  MIT License "), Some("MIT"));
        assert_eq!(
            spdx_by_pom_name("GNU Lesser General Public License"),
            Some("LGPL-2.1-or-later")
        );
        assert_eq!(spdx_by_pom_name("Eclipse Public License"), None);
    }

    #[test]
    fn tree_lines_become_name_version_pairs() {
        let tree = "\
or2-ffi v0.1.0 (/repo/core/or2-ffi)
serde v1.0.229
serde_derive v1.0.229 (proc-macro)
serde v1.0.229 (*)
libghostty-vt v0.1.1 (https://github.com/Uzaaft/libghostty-rs?rev=8953a7#8953a740)

stray
";
        assert_eq!(
            parse_tree(tree),
            [
                ("or2-ffi".to_string(), "0.1.0".to_string()),
                ("serde".to_string(), "1.0.229".to_string()),
                ("serde_derive".to_string(), "1.0.229".to_string()),
                ("libghostty-vt".to_string(), "0.1.1".to_string()),
            ]
        );
    }

    #[test]
    fn selection_skips_members_and_rejects_unknown_packages() {
        let meta = Metadata {
            packages: vec![
                package("m", "or2-ffi", "0.1.0"),
                package("b", "b", "2.0.0"),
                package("a", "a", "1.0.0"),
            ],
            workspace_members: vec!["m".into()],
        };
        let linked = vec![
            ("or2-ffi".to_string(), "0.1.0".to_string()),
            ("b".to_string(), "2.0.0".to_string()),
            ("a".to_string(), "1.0.0".to_string()),
        ];
        let names: Vec<_> = select_packages(&meta, &linked)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["a", "b"]);
        let unknown = vec![("zzz".to_string(), "1.0.0".to_string())];
        assert!(
            select_packages(&meta, &unknown)
                .unwrap_err()
                .contains("zzz")
        );
    }

    #[test]
    fn the_xtask_crate_is_a_member_and_never_listed() {
        // `xtask` is a workspace member, so even a tree that named it would not ship it.
        let meta = Metadata {
            packages: vec![package("x", "xtask", "0.1.0")],
            workspace_members: vec!["x".into()],
        };
        let linked = vec![("xtask".to_string(), "0.1.0".to_string())];
        assert!(select_packages(&meta, &linked).unwrap().is_empty());
    }

    #[test]
    fn ghostty_commit_is_read_from_the_build_script() {
        let commit = "22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018";
        let source = format!("const X: u8 = 1;\nconst GHOSTTY_COMMIT: &str = \"{commit}\";\n");
        assert_eq!(parse_ghostty_commit(&source), Some(commit));
        assert_eq!(
            parse_ghostty_commit("const GHOSTTY_COMMIT: &str = \"short\";"),
            None
        );
        assert_eq!(parse_ghostty_commit("nothing here"), None);
    }

    #[test]
    fn lockfile_lines_are_filtered_by_configuration() {
        let lockfile = "\
# This is a Gradle generated file for dependency locking.
androidx.core:core:1.13.0=releaseRuntimeClasspath,debugRuntimeClasspath
com.google.zxing:core:3.5.3=releaseRuntimeClasspath
org.junit:junit:4.13=debugUnitTestRuntimeClasspath
empty=androidApis
";
        assert_eq!(
            lockfile_artifacts(lockfile, "releaseRuntimeClasspath"),
            [
                (
                    "androidx.core".to_string(),
                    "core".to_string(),
                    "1.13.0".to_string()
                ),
                (
                    "com.google.zxing".to_string(),
                    "core".to_string(),
                    "3.5.3".to_string()
                ),
            ]
        );
    }

    #[test]
    fn texts_are_deduplicated_by_normalised_content() {
        let mut texts = Texts::default();
        let a = texts.add("terms\r\n\r\n");
        let b = texts.add("\nterms\n");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
        assert_eq!(texts.by_id.len(), 1);
        assert_eq!(texts.by_id[&a], "terms\n");
    }

    #[test]
    fn document_layout_is_one_package_per_line() {
        let mut texts = Texts::default();
        let id = texts.add("MIT text");
        let entry = Entry {
            name: "a".into(),
            title: None,
            version: "1".into(),
            license: "MIT".into(),
            repository: "https://example.org".into(),
            source: "crates.io",
            texts: vec![TextRef {
                id: id.clone(),
                file: "LICENSE".into(),
            }],
            fallback: true,
            note: Some("n".into()),
        };
        let document = dump_document(
            vec![("schema", json!(1)), ("group", json!("rust"))],
            &[entry],
            &texts,
        );
        assert_eq!(
            document,
            format!(
                "{{\n\"schema\": 1,\n\"group\": \"rust\",\n\"packages\": [\n\
                 {{\"fallback\": true, \"license\": \"MIT\", \"name\": \"a\", \"note\": \"n\", \"repository\": \"https://example.org\", \"source\": \"crates.io\", \"texts\": [{{\"file\": \"LICENSE\", \"id\": \"{id}\"}}], \"version\": \"1\"}}\n\
                 ],\n\"texts\": {{\n\"{id}\": \"MIT text\\n\"\n}}\n}}\n"
            )
        );
    }
}
