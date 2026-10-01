#!/usr/bin/env python3
"""Generates or2's open-source licence data. Run through scripts/gen-licenses.sh.

Outputs (all checked in; `--check` fails when one is stale):

  android/app/src/main/assets/licenses/rust.json     Rust crates linked into libor2_ffi.so for
                                                     aarch64-linux-android, plus the Zig-built
                                                     libghostty-vt and the Rust standard library
  android/app/src/main/assets/licenses/android.json  the release runtime classpath (Gradle)
  android/app/src/main/assets/licenses/notices.md    copy of THIRD_PARTY_NOTICES.md
  android/app/src/main/assets/licenses/COPYING       copy of LICENSE (GPL-3.0 text)
  core/or2-pair/THIRD_PARTY.md                       crates linked into the or2-pair CLI (only
                                                     when that package exists in the workspace)

Sources: `cargo metadata --locked --offline` (crate set, licence expressions, repositories) and
the licence/notice files inside each crate's source in the local cargo registry or git checkout;
`android/app/gradle.lockfile` (strict lock: every artifact of releaseRuntimeClasspath) and each
artifact's POM from the offline Gradle cache. A crate that ships no licence file falls back to the
SPDX standard text in scripts/licenses/spdx/ and is flagged `fallback`. Nothing is downloaded.
"""

import argparse
import hashlib
import json
import re
import subprocess
import sys
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORE = ROOT / "core"
ASSETS = ROOT / "android/app/src/main/assets/licenses"
SPDX_DIR = ROOT / "scripts/licenses/spdx"
GHOSTTY_DIR = ROOT / "scripts/licenses/ghostty"
LOCKFILE = ROOT / "android/app/gradle.lockfile"
ANDROID_TARGET = "aarch64-linux-android"
FFI_PACKAGE = "or2-ffi"
CLI_PACKAGE = "or2-pair"

# The Ghostty commit libghostty-vt-sys builds (its build.rs `GHOSTTY_COMMIT`). The texts under
# scripts/licenses/ghostty/ were read from that commit; if the pin moves the generator stops until
# the components below are re-read.
GHOSTTY_COMMIT = "22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018"
LICENSE_FILE_RE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright)([-_. ].*)?$", re.I)
MAX_TEXT_BYTES = 200_000


def die(message):
    print(f"gen-licenses: {message}", file=sys.stderr)
    sys.exit(1)


def run(cmd, cwd):
    result = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        die(f"{' '.join(cmd)} failed:\n{result.stderr.strip()}")
    return result.stdout


def normalise(text):
    return text.replace("\r\n", "\n").replace("\r", "\n").strip("\n") + "\n"


def read_text(path):
    data = path.read_bytes()
    if len(data) > MAX_TEXT_BYTES:
        die(f"{path} is {len(data)} bytes; refusing to embed a licence file that large")
    return normalise(data.decode("utf-8", errors="replace"))


class Texts:
    """Licence texts deduplicated by content; packages reference them by id."""

    def __init__(self):
        self.by_id = {}

    def add(self, text):
        text = normalise(text)
        text_id = hashlib.sha256(text.encode()).hexdigest()[:12]
        self.by_id[text_id] = text
        return text_id

    def dump(self):
        return {k: self.by_id[k] for k in sorted(self.by_id)}


def spdx_text(identifier):
    path = SPDX_DIR / f"{identifier}.txt"
    if not path.is_file():
        die(f"no standard text for SPDX id {identifier!r}: add scripts/licenses/spdx/{identifier}.txt")
    return read_text(path)


def spdx_ids(expression):
    tokens = re.split(r"\s+|[()]", expression or "")
    return [t for t in tokens if t and t.upper() not in ("OR", "AND", "WITH")]


# --- Rust ---------------------------------------------------------------------------------


def cargo_metadata():
    return json.loads(run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], CORE))


def linked_packages(meta, root_name, target):
    """The packages `cargo tree` says `root_name` links for `target` (normal edges only).

    Build-script and dev dependencies ship no code and are left out; proc-macro crates stay
    because their output is compiled into the binary. `cargo tree` resolves the features of that
    one package, which `cargo metadata` (workspace-wide features) over-reports. Workspace members
    are the project itself and are not listed. Returns None when the package does not exist.
    """
    members = set(meta["workspace_members"])
    if not any(p["name"] == root_name and p["id"] in members for p in meta["packages"]):
        return None
    tree = run(
        ["cargo", "tree", "--locked", "--offline", "-p", root_name, "--target", target, "-e", "normal", "--prefix", "none"],
        CORE,
    )
    linked = set()
    for line in tree.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1].startswith("v"):
            linked.add((fields[0], fields[1][1:]))
    by_key = {(p["name"], p["version"]): p for p in meta["packages"] if p["id"] not in members}
    missing = [k for k in linked if k not in by_key and not any(m["name"] == k[0] for m in meta["packages"] if m["id"] in members)]
    if missing:
        die(f"cargo tree names packages missing from cargo metadata: {missing}")
    return sorted((by_key[k] for k in linked if k in by_key), key=lambda p: (p["name"], p["version"]))


def source_kind(package):
    source = package.get("source") or ""
    if source.startswith("git+"):
        return "git"
    if "crates.io" in source:
        return "crates.io"
    die(f"{package['name']} {package['version']} has an unexpected source {source!r}")


def license_files(package):
    root = Path(package["manifest_path"]).parent
    candidates = [root]
    if source_kind(package) == "git":
        # Workspace members of a git checkout keep one LICENSE at the repository root.
        candidates += [p for p in root.parents if "checkouts" in p.parts][:3]
    for directory in candidates:
        found = sorted(
            f for f in directory.iterdir() if f.is_file() and LICENSE_FILE_RE.match(f.name)
        )
        if found:
            return found
    return []


def crate_entry(package, texts):
    entry = {
        "name": package["name"],
        "version": package["version"],
        "license": package.get("license") or package.get("license_file") or "",
        "repository": package.get("repository") or package.get("homepage") or "",
        "source": source_kind(package),
        "texts": [],
    }
    files = license_files(package)
    root = Path(package["manifest_path"]).parent
    for f in files:
        entry["texts"].append({"id": texts.add(read_text(f)), "file": f.name})
    if package.get("links") or package["name"].endswith("-sys"):
        # A crate that bundles C sources keeps the licences of the bundled code in subdirectories.
        for f in sorted(root.rglob("*")):
            relative = f.relative_to(root)
            if (
                f.is_file()
                and f not in files
                and len(relative.parts) <= 5
                and LICENSE_FILE_RE.match(f.name)
                and not {"tests", "test", "examples", "docs"} & set(relative.parts[:-1])
            ):
                text_id = texts.add(read_text(f))
                if all(text_id != t["id"] for t in entry["texts"]):
                    entry["texts"].append({"id": text_id, "file": str(relative)})
    if not files:
        ids = spdx_ids(entry["license"])
        if not ids:
            die(f"{package['name']} {package['version']} declares no licence and ships no licence file")
        for identifier in ids:
            entry["texts"].append({"id": texts.add(spdx_text(identifier)), "file": f"{identifier} (standard text)"})
        entry["fallback"] = True
        entry["note"] = "The crate ships no licence file; the SPDX standard text for its declared licence is shown."
    if not entry["license"]:
        die(f"{package['name']} {package['version']} declares no licence expression")
    return entry


def ghostty_pin(meta_packages):
    sys_pkg = next((p for p in meta_packages if p["name"] == "libghostty-vt-sys"), None)
    if sys_pkg is None:
        return None
    build_rs = Path(sys_pkg["manifest_path"]).parent / "build.rs"
    match = re.search(r'GHOSTTY_COMMIT: &str = "([0-9a-f]{40})"', build_rs.read_text())
    if not match:
        die(f"cannot read GHOSTTY_COMMIT from {build_rs}")
    if match.group(1) != GHOSTTY_COMMIT:
        die(
            f"libghostty-vt-sys now builds Ghostty {match.group(1)}, not {GHOSTTY_COMMIT}: re-read the "
            "Ghostty component licences, update scripts/licenses/ghostty/ and GHOSTTY_COMMIT in "
            "scripts/gen_licenses.py"
        )
    return match.group(1)


def zig_entries(texts):
    """Components compiled into libghostty-vt's static archive by `zig build` (not Cargo crates)."""

    def text(name):
        return {"id": texts.add(read_text(GHOSTTY_DIR / name[0])), "file": name[1]}

    def standard(identifier):
        return {"id": texts.add(spdx_text(identifier)), "file": f"{identifier} (standard text)"}

    ghostty = "https://github.com/ghostty-org/ghostty"
    return [
        {
            "name": "Ghostty (libghostty-vt)",
            "version": f"commit {GHOSTTY_COMMIT[:12]}",
            "license": "MIT",
            "repository": ghostty,
            "source": "zig",
            "texts": [text(("ghostty.txt", "LICENSE"))],
            "note": f"Built from Ghostty commit {GHOSTTY_COMMIT} by libghostty-rs with Zig 0.16.0 and statically linked.",
        },
        {
            "name": "Highway (in libghostty-vt)",
            "version": "1.2.0 (commit 66486a10623f)",
            "license": "Apache-2.0 OR BSD-3-Clause",
            "repository": "https://github.com/google/highway",
            "source": "zig",
            "texts": [text(("highway-apache.txt", "LICENSE")), text(("highway-bsd3.txt", "LICENSE-BSD3"))],
        },
        {
            "name": "simdutf (in libghostty-vt)",
            "version": "5.2.8",
            "license": "Apache-2.0 OR MIT",
            "repository": "https://github.com/simdutf/simdutf",
            "source": "zig",
            "texts": [standard("Apache-2.0"), text(("simdutf-mit.txt", "MIT (copyright line reconstructed)"))],
            "fallback": True,
            "note": "Vendored in Ghostty's pkg/simdutf as a single header; its licence files are not in the tree, so the standard texts are shown.",
        },
        {
            "name": "uucode (in libghostty-vt)",
            "version": "0.2.0 (commit 2826a37a4562)",
            "license": "MIT",
            "repository": "https://github.com/jacobsandlund/uucode",
            "source": "zig",
            "texts": [text(("uucode.txt", "LICENSE.md")), standard("Unicode-3.0")],
            "note": "Unicode tables generated from the Unicode Character Database, under the Unicode License v3.",
        },
        {
            "name": "Bjoern Hoehrmann's UTF-8 decoder (in libghostty-vt)",
            "version": "n/a",
            "license": "MIT",
            "repository": "http://bjoern.hoehrmann.de/utf-8/decoder/dfa",
            "source": "zig",
            "texts": [text(("hoehrmann.txt", "MIT (reconstructed)"))],
            "fallback": True,
            "note": "Ghostty's UTF8Decoder.zig is based on this decoder; the licence text is the MIT standard text with the author's copyright line.",
        },
        {
            "name": "Zig compiler runtime (in libghostty-vt)",
            "version": "0.16.0",
            "license": "MIT",
            "repository": "https://github.com/ziglang/zig",
            "source": "zig",
            "texts": [text(("zig.txt", "LICENSE"))],
            "note": "compiler_rt and standard-library code that zig build links into the static archive.",
        },
    ]


def rust_std_entry(texts):
    mit = spdx_text("MIT").replace("<year> <copyright holders>", "The Rust Project Contributors")
    return {
        "name": "Rust standard library",
        "version": "core, alloc, std, compiler-builtins, libunwind",
        "license": "MIT OR Apache-2.0",
        "repository": "https://github.com/rust-lang/rust",
        "source": "rustup",
        "texts": [
            {"id": texts.add(spdx_text("Apache-2.0")), "file": "LICENSE-APACHE (standard text)"},
            {"id": texts.add(mit), "file": "LICENSE-MIT (standard text)"},
        ],
        "fallback": True,
        "note": "Statically linked into every Rust binary; the toolchain's COPYRIGHT file lists its vendored parts.",
    }


def standard_map(texts):
    """SPDX id to text id, for the in-app view of vendored notices."""
    return {i: texts.add(spdx_text(i)) for i in ("Apache-2.0", "MIT")}


def linked_entries(packages, texts):
    """Entries for the crates, the Zig-built libghostty-vt components and the Rust standard library."""
    entries = [crate_entry(p, texts) for p in packages]
    if any(p["name"] == "libghostty-vt-sys" for p in packages):
        ghostty_pin(packages)
        entries += zig_entries(texts)
    entries.append(rust_std_entry(texts))
    entries.sort(key=lambda e: (e["name"].lower(), e["version"]))
    return entries


def rust_document(packages, target):
    texts = Texts()
    entries = linked_entries(packages, texts)
    return {
        "schema": 1,
        "group": "rust",
        "target": target,
        "generator": "scripts/gen-licenses.sh",
        "standard": standard_map(texts),
        "packages": entries,
        "texts": texts.dump(),
    }


# --- Android ------------------------------------------------------------------------------

SPDX_BY_POM_NAME = {
    "apache-2.0": "Apache-2.0",
    "apache 2.0": "Apache-2.0",
    "apache license 2.0": "Apache-2.0",
    "apache license, version 2.0": "Apache-2.0",
    "apache license v2.0": "Apache-2.0",
    "the apache license, version 2.0": "Apache-2.0",
    "the apache software license, version 2.0": "Apache-2.0",
    "mit": "MIT",
    "mit license": "MIT",
    "the mit license": "MIT",
    "lgpl, version 2.1": "LGPL-2.1-or-later",
    "lgpl-2.1-or-later": "LGPL-2.1-or-later",
    "gnu lesser general public license": "LGPL-2.1-or-later",
}


def gradle_home():
    import os

    return Path(os.environ.get("GRADLE_USER_HOME", Path.home() / ".gradle"))


def lockfile_artifacts(configuration):
    found = []
    for line in LOCKFILE.read_text().splitlines():
        if not line or line.startswith("#") or line.startswith("empty="):
            continue
        coordinate, _, configurations = line.partition("=")
        if configuration in configurations.split(","):
            group, artifact, version = coordinate.split(":")
            found.append((group, artifact, version))
    return sorted(found)


def cache_dir(group, artifact, version):
    return gradle_home() / "caches/modules-2/files-2.1" / group / artifact / version


def find_in_cache(group, artifact, version, suffixes):
    directory = cache_dir(group, artifact, version)
    for suffix in suffixes:
        hits = sorted(directory.glob(f"*/{artifact}-{version}{suffix}"))
        if hits:
            return hits[0]
    return None


def pom_field(root, tag):
    ns = root.tag[: root.tag.index("}") + 1] if root.tag.startswith("{") else ""
    return ns, root.find(f"{ns}{tag}")


def read_pom(group, artifact, version, depth=0):
    """(name, url, licences, packaging) of a POM, inheriting licences from its parent chain."""
    path = find_in_cache(group, artifact, version, [".pom"])
    if path is None:
        die(f"no POM for {group}:{artifact}:{version} in the Gradle cache ({cache_dir(group, artifact, version)})")
    root = ET.parse(path).getroot()
    ns = root.tag[: root.tag.index("}") + 1] if root.tag.startswith("{") else ""

    def text(tag):
        node = root.find(f"{ns}{tag}")
        return node.text.strip() if node is not None and node.text else ""

    licences = [
        {"name": (lic.findtext(f"{ns}name") or "").strip(), "url": (lic.findtext(f"{ns}url") or "").strip()}
        for lic in root.findall(f"{ns}licenses/{ns}license")
    ]
    parent = root.find(f"{ns}parent")
    if not licences and parent is not None and depth < 5:
        parent_version = (parent.findtext(f"{ns}version") or "").strip()
        _, _, parent_licences, _ = read_pom(
            (parent.findtext(f"{ns}groupId") or "").strip(),
            (parent.findtext(f"{ns}artifactId") or "").strip(),
            parent_version,
            depth + 1,
        )
        licences = parent_licences
    return text("name"), text("url"), licences, text("packaging") or "jar"


def archive_notices(group, artifact, version):
    """LICENSE/NOTICE files at the root (or META-INF) of the artifact's own jar/aar."""
    path = find_in_cache(group, artifact, version, [".aar", ".jar"])
    found = []
    if path is None:
        return found
    with zipfile.ZipFile(path) as archive:
        for name in sorted(archive.namelist()):
            base = name.rsplit("/", 1)[-1]
            parent = name.rsplit("/", 1)[0] if "/" in name else ""
            if parent in ("", "META-INF") and LICENSE_FILE_RE.match(base) and not name.endswith("/"):
                if archive.getinfo(name).file_size <= MAX_TEXT_BYTES:
                    found.append((name, normalise(archive.read(name).decode("utf-8", errors="replace"))))
    return found


def android_document():
    texts = Texts()
    entries = []
    for group, artifact, version in lockfile_artifacts("releaseRuntimeClasspath"):
        name, url, licences, packaging = read_pom(group, artifact, version)
        if packaging == "pom":
            continue  # a BOM or aggregator: no code ships
        if not licences:
            die(f"{group}:{artifact}:{version} declares no licence in its POM or its parents")
        resolved, entry_texts = [], []
        for lic in licences:
            key = lic["name"].strip().lower()
            if key not in SPDX_BY_POM_NAME:
                die(f"{group}:{artifact}:{version}: unknown POM licence {lic['name']!r} ({lic['url']}); classify it in SPDX_BY_POM_NAME")
            identifier = SPDX_BY_POM_NAME[key]
            resolved.append(identifier)
            entry_texts.append({"id": texts.add(spdx_text(identifier)), "file": f"{identifier} (standard text)"})
        # The library's own notices, when it ships any, come after the standard texts.
        for member, body in archive_notices(group, artifact, version):
            entry_texts.append({"id": texts.add(body), "file": member})
        # Dual-licensed artifacts list every licence; the expression keeps them as alternatives.
        unique = sorted(set(resolved), key=resolved.index)
        entry = {
            "name": f"{group}:{artifact}",
            "title": name if name and "${" not in name else artifact,
            "version": version,
            "license": " OR ".join(unique),
            "repository": url,
            "source": "maven",
            "texts": [],
        }
        seen = set()
        for t in entry_texts:
            if t["id"] not in seen:
                seen.add(t["id"])
                entry["texts"].append(t)
        entries.append(entry)
    return {
        "schema": 1,
        "group": "android",
        "configuration": "releaseRuntimeClasspath",
        "generator": "scripts/gen-licenses.sh",
        "standard": standard_map(texts),
        "packages": entries,
        "texts": texts.dump(),
    }


# --- output -------------------------------------------------------------------------------


def dump_json(document):
    head = {k: v for k, v in document.items() if k not in ("packages", "texts")}
    lines = ["{"]
    for key, value in head.items():
        lines.append(f"{json.dumps(key)}: {json.dumps(value, ensure_ascii=False, sort_keys=True)},")
    lines.append('"packages": [')
    lines.append(",\n".join(json.dumps(p, ensure_ascii=False, sort_keys=True) for p in document["packages"]))
    lines.append("],")
    lines.append('"texts": {')
    lines.append(",\n".join(f"{json.dumps(k)}: {json.dumps(v, ensure_ascii=False)}" for k, v in document["texts"].items()))
    lines.append("}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def third_party_markdown(packages, package_name):
    texts = Texts()
    entries = linked_entries(packages, texts)
    used_by = {}
    for e in entries:
        for t in e["texts"]:
            used_by.setdefault(t["id"], []).append(f"{e['name']} {e['version']} ({t['file']})")
    order = sorted(used_by, key=lambda i: used_by[i][0].lower())
    numbers = {text_id: n for n, text_id in enumerate(order, 1)}
    out = [
        f"# Third-party software in {package_name}",
        "",
        "Generated by `scripts/gen-licenses.sh`; do not edit. The crates and components linked into the",
        f"`{package_name}` binary and the licence texts they ship; the numbers refer to the texts below. A",
        "`standard` text is the SPDX text for a declared licence the package does not ship itself. or2's",
        "own code is GPL-3.0-or-later (see `LICENSE`).",
        "",
        "| Name | Version | Licence | Source | Texts |",
        "|---|---|---|---|---|",
    ]
    for e in entries:
        refs = ", ".join(str(numbers[t["id"]]) for t in e["texts"])
        out.append(f"| {e['name']} | {e['version']} | {e['license']} | {e['repository'] or e['source']} | {refs} |")
    out += ["", "## Licence texts", ""]
    for text_id in order:
        out += [f"### {numbers[text_id]}", "", "Used by: " + "; ".join(used_by[text_id]), ""]
        out += ["```text", texts.by_id[text_id].rstrip("\n"), "```", ""]
    return "\n".join(out).rstrip("\n") + "\n"


def outputs(args):
    result = {}
    meta = cargo_metadata()
    ffi = linked_packages(meta, FFI_PACKAGE, ANDROID_TARGET)
    if ffi is None:
        die(f"workspace has no {FFI_PACKAGE} package")
    result[ASSETS / "rust.json"] = dump_json(rust_document(ffi, ANDROID_TARGET))
    result[ASSETS / "android.json"] = dump_json(android_document())
    result[ASSETS / "notices.md"] = (ROOT / "THIRD_PARTY_NOTICES.md").read_text()
    result[ASSETS / "COPYING"] = (ROOT / "LICENSE").read_text()
    cli = linked_packages(meta, args.cli_package, "all")
    if cli is not None:
        destination = Path(args.cli_out) if args.cli_out else CORE / args.cli_package / "THIRD_PARTY.md"
        result[destination] = third_party_markdown(cli, args.cli_package)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="write nothing; fail if a generated file is stale")
    parser.add_argument("--cli-package", default=CLI_PACKAGE, help=argparse.SUPPRESS)
    parser.add_argument("--cli-out", default=None, help=argparse.SUPPRESS)
    args = parser.parse_args()

    generated = outputs(args)
    stale = []
    for path, content in generated.items():
        current = path.read_text() if path.is_file() else None
        if current != content:
            stale.append(path)
            if not args.check:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)
    rel = lambda p: p.relative_to(ROOT) if ROOT in p.parents else p
    if args.check:
        if stale:
            die("stale: " + ", ".join(str(rel(p)) for p in stale) + " (run scripts/gen-licenses.sh)")
        print(f"gen-licenses: {len(generated)} files up to date")
    else:
        print(f"gen-licenses: wrote {len(stale)} of {len(generated)} files")


if __name__ == "__main__":
    main()
