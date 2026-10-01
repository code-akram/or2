#!/usr/bin/env bash
# Regenerates the open-source licence data shipped in the app and the or2-pair CLI:
#   android/app/src/main/assets/licenses/rust.json     crates linked into libor2_ffi.so (+ Ghostty, Rust std)
#   android/app/src/main/assets/licenses/android.json  releaseRuntimeClasspath (strict Gradle lockfile + POMs)
#   android/app/src/main/assets/licenses/notices.md    copy of THIRD_PARTY_NOTICES.md
#   android/app/src/main/assets/licenses/COPYING       copy of LICENSE
#   core/or2-pair/THIRD_PARTY.md                       crates linked into or2-pair (when the package exists)
#
# Usage: scripts/gen-licenses.sh [--check]
#   --check  write nothing; fail if a generated file differs from a regeneration
#
# Needs python3, cargo (registry and git sources already fetched: it runs `cargo metadata --locked
# --offline`) and, for android.json, the Gradle cache of a build that resolved the locked
# dependencies. No network, no extra tools. Never edit the generated files.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
case ${1:-} in
  "" | --check) ;;
  *) sed -n '2,13p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
exec python3 "$root/scripts/gen_licenses.py" "$@"
