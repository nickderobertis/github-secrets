#!/usr/bin/env bash
#
# Run a command with the *isolated* Bitwarden e2e account's api-key credentials
# exported, sourced from your own vault.
#
# The live Bitwarden suite (`just test-live-bitwarden`) needs the throwaway test
# account's credentials in the environment as:
#
#   GH_SECRETS_BW_E2E_CLIENT_ID
#   GH_SECRETS_BW_E2E_CLIENT_SECRET
#   GH_SECRETS_BW_E2E_PASSWORD
#
# Those credentials are stored — for exactly this purpose — in *your* Bitwarden
# vault as three secure notes (the value lives in each note's body):
#
#   BITWARDEN_TEST_CLIENT_ID
#   BITWARDEN_TEST_CLIENT_SECRET
#   BITWARDEN_TEST_MASTER_PASSWORD
#
# This script uses `gh-secrets` itself to pull those notes out of your vault and
# into the GH_SECRETS_BW_E2E_* env vars, then exec's whatever command you pass.
# It writes them only to a 0700 tempdir that is shredded on exit, and never
# prints a value. Your own vault unlocks via the usual precedence (shell env >
# .env > .env.local > stored config), so run it from the repo root.
#
# Usage:
#   scripts/bw-e2e-env.sh just test-live-bitwarden
#   scripts/bw-e2e-env.sh cargo nextest run --test e2e_live_bitwarden --no-fail-fast
#
# CI does NOT use this script — there the GH_SECRETS_BW_E2E_* vars come straight
# from repo secrets. This is purely the local-developer convenience.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/bw-e2e-env.sh <command> [args...]" >&2
  exit 2
fi

# Run the product CLI. Inside the repo (Cargo.toml present) prefer the in-tree
# build so this always matches the code under test, never a stale globally
# installed `gh-secrets`; otherwise fall back to one on PATH.
gh_secrets() {
  if [ -f Cargo.toml ] && command -v cargo >/dev/null 2>&1; then
    cargo run --quiet -- "$@"
  elif command -v gh-secrets >/dev/null 2>&1; then
    gh-secrets "$@"
  else
    echo "error: run from the repo root (needs Cargo.toml) or install gh-secrets" >&2
    return 1
  fi
}

tmp="$(mktemp -d)"
chmod 700 "$tmp"
home="$(mktemp -d)"
cleanup() { rm -rf "$tmp" "$home"; }
trap cleanup EXIT

# Pull the three secure notes out of your vault into a dotenv file, mapping each
# onto the env var the test suite reads. `#notes` selects the note body.
GH_SECRETS_HOME="$home" gh_secrets sync \
  --from bitwarden \
  --to "env:$tmp/creds.env" \
  --state "$tmp/state.json" \
  --secret "GH_SECRETS_BW_E2E_CLIENT_ID=BITWARDEN_TEST_CLIENT_ID#notes" \
  --secret "GH_SECRETS_BW_E2E_CLIENT_SECRET=BITWARDEN_TEST_CLIENT_SECRET#notes" \
  --secret "GH_SECRETS_BW_E2E_PASSWORD=BITWARDEN_TEST_MASTER_PASSWORD#notes" \
  >/dev/null

# Load them into this shell (no echo), then hand off to the requested command.
set -a
# shellcheck disable=SC1091
. "$tmp/creds.env"
set +a

exec "$@"
