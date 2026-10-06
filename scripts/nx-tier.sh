#!/usr/bin/env bash
# Run Nx targets at a gate tier — the one place the justfile's gate recipes
# (check/test/lint/format-check/coverage) turn a tier into an Nx command.
#
#   scripts/nx-tier.sh affected <target>...   projects the change can reach
#   scripts/nx-tier.sh all <target>...        one full sweep over every project
#
# The affected tier keys off an EXPLICIT base, never Nx's implicit default:
#   * NX_BASE when set (CI exports the merge base it derived). Only a plain ref
#     name or a commit SHA is accepted — letters, digits and `. _ / -`, not
#     starting with `-` and without `..` — and it must resolve to a commit; any
#     other value is refused here, before a single target runs.
#   * otherwise `git merge-base origin/master HEAD`, the fork point from the
#     default branch (fetch origin/master first in a clone that lacks it).
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "nx-tier: $*" >&2
  exit 1
}

[ $# -ge 2 ] || fail "usage: scripts/nx-tier.sh affected|all <target>..."
readonly TIER="$1"
shift

case "$TIER" in
  all)
    echo "nx-tier: full sweep (run-many) -t $*" >&2
    exec bash scripts/nx.sh run-many --all -t "$@"
    ;;
  affected) ;;
  *) fail "unknown tier '$TIER' — use 'affected' (the default) or 'all'." ;;
esac

if [ -n "${NX_BASE+set}" ]; then
  case "$NX_BASE" in
    "" | -* | *..*) fail "NX_BASE must be a plain git ref name or commit SHA (got '$NX_BASE')." ;;
  esac
  printf '%s' "$NX_BASE" | grep -Eq '^[A-Za-z0-9._/-]+$' \
    || fail "NX_BASE must be a plain git ref name or commit SHA — letters, digits and . _ / - only (got '$NX_BASE')."
  git rev-parse --verify --quiet "$NX_BASE^{commit}" >/dev/null \
    || fail "NX_BASE '$NX_BASE' does not resolve to a commit in this clone; fetch it or unset NX_BASE."
  base="$NX_BASE"
  source="NX_BASE"
else
  git rev-parse --verify --quiet "origin/master^{commit}" >/dev/null \
    || fail "no origin/master in this clone to derive the merge base from; run 'git fetch origin master' or set NX_BASE."
  base="$(git merge-base origin/master HEAD)" \
    || fail "HEAD shares no history with origin/master; set NX_BASE to the commit to compare against."
  source="merge-base with origin/master"
fi

echo "nx-tier: affected since $base ($source) -t $*" >&2
exec bash scripts/nx.sh affected --base="$base" -t "$@"
