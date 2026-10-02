#!/bin/sh
# Install or2-pair, the host side of or2's Easy pair, from a GitHub release of code-akram/or2.
#
#   curl -fsSL https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh | sh
#   sh install-or2-pair.sh [--version vX.Y.Z] [--dir <directory>]
#
# It picks the binary for this OS and CPU (Linux: static x86_64 or aarch64; macOS: Intel or
# Apple silicon), downloads it with the release's SHA256SUMS, refuses it unless the checksum
# matches, runs it once (--version) and only then puts it in ~/.local/bin (or --dir, or
# $OR2_PAIR_INSTALL_DIR) in place of any older one. It never uses sudo and changes nothing else:
# no shell startup file, no ~/.ssh.
#
# The checksum proves the download is intact, not who made it: the binary and SHA256SUMS come
# from the same GitHub release. Signed releases are a future item.
#
# The default is the latest release (GitHub's "latest": no drafts, no prereleases), fetched
# from https://github.com/code-akram/or2/releases/latest/download/; --version vX.Y.Z fetches
# .../releases/download/vX.Y.Z/. No GitHub API is called. OR2_PAIR_RELEASES_BASE replaces
# https://github.com/code-akram/or2/releases for a mirror (or a test fixture) laid out the same
# way; it must be https:// (or file:// for a local copy).
#
# The whole script is one function, called on its last line: a download cut short runs nothing.

set -eu

say() { printf '%s\n' "$*"; }
die() {
	printf 'install-or2-pair: %s\n' "$*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
Usage: install-or2-pair.sh [--version <vX.Y.Z>] [--dir <directory>]

Installs or2-pair for this host from https://github.com/code-akram/or2/releases.

  --version <v>    a release (v0.1.0 or 0.1.0); default: the latest
  --dir <dir>      where to put it; default: $OR2_PAIR_INSTALL_DIR, else ~/.local/bin
  -h, --help       this text
EOF
}

# fetch URL FILE: 0 when downloaded, 2 when there is nothing at URL (HTTP 404, a missing file),
# 3 when curl refused a protocol (a redirect away from HTTPS), 1 otherwise.
fetch() {
	if [ "$downloader" = curl ]; then
		status=0
		code="$(curl -fsSL --proto '=https,file' --proto-redir '=https' -w '%{http_code}' -o "$2" "$1" 2>/dev/null)" || status=$?
		case "$status" in
		0) return 0 ;;
		1) return 3 ;;
		22) [ "$code" = 404 ] && return 2 ;;
		37) return 2 ;;
		esac
		return 1
	fi
	wget -q -O "$2" "$1" || return 1
}

# Whether the directory listing of $1 says group or others may write it, or it belongs to
# another user than $2.
unsafe_dir() {
	# shellcheck disable=SC2012 # only the mode and the owner's id are read
	set -- "$(ls -ldn -- "$1" 2>/dev/null | awk '{ print $1, $3 }')" "$2"
	case "$1" in
	?????w*' '* | ????????w*' '*) return 0 ;;
	*" $2") return 1 ;;
	esac
	return 0
}

main() {
	repo="code-akram/or2"
	base="${OR2_PAIR_RELEASES_BASE:-https://github.com/$repo/releases}"

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
			return 0
			;;
		*)
			usage >&2
			die "unknown argument: $1"
			;;
		esac
	done

	case "$version" in
	"") tag="" ;;
	*[!A-Za-z0-9.+-]*) die "not a version: $version (use the form v0.1.0)" ;;
	v[0-9]*) tag="$version" ;;
	[0-9]*) tag="v$version" ;;
	*) die "not a version: $version (use the form v0.1.0)" ;;
	esac

	case "$base" in
	https://* | file://*) ;;
	*) die "refusing $base: releases are fetched over https:// only (file:// for a local copy)" ;;
	esac

	# --- this host -------------------------------------------------------------------------

	os="$(uname -s)"
	arch="$(uname -m)"
	source_build="Build it from source instead: cargo install --git https://github.com/$repo or2-pair --locked"
	case "$arch" in
	x86_64 | amd64) arch="x86_64" ;;
	aarch64 | arm64) arch="aarch64" ;;
	*) die "unsupported CPU architecture: $arch (released: x86_64 and aarch64). $source_build" ;;
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
	*) die "unsupported operating system: $os (released: Linux and macOS). $source_build" ;;
	esac
	asset="or2-pair-$target"

	if command -v curl >/dev/null 2>&1; then
		downloader=curl
	elif command -v wget >/dev/null 2>&1; then
		# wget cannot be held to HTTPS on redirects; the checksum is checked all the same.
		case "$base" in
		file://*) die "a file:// release needs curl" ;;
		esac
		downloader=wget
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

	uid="$(id -u)"
	if [ "$uid" = 0 ]; then
		say "Note: running as root, so or2-pair is installed for root, and pairing with it from root's shell pairs the root account; run this as the user the phone should log in as."
	fi

	# --- where it goes ---------------------------------------------------------------------

	if [ -z "$dir" ]; then
		[ -n "${HOME:-}" ] || die "HOME is not set; pass --dir"
		dir="$HOME/.local/bin"
	fi
	mkdir -p "$dir" || die "cannot create $dir; pass another --dir (this script never uses sudo)"
	named="$dir"
	dir="$(cd "$dir" && pwd -P)" || die "cannot use $dir"
	[ -w "$dir" ] || die "$dir is not writable; pass another --dir (this script never uses sudo)"
	if [ "$uid" = 0 ]; then
		# root writes and later runs this file: nobody else may be able to change the directory.
		if unsafe_dir "$dir" 0; then
			die "refusing $dir: running as root, it must belong to root and not be writable by group or others; pass another --dir"
		fi
	else
		case "$(ls -ldn -- "$dir" 2>/dev/null)" in
		????????w*) die "refusing $dir: anyone can write it, so anyone could replace or2-pair there; pass another --dir" ;;
		esac
	fi
	[ ! -d "$dir/or2-pair" ] || die "$dir/or2-pair is a directory; move it away or pass another --dir"

	# --- download and check ----------------------------------------------------------------

	work=""
	stage=""
	trap '[ -z "$work" ] || rm -rf "$work"; [ -z "$stage" ] || rm -f "$stage"' EXIT
	trap 'exit 130' INT
	trap 'exit 143' TERM
	work="$(mktemp -d 2>/dev/null || mktemp -d -t or2-pair)"

	if [ -z "$tag" ]; then
		from="$base/latest/download"
		what="the latest release"
	else
		from="$base/download/$tag"
		what="$tag"
	fi
	say "Downloading $asset ($what)"
	for name in SHA256SUMS "$asset"; do
		status=0
		fetch "$from/$name" "$work/$name" || status=$?
		case "$status" in
		0) ;;
		2)
			if [ -n "$tag" ] && [ "$name" = SHA256SUMS ]; then
				die "version $tag does not exist (nothing at $from); the releases are listed at https://github.com/$repo/releases"
			elif [ -z "$tag" ] && [ "$name" = SHA256SUMS ]; then
				die "no release found at $base/latest; the releases are listed at https://github.com/$repo/releases"
			fi
			die "$what has no $asset (nothing at $from/$name)"
			;;
		3) die "refused $from/$name: it redirected away from HTTPS; nothing was installed" ;;
		*) die "could not download $from/$name; check the network and run this again" ;;
		esac
	done

	expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$work/SHA256SUMS" | tr 'A-F' 'a-f')"
	[ -n "$expected" ] || die "SHA256SUMS of $what lists no $asset; nothing was installed"
	actual="$(sha256 "$work/$asset")"
	if [ "$actual" != "$expected" ]; then
		die "checksum mismatch for $asset: expected $expected, got $actual. The download is corrupt or was tampered with (or a release was published between the two downloads: run this again); nothing was installed"
	fi
	say "Checksum OK ($actual)"

	# --- install ---------------------------------------------------------------------------

	# A new file of this run's own (mktemp creates it, refusing any name already there, a link
	# included), checked by running it, then renamed over the old one: a running or2-pair keeps
	# its binary, and a binary that does not run here replaces nothing.
	stage="$(mktemp "$dir/.or2-pair.XXXXXXXX")" || die "cannot write to $dir"
	cat "$work/$asset" >"$stage" || die "cannot write to $dir"
	chmod 755 "$stage"
	installed="$("$stage" --version 2>/dev/null)" || installed=""
	case "$installed" in
	"or2-pair "*) ;;
	*) die "the downloaded $asset does not run on this host; nothing was replaced" ;;
	esac
	if [ -n "$tag" ] && [ "$installed" != "or2-pair ${tag#v}" ]; then
		die "the downloaded $asset says it is \"$installed\", not or2-pair ${tag#v}; nothing was replaced"
	fi
	mv -f "$stage" "$dir/or2-pair" || die "cannot install $dir/or2-pair"
	stage=""
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
}

main "$@"
