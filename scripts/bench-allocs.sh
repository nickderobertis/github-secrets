#!/usr/bin/env bash
# Deterministic engine allocation counts, written to ${BENCH_OUT:-target/bench}/allocs.md
# (the Performance workflow's report reads it) and echoed to stdout. A failing
# bench fails this script — no empty or partial report is left behind as if it
# were a result.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly OUT="${BENCH_OUT:-$ROOT/target/bench}"

mkdir -p "$OUT"
tmp="$(mktemp "$OUT/allocs.md.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
if ! (cd "$ROOT" && cargo bench --locked --quiet -p gh-secrets-bench --bench engine_allocs) > "$tmp"; then
  echo "bench-allocs: the engine_allocs bench failed (above); no report was written. Fix it, then re-run 'just bench-allocs'." >&2
  exit 1
fi
mv "$tmp" "$OUT/allocs.md"
cat "$OUT/allocs.md"
