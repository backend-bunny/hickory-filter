#!/usr/bin/env bash
# Shared helpers for hack/ scripts. Source it: `. "$(dirname "$0")/lib.sh"`.
set -euo pipefail

# Repository root, regardless of the caller's working directory.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT

die() { echo "error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing tool: $1 (run through the pinned toolchain: devbox shell)"; }
