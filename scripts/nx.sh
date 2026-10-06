#!/usr/bin/env bash
# Run Nx on the pinned toolchain. Every gate recipe in the justfile delegates
# through here rather than calling `nx` directly, so:
#
#   * the bun `.tool-versions` pins is first on PATH for Nx and for every target
#     it runs (scripts/bun.sh resolves it; `just bootstrap` installs it);
#   * a worktree that has never run `bun install` gets the locked install first,
#     instead of "nx: command not found";
#   * Nx runs without its background daemon, cloud, or interactive TUI — every
#     invocation is one foreground process that finishes, the same locally and
#     in CI.
#
# Usage: scripts/nx.sh <nx arguments...>
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

bun_path="$(bash scripts/bun.sh path)"
PATH="$(dirname "$bun_path"):$PATH"
export PATH
export NX_DAEMON=false NX_NO_CLOUD=true NX_TUI=false NX_SKIP_NX_CACHE_WARNING=true

if [ ! -x node_modules/.bin/nx ] || [ bun.lock -nt node_modules/.bun-install-stamp ]; then
  if ! out="$(bun install --frozen-lockfile 2>&1)"; then
    printf '%s\n' "$out" >&2
    echo "nx.sh: 'bun install --frozen-lockfile' failed (above); fix it, or re-run 'just bootstrap'." >&2
    exit 1
  fi
  touch node_modules/.bun-install-stamp
fi

if ! command -v node >/dev/null 2>&1; then
  echo "nx.sh: Nx runs on Node, which is not on PATH; install Node (LTS) and re-run." >&2
  exit 1
fi

exec node_modules/.bin/nx "$@"
