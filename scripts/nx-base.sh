#!/usr/bin/env bash
# Print the base commit the AFFECTED gate tier keys off — always explicit, never
# Nx's implicit default. The justfile's gate recipes call this and hand the
# result to `nx affected --base=...`.
#
#   * NX_BASE when set (CI exports the base scripts/ci-gate-tier.mjs derived).
#     Only a plain ref name or a commit SHA is accepted — letters, digits and
#     `. _ / -`, not starting with `-` and without `..` — and it must resolve to
#     a commit; any other value is refused here, before a single target runs.
#   * otherwise `git merge-base origin/master HEAD`, the fork point from the
#     default branch (fetch origin/master first in a clone that lacks it).
#
# Prints the base on stdout and one line saying where it came from on stderr.
set -euo pipefail

fail() {
  echo "nx-base: $*" >&2
  exit 1
}

if [ -n "${NX_BASE+set}" ]; then
  case "$NX_BASE" in
    "" | -* | *..*) fail "NX_BASE must be a plain git ref name or commit SHA (got '$NX_BASE')." ;;
  esac
  printf '%s' "$NX_BASE" | grep -Eq '^[A-Za-z0-9._/-]+$' \
    || fail "NX_BASE must be a plain git ref name or commit SHA — letters, digits and . _ / - only (got '$NX_BASE')."
  git rev-parse --verify --quiet "$NX_BASE^{commit}" >/dev/null \
    || fail "NX_BASE '$NX_BASE' does not resolve to a commit in this clone; fetch it or unset NX_BASE."
  echo "nx-base: affected since $NX_BASE (NX_BASE)" >&2
  printf '%s\n' "$NX_BASE"
else
  git rev-parse --verify --quiet "origin/master^{commit}" >/dev/null \
    || fail "no origin/master in this clone to derive the merge base from; run 'git fetch origin master' or set NX_BASE."
  base="$(git merge-base origin/master HEAD)" \
    || fail "HEAD shares no history with origin/master; set NX_BASE to the commit to compare against."
  echo "nx-base: affected since $base (merge-base with origin/master)" >&2
  printf '%s\n' "$base"
fi
