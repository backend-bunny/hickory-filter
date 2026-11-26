#!/usr/bin/env bash
# Regenerate apko.lock.json: the Wolfi packages only (the locally built
# hickory-filter APK is added at build time by hack/lockfile.py merge).
# Commit the result through review.
. "$(dirname "$0")/lib.sh"
need apko; need python3
cd "$ROOT"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
python3 hack/lockfile.py strip apko.yaml > "$tmp/apko.yaml"
( cd "$tmp" && apko lock apko.yaml --output "$ROOT/apko.lock.json" )
python3 hack/lockfile.py check apko.yaml apko.lock.json
echo "wrote apko.lock.json"
