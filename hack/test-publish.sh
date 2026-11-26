#!/usr/bin/env bash
# The publish dry run lists exactly the two release tags and no floating one.
. "$(dirname "$0")/lib.sh"
v="$(sed -n 's/^  version: \([^ ]*\).*/\1/p' "$ROOT/melange.yaml" | head -1)"
out="$(DRY_RUN=1 GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 IMAGE=registry.example/hickory-filter "$ROOT/hack/publish.sh")"
want="registry.example/hickory-filter:$v
registry.example/hickory-filter:sha-0123456"
[ "$out" = "$want" ] || { echo "FAIL dry run printed:" >&2; echo "$out" >&2; exit 1; }
! grep -q latest <<<"$out" || { echo "FAIL a latest tag" >&2; exit 1; }
echo "ok   dry run lists exactly the version and sha tags"
