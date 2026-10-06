#!/usr/bin/env bash
# The line-coverage gate, in three steps over cargo-llvm-cov's one profile
# directory (target/llvm-cov-target):
#
#   coverage.sh clear          drop every raw profile and instrumented artifact
#   coverage.sh test <crate>   run one crate's tests instrumented, keep the profiles
#   coverage.sh report         merge every crate's profiles; fail below the floor
#
# Each project's `test` target is step two, `coverage-aggregate:coverage-clear` is step one
# and `coverage-aggregate:coverage` is step three, so the floor is enforced once over the
# union of every crate's run — the e2e crate's journeys count toward the lines of
# the gh-secrets crate they drive. `--no-report` is what lets the crates share the
# directory: a reporting run clears every profile in it first.
#
# The floor covers the gh-secrets crate's own sources only (src/): the other
# workspace members are test and bench crates under tests/ and benches/, whose own
# lines are not product code. Nothing under src/ is excluded.
#
# On Windows the tests run uninstrumented and the report is skipped with a printed
# notice: cargo-llvm-cov there does not attribute the coverage of the binary the
# e2e journeys spawn, so the number would understate the crate and mean nothing.
# The floor is enforced on the Linux and macOS legs of the same gate.
#
# Exit status: 0 success; 1 a test run, the report, or the floor failed; 2 a
# usage error (unknown step, wrong argument count, or a crate that is not a
# workspace member).
set -euo pipefail

readonly MIN_LINES=95
readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

usage() {
  echo "coverage: $1" >&2
  echo "usage: scripts/coverage/coverage.sh clear | test <crate> | report" >&2
  exit 2
}

[ $# -ge 1 ] || usage "no step given"
readonly STEP="$1"
case "$STEP" in
  clear | report) [ $# -eq 1 ] || usage "'$STEP' takes no arguments (got $(($# - 1)))" ;;
  test) [ $# -eq 2 ] || usage "'test' takes exactly one crate name (got $(($# - 1)) arguments)" ;;
esac

is_windows() {
  case "${OS:-}${OSTYPE:-}" in
    *Windows_NT* | *msys* | *cygwin* | *win32*) return 0 ;;
  esac
  return 1
}

require() {
  local out
  if ! out="$(cargo "$1" --version 2>&1)"; then
    printf '%s\n' "$out" >&2
    echo "coverage: 'cargo $1 --version' failed (above) — cargo-$1 is missing or broken; run 'just bootstrap' (or 'cargo binstall cargo-$1')." >&2
    exit 1
  fi
}

# The crate selector must name a real workspace member: unchecked, a typo would
# measure nothing and pass. Only plain package names are accepted, and they are
# checked against the names of the packages `cargo metadata` lists in
# `workspace_members` (parsed as JSON by the pinned bun scripts/nx provides).
validate_crate() {
  local crate="$1"
  if ! printf '%s' "$crate" | grep -Eq '^[a-z0-9][a-z0-9-]*$'; then
    echo "coverage: '$crate' is not a valid crate name; pass a workspace package name (lowercase letters, digits, -)." >&2
    exit 2
  fi
  local metadata
  if ! metadata="$(cargo metadata --format-version 1 --no-deps --locked 2>&1)"; then
    printf '%s\n' "$metadata" >&2
    echo "coverage: 'cargo metadata' failed (above); fix the manifests so it resolves, then re-run." >&2
    exit 1
  fi
  local members
  if ! members="$(printf '%s' "$metadata" | bun -e '
    const m = JSON.parse(require("fs").readFileSync(0, "utf8"));
    const str = (v) => typeof v === "string" && v.length > 0;
    if (!Array.isArray(m.workspace_members) || !m.workspace_members.every(str)) throw new Error("workspace_members is not a list of ids");
    if (!Array.isArray(m.packages) || !m.packages.every((p) => p && str(p.id) && str(p.name))) throw new Error("packages are not {id, name} objects");
    const ids = new Set(m.workspace_members);
    console.log(m.packages.filter((p) => ids.has(p.id)).map((p) => p.name).join("\n"));
  ' 2>&1)"; then
    printf '%s\n' "$members" >&2
    echo "coverage: could not read the workspace members from 'cargo metadata' (above); check that bun is on PATH ('just bootstrap')." >&2
    exit 1
  fi
  if ! printf '%s\n' "$members" | grep -qxF -- "$crate"; then
    echo "coverage: '$crate' is not a member of this Cargo workspace; pass one of: $(printf '%s\n' "$members" | tr '\n' ' ')" >&2
    exit 2
  fi
}

case "$STEP" in
  clear)
    is_windows && exit 0
    require llvm-cov
    if ! out="$(cargo llvm-cov clean --workspace 2>&1)"; then
      printf '%s\n' "$out" >&2
      echo "coverage: could not clear target/llvm-cov-target; fix the error above and re-run." >&2
      exit 1
    fi
    ;;

  test)
    [ $# -eq 2 ] || usage
    readonly CRATE="$2"
    validate_crate "$CRATE"
    require nextest
    if is_windows; then
      echo "coverage: Windows — running $CRATE's tests uninstrumented (see scripts/coverage/coverage.sh)." >&2
      exec cargo nextest run -p "$CRATE" --locked
    fi
    require llvm-cov
    # The suites that spawn the binary find it beside their own test executables,
    # i.e. in target/llvm-cov-target; build the instrumented copy there first. A
    # no-op when it is already fresh, so concurrent crates never relink it under
    # each other. Its one `--version` run adds a profile covering only the
    # argument parsing that every journey executes anyway.
    if [ "$CRATE" != "gh-secrets" ]; then
      if ! out="$(cargo llvm-cov --no-report run -p gh-secrets --bin gh-secrets --locked -- --version 2>&1)"; then
        printf '%s\n' "$out" >&2
        echo "coverage: building the instrumented gh-secrets binary failed; fix the error above." >&2
        exit 1
      fi
    fi
    exec cargo llvm-cov --no-report nextest -p "$CRATE" --locked
    ;;

  report)
    if is_windows; then
      echo "coverage: report skipped on Windows (subprocess coverage is not attributed there); the Linux and macOS legs enforce ${MIN_LINES}%." >&2
      exit 0
    fi
    require llvm-cov
    # Every workspace member other than the gh-secrets crate lives under tests/ or
    # benches/; those are the only paths left out of the report.
    ignore="^$(printf '%s' "$ROOT" | sed 's/[][\.*^$+?(){}|]/\\&/g')/(tests|benches)/"
    if ! out="$(cargo llvm-cov report --summary-only --show-missing-lines \
      --ignore-filename-regex "$ignore" --fail-under-lines "$MIN_LINES" 2>&1)"; then
      printf '%s\n' "$out" >&2
      if printf '%s\n' "$out" | grep -q '^TOTAL '; then
        echo "coverage: the gh-secrets crate is below ${MIN_LINES}% line coverage over every crate's run." >&2
        echo "coverage: the uncovered lines are listed above — cover them with a test that drives the real behaviour." >&2
      else
        echo "coverage: no report could be produced (reason above). Run the tests first: 'just check' or 'just coverage'." >&2
      fi
      exit 1
    fi
    if ! total="$(printf '%s\n' "$out" | grep '^TOTAL ')"; then
      printf '%s\n' "$out" >&2
      echo "coverage: the report passed but has no TOTAL row (format above); check the cargo-llvm-cov version against scripts/coverage/coverage.sh." >&2
      exit 1
    fi
    printf '%s\n' "$total" | awk -v min="$MIN_LINES" '{ print "coverage: " $(NF-3) " lines covered (floor " min "%)" }' >&2
    ;;

  *)
    usage "unknown step '$STEP'"
    ;;
esac
