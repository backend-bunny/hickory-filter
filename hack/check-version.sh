#!/usr/bin/env bash
# Fail unless every place that carries the release version agrees:
# Cargo.toml, Cargo.lock, melange.yaml, Chart.yaml (version and appVersion),
# `hickory-filter --version`, and the tag when one is given (vX.Y.Z).
#   hack/check-version.sh [vX.Y.Z]
# BIN=<path> uses that binary for --version; otherwise it is built with cargo.
. "$(dirname "$0")/lib.sh"
cd "$ROOT"
tag="${1:-}"
first() { sed -n "$1" "$2" | head -1; }
cargo_toml="$(first 's/^version = "\(.*\)"$/\1/p' Cargo.toml)"
cargo_lock="$(awk '/^name = "hickory-filter"$/ {getline; sub(/^version = "/, ""); sub(/"$/, ""); print; exit}' Cargo.lock)"
melange="$(first 's/^  version: \([^ ]*\).*/\1/p' melange.yaml)"
chart="$(first 's/^version: \([^ ]*\).*/\1/p' charts/hickory-filter/Chart.yaml)"
app="$(first 's/^appVersion: "\{0,1\}\([^" ]*\)"\{0,1\}.*/\1/p' charts/hickory-filter/Chart.yaml)"
if [ -n "${BIN:-}" ]; then out="$("$BIN" --version)"; else need cargo; out="$(cargo run --locked -q -- --version)"; fi
binary="${out#hickory-filter }"

fail=0
expected="${tag#v}"
[ -n "$tag" ] && [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { [ -z "$tag" ] || die "tag '$tag' is not vX.Y.Z"; }
[ -n "$expected" ] || expected="$cargo_toml"
report() { if [ "$2" = "$expected" ]; then echo "ok   $1 $2"; else echo "FAIL $1 is '$2', expected '$expected'" >&2; fail=1; fi; }
[ -z "$tag" ] || report "tag" "${tag#v}"
report "Cargo.toml" "$cargo_toml"
report "Cargo.lock" "$cargo_lock"
report "melange.yaml" "$melange"
report "Chart.yaml version" "$chart"
report "Chart.yaml appVersion" "$app"
[ "$out" = "hickory-filter $expected" ] && echo "ok   --version $out" || { echo "FAIL --version prints '$out', expected 'hickory-filter $expected'" >&2; fail=1; }
exit $fail
