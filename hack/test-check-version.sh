#!/usr/bin/env bash
# Tests for check-version.sh: it passes on the repository as committed and
# fails for each kind of mismatch. Runs against a temporary copy.
. "$(dirname "$0")/lib.sh"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
v="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -1)"
mkdir -p "$work/hack" "$work/charts/hickory-filter"
cp "$ROOT/hack/lib.sh" "$ROOT/hack/check-version.sh" "$work/hack/"
printf '#!/bin/sh\necho "hickory-filter %s"\n' "$v" > "$work/bin"; chmod +x "$work/bin"
reset() { cp "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" "$ROOT/melange.yaml" "$work/"; cp "$ROOT/charts/hickory-filter/Chart.yaml" "$work/charts/hickory-filter/"; printf '#!/bin/sh\necho "hickory-filter %s"\n' "$v" > "$work/bin"; }
run() { BIN="$work/bin" bash "$work/hack/check-version.sh" "$@" 2>&1; }
fails=0
expect_ok() { if run "${@:2}" >/dev/null; then echo "ok   $1"; else echo "FAIL $1 should pass" >&2; fails=1; fi; }
expect_fail() { # name, expected message, args...
  rc=0; out="$(run "${@:3}")" || rc=$?
  if [ "$rc" -ne 0 ] && grep -qF -e "$2" <<<"$out"; then echo "ok   $1"; else echo "FAIL $1 should fail with '$2': $out" >&2; fails=1; fi; }

reset; expect_ok "committed versions agree"
reset; expect_ok "matching tag" "v$v"
reset; expect_fail "tag differs from the files" "Cargo.toml is '0.1.0', expected '9.9.9'" "v9.9.9"
reset; expect_fail "malformed tag" "not vX.Y.Z" "1.2"
reset; sed -i "0,/^version = \"$v\"/s//version = \"9.9.9\"/" "$work/Cargo.toml"; expect_fail "Cargo.toml differs from the tag" "Cargo.toml is '9.9.9', expected '$v'" "v$v"
reset; sed -i "0,/^version = \"$v\"/s//version = \"9.9.9\"/" "$work/Cargo.lock"; expect_fail "Cargo.lock differs" "Cargo.lock is '9.9.9'"
reset; sed -i "s/^  version: $v/  version: 9.9.9/" "$work/melange.yaml"; expect_fail "melange differs" "melange.yaml is '9.9.9'"
reset; sed -i "s/^version: $v/version: 9.9.9/" "$work/charts/hickory-filter/Chart.yaml"; expect_fail "chart version differs" "Chart.yaml version is '9.9.9'"
reset; sed -i "s/^appVersion: \"$v\"/appVersion: \"9.9.9\"/" "$work/charts/hickory-filter/Chart.yaml"; expect_fail "chart appVersion differs" "Chart.yaml appVersion is '9.9.9'"
reset; printf '#!/bin/sh\necho "hickory-filter 9.9.9"\n' > "$work/bin"; expect_fail "binary version differs" "--version prints 'hickory-filter 9.9.9'"
exit $fails
