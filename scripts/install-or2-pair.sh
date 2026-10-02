#!/bin/sh
# Install or2-pair, the host side of or2's Easy pair, from a GitHub release of code-akram/or2.
#
#   curl -fsSL https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh | sh
#   sh install-or2-pair.sh [--version <v>] [--dir <directory>]
#
# It picks the binary for this OS and CPU (Linux: static x86_64 or aarch64; macOS: Intel or
# Apple silicon), downloads it with the release's SHA256SUMS, refuses it unless the checksum
# matches, and puts it in ~/.local/bin (or --dir, or $OR2_PAIR_INSTALL_DIR). It never uses sudo
# and changes nothing else: no shell startup file, no ~/.ssh.
#
# --version takes 0.2.0, v0.2.0 or or2-pair-v0.2.0; the default is the newest or2-pair release.
# OR2_PAIR_DOWNLOAD_BASE and OR2_PAIR_RELEASES_URL point it at a mirror (or a test fixture).

set -eu

repo="code-akram/or2"
download_base="${OR2_PAIR_DOWNLOAD_BASE:-https://github.com/$repo/releases/download}"
releases_url="${OR2_PAIR_RELEASES_URL:-https://api.github.com/repos/$repo/releases?per_page=100}"

say() { printf '%s\n' "$*"; }
die() {
	printf 'install-or2-pair: %s\n' "$*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
Usage: install-or2-pair.sh [--version <version>] [--dir <directory>]

Installs or2-pair for this host from https://github.com/code-akram/or2/releases.

  --version <v>    a release (0.2.0, v0.2.0 or or2-pair-v0.2.0); default: the newest
  --dir <dir>      where to put it; default: $OR2_PAIR_INSTALL_DIR, else ~/.local/bin
  -h, --help       this text
EOF
}

version=""
dir="${OR2_PAIR_INSTALL_DIR:-}"
while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || die "--version needs a value"
		version="$2"
		shift 2
		;;
	--version=*)
		version="${1#--version=}"
		shift
		;;
	--dir)
		[ $# -ge 2 ] || die "--dir needs a value"
		dir="$2"
		shift 2
		;;
	--dir=*)
		dir="${1#--dir=}"
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		die "unknown argument: $1"
		;;
	esac
done

# --- this host -----------------------------------------------------------------------------

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
aarch64 | arm64) arch="aarch64" ;;
*) die "unsupported CPU architecture: $arch (released: x86_64 and aarch64). Build it from source instead: cargo install --git https://github.com/$repo or2-pair --locked" ;;
esac
case "$os" in
Linux) target="$arch-unknown-linux-musl" ;;
Darwin)
	# An Intel shell under Rosetta on Apple silicon still gets the native binary.
	if [ "$arch" = "x86_64" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = "1" ]; then
		target="aarch64-apple-darwin"
	else
		target="$arch-apple-darwin"
	fi
	;;
*) die "unsupported operating system: $os (released: Linux and macOS). Build it from source instead: cargo install --git https://github.com/$repo or2-pair --locked" ;;
esac
asset="or2-pair-$target"

# --- downloads -----------------------------------------------------------------------------

if command -v curl >/dev/null 2>&1; then
	# HTTPS only (file: is for a local mirror the caller named), also after redirects.
	fetch() { curl -fsSL --proto '=https,file' --proto-redir '=https' -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
	fetch() { wget -q -O "$2" "$1"; }
else
	die "neither curl nor wget is installed; install one and run this again"
fi

if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
	die "neither sha256sum nor shasum is installed, so the download cannot be checked; nothing was installed"
fi

work="$(mktemp -d 2>/dev/null || mktemp -d -t or2-pair)"
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

case "$version" in
"") ;;
or2-pair-v*) tag="$version" ;;
v*) tag="or2-pair-$version" ;;
*) tag="or2-pair-v$version" ;;
esac
if [ -z "$version" ]; then
	fetch "$releases_url" "$work/releases.json" || die "could not list the releases at $releases_url"
	# The newest release first; the app's own releases have other tags.
	tag="$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\(or2-pair-v[^"]*\)".*/\1/p' "$work/releases.json" | head -n 1)"
	[ -n "$tag" ] || die "no or2-pair release found at $releases_url"
fi

say "Downloading $asset ($tag)"
fetch "$download_base/$tag/$asset" "$work/$asset" || die "could not download $download_base/$tag/$asset"
fetch "$download_base/$tag/SHA256SUMS" "$work/SHA256SUMS" || die "could not download $download_base/$tag/SHA256SUMS"

expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$work/SHA256SUMS" | tr 'A-F' 'a-f')"
[ -n "$expected" ] || die "SHA256SUMS of $tag lists no $asset; nothing was installed"
actual="$(sha256 "$work/$asset")"
if [ "$actual" != "$expected" ]; then
	die "checksum mismatch for $asset: expected $expected, got $actual. The download is corrupt or was tampered with; nothing was installed"
fi
say "Checksum OK ($actual)"

# --- install -------------------------------------------------------------------------------

if [ -z "$dir" ]; then
	[ -n "${HOME:-}" ] || die "HOME is not set; pass --dir"
	dir="$HOME/.local/bin"
fi
mkdir -p "$dir" || die "cannot create $dir; pass another --dir (this script never uses sudo)"
named="$dir"
dir="$(cd "$dir" && pwd -P)" || die "cannot use $dir"
[ -w "$dir" ] || die "$dir is not writable; pass another --dir (this script never uses sudo)"

chmod 755 "$work/$asset"
# A new file renamed over the old one: a running or2-pair keeps its binary.
cp "$work/$asset" "$dir/.or2-pair.new.$$" || die "cannot write to $dir"
chmod 755 "$dir/.or2-pair.new.$$"
mv -f "$dir/.or2-pair.new.$$" "$dir/or2-pair" || {
	rm -f "$dir/.or2-pair.new.$$"
	die "cannot install $dir/or2-pair"
}

installed="$("$dir/or2-pair" --version 2>/dev/null)" || die "$dir/or2-pair was installed but does not run here"
say "Installed $installed to $dir/or2-pair"

# sshd runs or2-pair by this path through the login shell, which or2-pair checks; a path
# needing quotes would be refused when pairing.
case "$dir" in
*[!A-Za-z0-9/._+-]*) say "Warning: $dir contains characters that or2-pair refuses (sshd's shell could misread them); install with --dir to a plain path such as ~/.local/bin." ;;
esac

case ":${PATH:-}:" in
*":$dir:"* | *":$named:"*) run="or2-pair" ;;
*)
	run="$dir/or2-pair"
	say ""
	say "$dir is not on your PATH. Add it to your shell's startup file, for example:"
	say "  export PATH=\"$dir:\$PATH\""
	;;
esac
say ""
say "Next: open or2 on your phone (Add host > Easy pair with QR) and run:"
say "  $run"
