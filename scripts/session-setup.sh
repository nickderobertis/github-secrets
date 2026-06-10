#!/usr/bin/env bash
#
# Provision a dev environment that can run the full `just check` gate —
# i.e. everything except the opt-in live e2e suite (`just test-live`), which
# needs a real GitHub token and is intentionally left out.
#
# The base image already ships the Rust toolchain plus the rustfmt/clippy
# components (see rust-toolchain.toml). What it lacks, and what this script
# installs, is the rest of the command surface AGENTS.md relies on:
#
#   * just            — the task runner every recipe is invoked through.
#   * cargo-nextest   — the test runner used by `just test` / `just test-e2e`.
#
# Both are fetched as prebuilt binaries into the cargo bin dir (already on
# PATH), so no compile cost. The script is idempotent: anything already
# present is left untouched, so it is safe to re-run on every session start.
#
# Wired into `.claude/settings.json` as a SessionStart hook. Run it by hand
# any time with `./scripts/session-setup.sh`.
set -euo pipefail

# Keep in sync with .tool-versions.
JUST_VERSION="1.51.0"

CARGO_BIN="${CARGO_HOME:-$HOME/.cargo}/bin"

log() { printf 'session-setup: %s\n' "$*" >&2; }

# rustfmt + clippy come from rust-toolchain.toml; add them defensively in case
# the base image ever drops them. Never fatal — the gate will surface a real
# absence far more clearly than this line would.
ensure_components() {
  if command -v rustup >/dev/null 2>&1; then
    rustup component add rustfmt clippy >/dev/null 2>&1 || true
  fi
}

ensure_just() {
  if command -v just >/dev/null 2>&1; then
    return 0
  fi
  log "installing just ${JUST_VERSION}"
  curl --proto '=https' --tlsv1.2 -sSfL https://just.systems/install.sh \
    | bash -s -- --tag "${JUST_VERSION}" --to "${CARGO_BIN}" >&2
}

ensure_nextest() {
  if command -v cargo-nextest >/dev/null 2>&1; then
    return 0
  fi
  log "installing cargo-nextest"
  curl --proto '=https' --tlsv1.2 -sSfL https://get.nexte.st/latest/linux \
    | tar zxf - -C "${CARGO_BIN}"
}

main() {
  mkdir -p "${CARGO_BIN}"
  ensure_components
  ensure_just
  ensure_nextest
  # Warm the crate cache so the first `just check` of the session is not the
  # one that pays the download cost. Non-fatal: the build would fetch anyway.
  cargo fetch --locked >/dev/null 2>&1 || true
  log "ready (just $(just --version 2>/dev/null | awk '{print $2}'), $(cargo-nextest --version 2>/dev/null | head -1))"
}

main "$@"
