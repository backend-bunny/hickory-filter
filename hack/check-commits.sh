#!/usr/bin/env bash
# Conventional Commits check: `type(scope)!: subject`. The subjects decide the
# next version and the changelog, so CI enforces them.
#
#   hack/check-commits.sh --title "<PR title>" [--range <base>..<head>]
#
# Merge commits are skipped. Exits non-zero and lists every bad subject.
. "$(dirname "$0")/lib.sh"

TYPES='feat|fix|perf|revert|build|deps|docs|ci|chore|test|refactor|style'
PATTERN="^(${TYPES})(\\([a-z0-9._/-]+\\))?!?: .+"

title=""
range=""
while [ $# -gt 0 ]; do
  case "$1" in
    --title) title="${2-}"; shift 2 ;;
    --range) range="${2-}"; shift 2 ;;
    *) die "unknown argument: $1" ;;
  esac
done
[ -n "$title" ] || [ -n "$range" ] || die "usage: check-commits.sh --title <title> [--range <base>..<head>]"

bad=0
check() {
  local what="$1" subject="$2"
  if ! printf '%s' "$subject" | grep -Eq "$PATTERN"; then
    echo "not a Conventional Commit ($what): $subject" >&2
    bad=1
  fi
}

[ -z "$title" ] || check "PR title" "$title"
if [ -n "$range" ]; then
  while IFS= read -r subject; do
    case "$subject" in "Merge "*) continue ;; esac
    check "commit" "$subject"
  done < <(git -C "$ROOT" log --format=%s "$range")
fi

if [ "$bad" -ne 0 ]; then
  echo "expected: type(scope)!: subject, with type one of: ${TYPES//|/, }" >&2
  exit 1
fi
