#!/usr/bin/env bash
# Idempotent setup for the optional `llmlint` LLM-judge tier (oneharness + llmlint).
#
# Wired into the Claude Code SessionStart hook (.claude/settings.json) so web/cloud
# sessions can run `just lint-llm` / `just lint-llm-diff` with no manual steps; also
# safe to run by hand (`just setup-llmlint`) or from a terminal. Every step
# tolerates failure and the script always exits 0 — a flaky install must never
# break session startup.
#
# What it does, and why:
#   1. Installs the `llmlint` binary from PyPI via `uv tool`. `llmlint-cli` wraps
#      the prebuilt binary and depends on `oneharness-cli`, so one dependency
#      resolution fetches both wheels — no Rust toolchain and no github.com
#      reachability (works in restricted-egress sessions where PyPI is reachable).
#      `uv tool` links only the *requested* package's executable onto PATH, but
#      llmlint >= 0.3.23 finds `oneharness` beside its own binary in the tool venv —
#      so this one install is a complete setup; no separate oneharness install /
#      PATH entry. `--upgrade` bumps an older cached tool, honouring the floor below
#      (`just lint-llm-diff` needs the changed-file-scoped `--diff` and three-dot
#      `--diff-base` default; `just lint-llm-validate` needs the `validate` gate).
#   2. In a Claude Code session, persists PATH (so the freshly installed binary
#      resolves) into CLAUDE_ENV_FILE so later Bash calls inherit it.
#
# It sets no ONEHARNESS_* override: oneharness.toml's fallback list picks the
# harness, and an override would clobber that list.
# llmlint: ignore-file[tool_output_is_signal, boundary_inputs_validated, cli_output_contract] deliberate for a session-startup installer (see header): it always exits 0 so a flaky install can never abort the hook (CI's llmlint job checks `llmlint --version` right after, so a failed install still fails there); success stays quiet while failures log-and-continue rather than block startup; and the toolchain is installed from PyPI (`uv tool install llmlint-cli`) whose wheels ship with Trusted Publishing + PEP 740 attestations, so no unvalidated external input is executed.
set -uo pipefail

# Version floor, as a PyPI constraint (`llmlint-cli` tracks the binary's version;
# oneharness comes along transitively). 0.3.23 is the first release with every
# behaviour the recipes rely on: oneharness found beside llmlint, the
# changed-files `--diff` with merge-base `--diff-base`, and the `validate` gate.
# llmlint: ignore[changed_behavior_has_e2e] this dependency floor selects the validator release used by the existing real `just lint-llm-validate` gate; installer control flow and its user-visible contract are unchanged.
readonly LLMLINT_MIN="0.3.23"
readonly BIN_DIR="$HOME/.local/bin"
# The inherited PATH, captured before BIN_DIR is prepended below, so
# persist_session_env can tell whether the session already resolves it.
readonly ORIG_PATH="${PATH}"

log() { printf 'setup-llmlint: %s\n' "$*" >&2; }

# Install llmlint from PyPI via uv (the repo's Python package manager). uv is a
# clean-clone prerequisite; if it is somehow absent, log an actionable pointer and
# leave any already-installed binary in place rather than aborting startup.
ensure_toolchain() {
  if ! command -v uv >/dev/null 2>&1; then
    log "uv not found; cannot install llmlint (install uv: https://docs.astral.sh/uv/)"
    return 0
  fi
  log "installing llmlint-cli >= $LLMLINT_MIN via uv tool"
  uv tool install --upgrade "llmlint-cli>=$LLMLINT_MIN" >&2 \
    || log "llmlint-cli install failed (continuing)"
}

# Persist env for the rest of the session via CLAUDE_ENV_FILE (Claude Code sources
# it into every later Bash call). PATH so the freshly installed binaries resolve.
# No-op outside a session.
persist_session_env() {
  [ -n "${CLAUDE_ENV_FILE:-}" ] || { log "no CLAUDE_ENV_FILE (not a session); skipping env"; return 0; }
  case ":${ORIG_PATH}:" in
    *":${BIN_DIR}:"*) log "${BIN_DIR} already on the session PATH"; return 0 ;;
  esac
  if ! printf 'export PATH=%q\n' "${BIN_DIR}:${ORIG_PATH}" >> "$CLAUDE_ENV_FILE"; then
    log "could not write $CLAUDE_ENV_FILE; add ${BIN_DIR} to PATH yourself for this session"
    return 0
  fi
  log "exported PATH"
}

export PATH="${BIN_DIR}:${PATH}"
ensure_toolchain
persist_session_env
# `llmlint doctor` confirms the sibling `oneharness` is reachable (it is not on
# PATH — llmlint resolves it beside its own binary), so report via doctor.
if command -v llmlint >/dev/null 2>&1; then
  log "ready (llmlint: $(llmlint --version 2>/dev/null || echo unknown))"
  llmlint doctor >&2 2>&1 || log "llmlint doctor reported an issue (see above)"
else
  log "llmlint not installed"
fi
exit 0
