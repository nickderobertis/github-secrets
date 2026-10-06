#!/usr/bin/env bash
# Deterministic engine allocation counts, written to ${BENCH_OUT:-target/bench}/allocs.md
# (the Performance workflow's report reads it). A failing bench fails this script
# and leaves any earlier report untouched — a partial table is never published as
# a result.
#
# Exit status: 0 the report was written (its path is printed); 1 the bench or the
# write failed (the message says which and what to do).
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly OUT="${BENCH_OUT:-$ROOT/target/bench}"

fail() {
  echo "bench-allocs: $*" >&2
  exit 1
}

mkdir -p "$OUT" || fail "cannot create $OUT; make it writable or set BENCH_OUT, then re-run 'just bench-allocs'."
tmp="$(mktemp "$OUT/allocs.md.XXXXXX")" || fail "cannot write in $OUT; make it writable or set BENCH_OUT."
trap 'rm -f "$tmp"' EXIT
(cd "$ROOT" && cargo bench --locked --quiet -p gh-secrets-bench --bench engine_allocs) > "$tmp" \
  || fail "the engine_allocs bench failed (above); no report was written. Fix it, then re-run 'just bench-allocs'."
mv "$tmp" "$OUT/allocs.md" || fail "cannot replace $OUT/allocs.md; check its permissions."
echo "bench-allocs: wrote $OUT/allocs.md" >&2
