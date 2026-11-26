#!/usr/bin/env bash
# Build the APK with melange from a vendored copy of the committed files.
# Output: packages/ (the APK). A throwaway signing key is generated per build
# and discarded with the temp directory; the public half is kept in the temp
# directory only for later steps in this script.
. "$(dirname "$0")/lib.sh"
need melange; need apko; need cargo; need git; need python3; need curl
cd "$ROOT"
ARCH="${ARCH:-x86_64}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
src="$work/src"; mkdir -p "$src"

echo "== stage committed files"
git ls-files -z --cached --others --exclude-standard | grep -zv '^openspec/changes/archive/' \
  | while IFS= read -r -d '' f; do [ -e "$f" ] && echo "$f"; done \
  | tar -c -T - | tar -x -C "$src"

echo "== vendor crates (locked)"
( cd "$src" && mkdir -p .cargo && cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml )

echo "== third-party notices (from the vendored tree this build compiles)"
# musl's version comes from the Alpine repository the build environment uses.
repo="$(sed -n 's#^ *- \(https://[^ ]*/main\)$#\1#p' "$ROOT/melange.yaml" | head -1)"
musl_version="$(curl -sf "$repo/$ARCH/APKINDEX.tar.gz" | tar -xzO APKINDEX \
  | awk -v RS= '/\nP:musl\n/ { n = split($0, l, "\n"); for (i = 1; i <= n; i++) if (l[i] ~ /^V:/) print substr(l[i], 3) }')"
[ -n "$musl_version" ] || die "could not find the musl version in the build environment's repository"
python3 "$ROOT/hack/notices.py" generate --source "$src" --out-dir "$src" \
  --target "$ARCH-unknown-linux-musl" --musl-version "$musl_version"
mkdir -p "$ROOT/out"; cp "$src/notices.json" "$src/THIRD-PARTY-NOTICES.md" "$ROOT/out/"
# notices.json is the attestation predicate, not part of the image.
rm "$src/notices.json"

echo "== melange build"
melange keygen "$work/melange.rsa" >/dev/null
mkdir -p "$ROOT/packages" "$ROOT/.cache/melange"
melange build "$ROOT/melange.yaml" \
  --arch "$ARCH" \
  --source-dir "$src" \
  --signing-key "$work/melange.rsa" \
  --out-dir "$ROOT/packages" \
  --cache-dir "$ROOT/.cache/melange" \
  --workspace-dir "$work/workspace"
ls -l "$ROOT/packages/$ARCH/" | grep hickory-filter

echo "== melange test (Wolfi environment: version, static linking)"
melange test "$ROOT/melange.yaml" \
  --arch "$ARCH" \
  --repository-append "$ROOT/packages" \
  --keyring-append "$work/melange.rsa.pub" \
  --cache-dir "$ROOT/.cache/melange"

echo "== apko build (locked Wolfi set + the local APK)"
version="$(sed -n 's/^  version: \([^ ]*\).*/\1/p' "$ROOT/melange.yaml")"
python3 "$ROOT/hack/lockfile.py" merge apko.yaml apko.lock.json "packages/$ARCH/hickory-filter-${version}-r0.apk" "$work/apko.lock.json"
mkdir -p "$ROOT/out"
apko build apko.yaml "hickory-filter:$version" "$ROOT/out/image.tar" \
  --arch "$ARCH" \
  --lockfile "$work/apko.lock.json" \
  --keyring-append "$work/melange.rsa.pub" \
  --sbom-path "$ROOT/out" \
  --cache-dir "$ROOT/.cache/apko"
ls -l "$ROOT/out"

if [ -n "${KEEP_PUBLIC_KEY:-}" ]; then cp "$work/melange.rsa.pub" "$KEEP_PUBLIC_KEY"; fi
# The private key lives only in $work, which the trap removes.
