#!/usr/bin/env bash
# The measurement behind github-secrets-pool-maintenance-seconds (budgets.yaml):
# one steady-state pass of the declared pool maintenance (`just pool-maintenance`),
# the pass a regular post-run maintenance faces — never a first pass over history.
#
#   a. run the declared maintenance once, untimed, to clear any backlog;
#   b. build every workspace member's test targets the way the gate does
#      (cargo-llvm-cov + nextest, one crate at a time) under an alternative dev
#      profile that gives every unit a new metadata hash, then backdate exactly
#      the entries that build created past the 2-day window: they stand in for a
#      run's builds that have since been superseded;
#   c. run the ordinary gate build, so each unit's newest build is the ordinary one;
#   d. run the declared maintenance once more — the figure. The pruner itself
#      reports that pass's wall-clock seconds and breakdown to ONEBUDGETSPEC_RESULT;
#      this script only appends the untimed backlog pass's seconds to its detail.
#
# Every entry of every profile's deps/, .fingerprint/, build/ and incremental/
# (named by unit and metadata hash) is listed before and after (d): the run fails
# unless (d) removed something, removed only what (b) created, and left every
# other entry in place. The lists are kept under target/pool-maintenance-measure/.
#
# Linux only (GNU find/touch), like the hosts onevcs maintains. Run it under
# `onebudgetspec check`; it refuses to build anything without ONEBUDGETSPEC_RESULT.
# Exit status: 0 measured; 1 a step failed or the workload was not what it claims.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "measure: $*" >&2
  exit 1
}

[ -n "${ONEBUDGETSPEC_RESULT:-}" ] \
  || fail "ONEBUDGETSPEC_RESULT is not set, so the pass's figure would go nowhere; run it as 'just pool-maintenance-budget'."
readonly RESULT="$ONEBUDGETSPEC_RESULT"
unset ONEBUDGETSPEC_RESULT

readonly OUT="target/pool-maintenance-measure"
mkdir -p "$OUT"

# A dev-profile setting nothing in this repository uses: it changes every unit's
# metadata hash (so the alternative builds sit beside the ordinary ones in the same
# profile directories) without changing what is compiled.
readonly ALTERNATIVE=(env CARGO_PROFILE_DEV_OVERFLOW_CHECKS=false)

# Every build entry, one `<profile>/<dir>/<name>` per line, sorted.
entries() {
  find target -name .fingerprint -type d -prune -print | while read -r fp; do
    profile="$(dirname "$fp")"
    for dir in deps .fingerprint build incremental; do
      [ -d "$profile/$dir" ] && find "$profile/$dir" -mindepth 1 -maxdepth 1 -print
    done
  done | LC_ALL=C sort
}

# The gate's test build: each member's test targets as `coverage.sh test` builds
# them, running none of their tests (the `none()` filter skips every binary).
gate_build() {
  local crates crate
  crates="$(cargo metadata --format-version 1 --no-deps --locked | bun -e '
    const m = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const ids = new Set(m.workspace_members);
    console.log(m.packages.filter((p) => ids.has(p.id)).map((p) => p.name).join("\n"));
  ')" || fail "could not list the workspace members with 'cargo metadata' (above)."
  for crate in $crates; do
    "$@" cargo llvm-cov --no-report nextest -p "$crate" --locked -E 'none()' --no-tests=pass \
      || fail "building $crate's test targets failed (above); fix the build and re-run."
  done
}

# Run the declared maintenance, its pruner reporting to the file "$1".
maintain() {
  ONEBUDGETSPEC_RESULT="$1" just pool-maintenance \
    || fail "'just pool-maintenance' failed (above); run 'just bootstrap' if cargo-prune-superseded is missing."
}

# a. the backlog, untimed: its seconds are evidence for the detail, never the figure.
backlog="$(mktemp "${TMPDIR:-/tmp}/pool-maintenance-backlog.XXXXXX")"
trap 'rm -f "$backlog"' EXIT
maintain "$backlog"
backlog_seconds="$(bun -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).value.toFixed(3))' "$backlog")" \
  || fail "the backlog pass wrote no readable figure to $backlog; check that 'cargo-prune-superseded --version' is the release the justfile pins (re-run 'just bootstrap' if not)."
entries >"$OUT/before-alternative.txt"

# b. one run's worth of superseded builds, backdated past the window — and older
# than every entry already present, so none of those becomes superseded by them.
gate_build "${ALTERNATIVE[@]}"
entries >"$OUT/after-alternative.txt"
LC_ALL=C comm -13 "$OUT/before-alternative.txt" "$OUT/after-alternative.txt" >"$OUT/alternative.txt"
[ -s "$OUT/alternative.txt" ] || fail "the alternative build created no new entries, so there is nothing superseded to measure; check ALTERNATIVE in $0."
stamp=$(($(date +%s) - 3 * 86400))
if [ -s "$OUT/before-alternative.txt" ]; then
  oldest="$(xargs -d '\n' -r sh -c 'find "$@" -printf "%T@\n"' find <"$OUT/before-alternative.txt" | awk 'NR == 1 || $1 < min { min = $1 } END { print min }')"
  oldest="${oldest%.*}"
  [ "$((oldest - 3600))" -lt "$stamp" ] && stamp=$((oldest - 3600))
fi
xargs -d '\n' -r sh -c 'find "$@" -exec touch -h -d "@$0" {} +' "$stamp" <"$OUT/alternative.txt"

# c. the ordinary gate build, newest for every unit.
gate_build
entries >"$OUT/before-pass.txt"

# d. the timed pass.
maintain "$RESULT"
entries >"$OUT/after-pass.txt"

LC_ALL=C comm -23 "$OUT/before-pass.txt" "$OUT/after-pass.txt" >"$OUT/removed.txt"
LC_ALL=C comm -23 "$OUT/removed.txt" "$OUT/alternative.txt" >"$OUT/removed-not-alternative.txt"
LC_ALL=C comm -23 "$OUT/before-pass.txt" "$OUT/alternative.txt" >"$OUT/ordinary.txt"
removed=$(wc -l <"$OUT/removed.txt")
ordinary=$(wc -l <"$OUT/ordinary.txt")
ordinary_kept=$(LC_ALL=C comm -12 "$OUT/ordinary.txt" "$OUT/after-pass.txt" | wc -l)
echo "measure: alternative build created $(wc -l <"$OUT/alternative.txt") entries; the timed pass removed $removed of them; ordinary entries before/after: $ordinary/$ordinary_kept (lists in $OUT)" >&2
[ "$removed" -gt 0 ] || fail "the timed pass removed nothing; the workload did not supersede any build."
[ -s "$OUT/removed-not-alternative.txt" ] && fail "the timed pass removed entries the alternative build did not create (see $OUT/removed-not-alternative.txt)."
[ "$ordinary_kept" -eq "$ordinary" ] || fail "the timed pass did not keep every ordinary entry (see $OUT/ordinary.txt)."

bun -e '
  const fs = require("fs");
  const [file, seconds] = process.argv.slice(1);
  const result = JSON.parse(fs.readFileSync(file, "utf8"));
  result.detail = `${result.detail}; untimed backlog pass (not in the figure): ${seconds} s`;
  fs.writeFileSync(file, JSON.stringify(result));
' "$RESULT" "$backlog_seconds" || fail "could not add the backlog pass to the report in $RESULT; check that the timed pass wrote valid JSON there, then re-run 'just pool-maintenance-budget'."
