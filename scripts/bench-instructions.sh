#!/usr/bin/env bash
#
# Deterministic end-to-end CLI cost via instruction counts (valgrind's
# cachegrind, no cache simulation). Wall-clock timings (scripts/bench.sh) are
# noisy on shared hardware, so a small regression hides inside the jitter;
# instruction counts are reproducible to within ~0.1% (ASLR and environment
# size leave a little), which makes a base-vs-PR delta trustworthy where a
# hyperfine delta is not. Linux-only: it needs valgrind on PATH.
#
# Counts come from the `profiling` Cargo profile — codegen-matched to the
# shipped release profile, with symbols kept so a regression can be dug into
# with callgrind/cachegrind annotation tools afterwards.
#
# Usage:
#   scripts/bench-instructions.sh                   Run the suite.
#   scripts/bench-instructions.sh report BASE HEAD  Print a markdown delta table
#                                                   from two instructions.tsv files.
#
# Results: markdown table on stdout plus machine-readable exports under
# ${BENCH_OUT:-target/bench} (instructions.tsv, instructions.md).
#
# Environment overrides:
#   BENCH_OUT      output directory (default: <repo>/target/bench)
#   BENCH_SECRETS  managed secrets seeded into the config/source (default: 50)

set -euo pipefail

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

# `report` joins a base and a head TSV (case<TAB>instructions) into a markdown
# delta table; it needs no valgrind, so CI can run it after checking back out
# of the base revision.
if [[ "${1:-}" == "report" ]]; then
    [[ $# -eq 3 && -s "$2" && -s "$3" ]] ||
        fail "usage: bench-instructions.sh report BASE.tsv HEAD.tsv (both non-empty)"
    awk -F'\t' '
        NR == FNR { base[$1] = $2; next }
        FNR == 1 {
            print "| command | base | head | Δ instructions |"
            print "|---|---:|---:|---:|"
        }
        {
            if ($1 in base && base[$1] > 0) {
                delta = ($2 - base[$1]) / base[$1] * 100
                printf "| %s | %s | %s | %+.2f%% |\n", $1, base[$1], $2, delta
            } else {
                printf "| %s | — | %s | new |\n", $1, $2
            }
        }
    ' "$2" "$3"
    exit 0
fi

[[ "${1:-}" == "" ]] || fail "usage: bench-instructions.sh [report BASE.tsv HEAD.tsv]"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="$repo_root/target/profiling/gh-secrets"
out="${BENCH_OUT:-$repo_root/target/bench}"
secrets="${BENCH_SECRETS:-50}"

note() { printf '%s\n' "$*"; }

command -v valgrind >/dev/null 2>&1 ||
    fail "valgrind not found on PATH (Linux-only; install it with your package manager, e.g. 'apt-get install valgrind')."

note "» building binary (profiling profile)"
(cd "$repo_root" && cargo build --profile profiling --locked --quiet)
[ -x "$bin" ] || fail "profiling binary not found at $bin"

# Hermetic sandbox, mirroring scripts/bench.sh: a config root for the vault and
# a project dir holding the env-file source + config, so counts are reproducible
# and the host machine's own config/vault never leaks in.
sandbox="$(mktemp -d)"
cleanup() { rm -rf "$sandbox"; }
trap cleanup EXIT

home="$sandbox/home"
proj="$sandbox/project"
cfg="$proj/gh-secrets.json"
src="$proj/source.env"
mkdir -p "$home" "$proj"

export GH_SECRETS_HOME="$home"
export GH_SECRETS_PASSPHRASE="bench-passphrase"

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

for name in TOKEN_A TOKEN_B TOKEN_C; do
    printf 'seed-value-%s\n' "$name" | "$bin" store set "$name" >/dev/null
done

cd "$proj"

mkdir -p "$out"
tsv="$out/instructions.tsv"
md="$out/instructions.md"
: >"$tsv"

# Run one case under cachegrind and append its instruction count to the TSV.
# The first argument names the case; the rest is the command.
measure() {
    local name="$1"
    shift
    local log="$sandbox/cachegrind.log"
    valgrind --tool=cachegrind --cache-sim=no \
        --cachegrind-out-file="$sandbox/cachegrind.out" \
        --log-file="$log" -- "$@" >/dev/null
    local refs
    refs="$(awk '/I +refs:/ { gsub(",", "", $4); print $4; exit }' "$log")"
    [ -n "$refs" ] || fail "no instruction count for '$name' (see $log)"
    printf '%s\t%s\n' "$name" "$refs" >>"$tsv"
    note "  $name: $refs instructions"
}

note "» counting instructions ($bin)"
measure "version" "$bin" --version
measure "list" "$bin" list --config "$cfg"
measure "check" "$bin" check --config "$cfg"
# A full create-push: clear the write targets so the count is the create path,
# not an idempotent re-run.
rm -f "$proj/out.env" "$proj/.gh-secrets-state.json"
measure "sync" "$bin" sync --config "$cfg"
measure "source:list" "$bin" source list --config "$cfg"
measure "store:list" "$bin" store list

{
    echo "| command | instructions |"
    echo "|---|---:|"
    awk -F'\t' '{ printf "| %s | %s |\n", $1, $2 }' "$tsv"
} >"$md"

note ""
note "✓ wrote $tsv"
note "       $md"
