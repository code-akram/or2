#!/usr/bin/env bash
# Regenerates the herdr wire types:
#   core/or2-core/src/herdr/schema.json     normalized schema (see herdr_schema.py)
#   core/or2-core/src/herdr/generated.rs    cargo-typify output, one module per family
#
# Usage: scripts/gen-herdr-types.sh [--herdr PATH] [--offline] [--check]
#   --herdr PATH  the herdr binary whose `api schema --json` is the source (default: herdr)
#   --offline     regenerate generated.rs from the checked-in schema.json without running herdr
#   --check       write nothing; fail if the checked-in files differ from a regeneration
#
# Needs python3, rustfmt and cargo-typify (`cargo install cargo-typify --version
# 0.10.0-alpha.1 --locked`). Never edit generated.rs: change herdr_schema.py (or this script)
# and regenerate.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
dest=$root/core/or2-core/src/herdr
herdr=herdr
offline=0
check=0
while [ $# -gt 0 ]; do
  case $1 in
    --herdr) herdr=${2:?--herdr needs a path}; shift 2 ;;
    --offline) offline=1; shift ;;
    --check) check=1; shift ;;
    *) sed -n '2,12p' "${BASH_SOURCE[0]}"; exit 2 ;;
  esac
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/families"

if [ "$offline" = 1 ]; then
  cp "$dest/schema.json" "$work/schema.json"
else
  "$herdr" api schema --json >"$work/raw.json"
  version=$("$herdr" --version | awk 'NR == 1 { print $NF }')
  python3 "$root/scripts/herdr_schema.py" normalize "$work/raw.json" "$version" "$work/schema.json"
fi

read -r version protocol < <(python3 "$root/scripts/herdr_schema.py" split "$work/schema.json" "$work/families")

{
  cat <<EOF
//! **Generated. Do not edit.** herdr's socket API types for herdr $version (protocol $protocol),
//! produced by \`scripts/gen-herdr-types.sh\` from \`schema.json\` (the normalized output of
//! \`herdr api schema --json\`) with cargo-typify. To regenerate after a herdr update, run
//! \`scripts/gen-herdr-types.sh\`; fix problems in \`scripts/herdr_schema.py\`, never here.
//!
//! One module per schema family. Enums herdr sends carry a catch-all variant for values this
//! build does not know; unknown fields are ignored.

#![allow(clippy::all, dead_code, unused_imports)]

/// The herdr release this code was generated from.
pub const HERDR_VERSION: &str = "$version";
/// The herdr API protocol number this code was generated from.
pub const PROTOCOL: u32 = $protocol;
EOF
  for family in request success_response error_response; do
    cargo typify -B -o "$work/$family.rs" "$work/families/$family.json" >&2
    printf '\n/// Types generated from the `%s` schema.\npub mod %s {\n' "$family" "$family"
    grep -v '^#!\[' "$work/$family.rs"
    printf '}\n'
  done
} >"$work/generated.rs"
rustfmt --edition 2024 "$work/generated.rs"

if [ "$check" = 1 ]; then
  status=0
  if [ "$offline" = 0 ]; then
    diff -q "$work/schema.json" "$dest/schema.json" || status=1
  fi
  diff -q "$work/generated.rs" "$dest/generated.rs" || status=1
  exit $status
fi
[ "$offline" = 1 ] || cp "$work/schema.json" "$dest/schema.json"
cp "$work/generated.rs" "$dest/generated.rs"
echo "wrote $dest/schema.json and generated.rs (herdr $version, protocol $protocol)" >&2
