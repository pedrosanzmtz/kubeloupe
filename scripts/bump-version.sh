#!/usr/bin/env bash
# Set the kubeloupe version in every place it lives:
#   Cargo.toml, desktop/Cargo.toml, desktop/tauri.conf.json and Cargo.lock.
#
# Usage: scripts/bump-version.sh <version>     e.g. 0.2.0 or 0.2.0-1
#
# Pre-release versions must be numeric (0.2.0-1, not 0.2.0-rc.1): the Windows
# MSI bundler (WiX) only accepts a numeric pre-release no greater than 65535.
set -euo pipefail

usage() { echo "usage: $0 <version>   (e.g. 0.2.0 or 0.2.0-1)" >&2; exit 2; }
[ $# -eq 1 ] || usage
version="${1#v}"

if ! [[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9]+))?$ ]]; then
  echo "error: '$1' is not MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-N" >&2
  echo "       (the MSI bundler rejects non-numeric pre-releases like -rc.1)" >&2
  exit 1
fi
if [ -n "${BASH_REMATCH[5]}" ] && [ "${BASH_REMATCH[5]}" -gt 65535 ]; then
  echo "error: the MSI bundler rejects a pre-release number above 65535" >&2
  exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# Replace the first top-level `version = "..."` (the [package] one).
set_cargo_toml() {
  local f="$1" tmp
  tmp="$(mktemp)"
  awk -v v="$version" '
    !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
    { print }
  ' "$f" >"$tmp"
  cat "$tmp" >"$f"
  rm -f "$tmp"
}

# Replace the version of the given packages in Cargo.lock.
set_cargo_lock() {
  local tmp
  tmp="$(mktemp)"
  awk -v v="$version" '
    /^name = "(kubeloupe|kubeloupe-desktop)"$/ { hit = 1; print; next }
    hit && /^version = / { print "version = \"" v "\""; hit = 0; next }
    { hit = 0; print }
  ' Cargo.lock >"$tmp"
  cat "$tmp" >Cargo.lock
  rm -f "$tmp"
}

set_cargo_toml Cargo.toml
set_cargo_toml desktop/Cargo.toml
set_cargo_lock

# Only the top-level "version" key in tauri.conf.json (2-space indent).
tmp="$(mktemp)"
awk -v v="$version" '
  !done && /^  "version":[[:space:]]*"/ { sub(/"version":[[:space:]]*"[^"]*"/, "\"version\": \"" v "\""); done = 1 }
  { print }
' desktop/tauri.conf.json >"$tmp"
cat "$tmp" >desktop/tauri.conf.json
rm -f "$tmp"

# Verify all four places now agree.
fail=0
check() { [ "$2" = "$version" ] || { echo "error: $1 is $2, expected $version" >&2; fail=1; }; }
check Cargo.toml "$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
check desktop/Cargo.toml "$(grep -m1 '^version' desktop/Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
check desktop/tauri.conf.json \
  "$(python3 -c "import json;print(json.load(open('desktop/tauri.conf.json'))['version'])")"
for p in kubeloupe kubeloupe-desktop; do
  check "Cargo.lock ($p)" \
    "$(grep -A1 "^name = \"$p\"$" Cargo.lock | sed -nE 's/^version = "(.*)"/\1/p')"
done
[ "$fail" -eq 0 ] || exit 1

echo "Version set to $version. Next:"
echo "  git commit -am \"Release v$version\" && git push"
echo "  then, once CI is green, the maintainer tags it: git tag v$version && git push origin v$version"
