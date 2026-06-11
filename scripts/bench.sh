#!/usr/bin/env bash
#
# End-to-end CLI latency benchmark. Drives the optimized release binary the way
# a user (or a CI step) does — one process per command — and measures wall-clock
# time with hyperfine across the offline verbs. This captures the cost that
# matters in practice: process startup + config discovery (fs) + dotenv/source
# parse + SHA-256 change detection + env-file write, which the in-process
# Criterion benches (`benches/engine.rs`) deliberately exclude.
#
# Every benchmarked command is fully offline and hermetic: the source and
# destination are env files in a throwaway sandbox, the config root
# (GH_SECRETS_HOME) and vault passphrase come from the environment, and no
# command needs the network, a GitHub token, or the Bitwarden CLI.
#
# Usage:
#   scripts/bench.sh            Full run (warmup + adaptive sampling).
#   scripts/bench.sh --dry-run  One run, no warmup — a fast smoke check that the
#                               harness and every command still work (used by CI
#                               and `just`), without depending on stable numbers.
#
# Results: human table on stdout plus machine-readable exports under
# ${BENCH_OUT:-target/bench} (results.json, results.md).
#
# Environment overrides:
#   BENCH_OUT      output directory (default: <repo>/target/bench)
#   BENCH_WARMUP   warmup runs before timing (default: 10)
#   BENCH_SECRETS  managed secrets seeded into the config/source/state
#                  (default: 50; 5 under --dry-run)
#   BENCH_KEEP     set to 1 to keep the temp sandbox for inspection

set -euo pipefail

mode="${1:-run}"
case "$mode" in
    run | --dry-run) ;;
    *)
        echo "usage: bench.sh [--dry-run]" >&2
        exit 2
        ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$repo_root/target/release/gh-secrets"
out="${BENCH_OUT:-$repo_root/target/bench}"
warmup="${BENCH_WARMUP:-10}"

note() { printf '%s\n' "$*"; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

if ! command -v hyperfine >/dev/null 2>&1; then
    fail "hyperfine not found on PATH. Install it with 'cargo binstall hyperfine' or your package manager (the CI Performance workflow installs it via taiki-e/install-action)."
fi

# A `--dry-run` proves the harness and commands work without spending time on
# statistics; the full run warms up and lets hyperfine sample adaptively. The
# managed-secret count shrinks with it so the smoke check stays fast.
runs_opt=()
secrets_default=50
if [[ "$mode" == "--dry-run" ]]; then
    warmup=0
    runs_opt=(--runs 1)
    secrets_default=5
fi
secrets="${BENCH_SECRETS:-$secrets_default}"

note "» building release binary"
(cd "$repo_root" && cargo build --release --locked --quiet)
[ -x "$bin" ] || fail "release binary not found at $bin"

# Hermetic sandbox: a config root for the vault, a project dir holding the
# env-file source + config, and an empty dir for `init`. Nothing here touches
# the developer's real config or vault.
sandbox="$(mktemp -d)"
cleanup() { [ "${BENCH_KEEP:-0}" = "1" ] || rm -rf "$sandbox"; }
trap cleanup EXIT

home="$sandbox/home"
proj="$sandbox/project"
initdir="$sandbox/initdir"
cfg="$proj/gh-secrets.json"
src="$proj/source.env"
mkdir -p "$home" "$proj" "$initdir"

export GH_SECRETS_HOME="$home"
# Lets the vault-backed `store` command run without an interactive prompt; this
# is a throwaway sandbox vault, so the passphrase is not a secret.
export GH_SECRETS_PASSPHRASE="bench-passphrase"

# A dotenv source of `secrets` keys and a matching env-file → env-file config.
note "» seeding env source + config ($secrets secrets)"
: >"$src"
for ((i = 0; i < secrets; i++)); do
    printf 'SECRET_%d="value-%d-%s"\n' "$i" "$i" "xxxxxxxxxxxxxxxxxxxxxxxx" >>"$src"
done
{
    printf '{\n  "source": { "type": "env_file", "path": "source.env" },\n  "secrets": [\n'
    for ((i = 0; i < secrets; i++)); do
        [ "$i" -gt 0 ] && printf ',\n'
        printf '    { "name": "SECRET_%d" }' "$i"
    done
    printf '\n  ],\n  "destinations": [\n    { "type": "env_file", "path": "out.env" }\n  ]\n}\n'
} >"$cfg"

# Seed a few values into the vault so `store list` reads a non-empty store. The
# value comes on stdin so it never lands in a process argv.
note "» seeding local store"
for name in TOKEN_A TOKEN_B TOKEN_C; do
    printf 'seed-value-%s\n' "$name" | "$bin" store set "$name" >/dev/null
done

mkdir -p "$out"

# Run from the project dir so the relative config resolves and dotenv auto-load
# only ever sees this sandbox.
cd "$proj"

note "» benchmarking $bin"
# One invocation so a single export holds every command. `--prepare` clears the
# write targets before each run so `sync` always measures a full create-push
# (and `check` the all-pending read), and removes any config `init` wrote so it
# measures the create-from-empty path each time. The deny-free read commands are
# unaffected by the resets.
hyperfine \
    --warmup "$warmup" "${runs_opt[@]}" \
    --prepare "rm -f '$proj/out.env' '$proj/.gh-secrets-state.json' '$initdir/gh-secrets.json'" \
    --export-json "$out/results.json" \
    --export-markdown "$out/results.md" \
    -n "version" "'$bin' --version" \
    -n "help" "'$bin' --help" \
    -n "list" "'$bin' list --config '$cfg' > /dev/null" \
    -n "check" "'$bin' check --config '$cfg' > /dev/null" \
    -n "sync" "'$bin' sync --config '$cfg' > /dev/null" \
    -n "source:list" "'$bin' source list --config '$cfg' > /dev/null" \
    -n "store:list" "'$bin' store list > /dev/null" \
    -n "init" "cd '$initdir' && '$bin' init > /dev/null"

note ""
note "✓ wrote $out/results.json"
note "       $out/results.md"
