#!/usr/bin/env bash
# Repository policy checks that CI runs on every change:
#  1. every workflow and action pins third-party actions by full commit SHA;
#  2. docs, specs and code only link to hosts on the allowlist below, so
#     nothing references other projects by URL.
. "$(dirname "$0")/lib.sh"
cd "$ROOT"
fail=0

# 1. Pinned actions: `uses: owner/name@<40 hex>` (local `./` actions are fine).
if [ -d .github ]; then
  while IFS= read -r line; do
    echo "action not pinned by full SHA: $line" >&2
    fail=1
  done < <(grep -rnE '^\s*-?\s*uses:' .github \
    | grep -vE 'uses:\s*\./' \
    | grep -vE 'uses:\s*[A-Za-z0-9._/-]+@[0-9a-f]{40}(\s|$)' || true)
fi

# 2. Link hosts. Add a host here only when the repo depends on that service.
#    Documentation address ranges and example names are fine in examples.
ALLOWED_HOSTS='^(github\.com/backend-bunny/hickory-filter|ghcr\.io/backend-bunny(/.*)?|(.+\.)?example(\.[a-z]+)?|localhost|127\.0\.0\.1|(192\.0\.2|198\.51\.100|203\.0\.113)\.[0-9]+)$'
while IFS=: read -r file lineno url; do
  host_path="${url#*://}"
  host_path="${host_path%%[\"\'\`) >,;]*}"
  host="${host_path%%/*}"
  [ -n "$host" ] || continue   # `https:///x` links nowhere
  if ! printf '%s\n' "$host_path" | grep -Eq "$ALLOWED_HOSTS" \
     && ! printf '%s\n' "$host" | grep -Eq "$ALLOWED_HOSTS"; then
    echo "link to a host not on the allowlist: $file:$lineno $url" >&2
    fail=1
  fi
done < <(grep -rnoE 'https?://[^[:space:]"'"'"'`)>]+' \
  README.md AGENTS.md Taskfile.yml Cargo.toml openspec src tests hack 2>/dev/null \
  | grep -v '^openspec/changes/archive/' || true)

exit "$fail"
