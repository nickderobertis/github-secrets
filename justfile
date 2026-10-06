# Canonical command surface for gh-secrets.
#
# `just bootstrap` works from a clean clone; `just check` is the strict gate (no
# warnings-only mode). The gate recipes DELEGATE to Nx (scripts/nx-tier.sh ->
# scripts/nx.sh): each project declares what its targets do (cargo fmt, clippy,
# nextest under cargo-llvm-cov), and the root only chooses which projects run
# them. Every gate recipe takes a tier: `affected` (the default) runs the
# projects the change can reach, keyed off NX_BASE or the merge base with
# origin/master; `all` is one full sweep over every project. See AGENTS.md
# "Commits, releases, and merging" for which CI run uses which tier.

set shell := ["bash", "-uc"]
set windows-shell := ["bash", "-uc"]

# List available recipes.
default:
    @just --list

# Set up from a clean clone: the pinned Rust toolchain, cargo-nextest and
# cargo-llvm-cov, the pinned bun + the locked Nx install, the git hooks, the
# llmlint tier (best effort; CI's llmlint job installs it itself), and a crate pre-fetch.
bootstrap:
    rustup toolchain install 2>/dev/null || rustup show >/dev/null
    sh scripts/install-nextest.sh
    sh scripts/install-llvm-cov.sh
    bash scripts/bun.sh ensure
    bash scripts/nx.sh --version >/dev/null
    git config core.hooksPath .githooks
    @[ -n "${CI:-}" ] || bash scripts/setup-llmlint.sh
    cargo fetch --locked

# Full quality gate: format check, clippy (-D warnings), build, every project's
# tests (unit, offline e2e, the compiled no-op live suites) under coverage, and
# the 95% line-coverage floor over their union. `just check all` sweeps everything.
check tier="affected":
    bash scripts/nx-tier.sh {{ tier }} format-check lint build test coverage

# The test targets alone (each writes its coverage profiles; no floor).
test tier="affected":
    bash scripts/nx-tier.sh {{ tier }} test

# The offline e2e suite in isolation (also run by `check`). Builds the binary first.
test-e2e:
    bash scripts/nx.sh run gh-secrets-e2e:test

# Live end-to-end tests against the real GitHub API. Requires `GH_TOKEN` with
# `repo` scope (covers `secrets:write`); creates and reuses a private sandbox
# repo `gh-secrets-e2e-sandbox` on the authenticated user's account.
test-live:
    bash scripts/nx.sh run gh-secrets-live-github:live

# Live end-to-end tests against a real, isolated Bitwarden account. Requires the
# isolated account's api-key credentials in the environment:
# `GH_SECRETS_BW_E2E_CLIENT_ID`, `GH_SECRETS_BW_E2E_CLIENT_SECRET`,
# `GH_SECRETS_BW_E2E_PASSWORD`, plus the `bw` CLI on PATH. Locally, run it via
# `scripts/bw-e2e-env.sh just test-live-bitwarden`, which pulls those creds out
# of your own vault (where they live as the `BITWARDEN_TEST_*` secure notes).
# Without the creds, every test skips as a no-op. Runs serially (`-j1`).
test-live-bitwarden:
    bash scripts/nx.sh run gh-secrets-live-bitwarden:live

# Lint (clippy -D warnings per crate, plus the project-boundary and workflow-contract checks).
lint tier="affected":
    bash scripts/nx-tier.sh {{ tier }} lint

# Format check (used by the gate; does not write files).
format-check tier="affected":
    bash scripts/nx-tier.sh {{ tier }} format-check

# Format every project in place.
format:
    bash scripts/nx.sh run-many --all -t format

# Every crate's tests under cargo-llvm-cov, then the 95% line floor over the union.
coverage:
    bash scripts/nx.sh run workspace:coverage

# Supply chain: cargo-deny (advisories, licenses, bans, sources) + cargo-machete.
# Linux-only in CI, in its own job. Needs cargo-deny and cargo-machete installed.
supply-chain:
    bash scripts/nx.sh run workspace:supply-chain

# Check the gh-secrets crate against the MSRV its manifest declares.
msrv:
    bash scripts/msrv.sh

# Update dependencies, then re-run the full gate as a sweep (an upgrade can reach anything).
upgrade:
    cargo update
    bun update
    @just check all

# Build a release binary (for the host, or for one target triple).
release target="":
    cargo build --release --locked {{ if target == "" { "" } else { "--target " + target } }}

# ---- llmlint (LLM-judge tier) ----
#
# Kept OUT of `check`: it drives a real coding harness (non-deterministic,
# credentialed, networked). The `llmlint` CI job runs validate, then the
# diff-scoped judge. Config: llmlint.yml (harness choice: oneharness.toml).

# Provision the dev toolchain for a Claude Code session (also its SessionStart hook).
session-setup:
    ./scripts/session-setup.sh

# Install/refresh the llmlint toolchain (llmlint + oneharness). Idempotent.
setup-llmlint:
    ./scripts/setup-llmlint.sh

# LLM-judge lint over the configured set (or the paths given).
lint-llm *paths:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'" >&2; exit 1; }
    llmlint {{ paths }}

# Fast, model-free llmlint gate: config structure, ignore directives, fragment bumps.
lint-llm-validate *args:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'" >&2; exit 1; }
    llmlint validate {{ args }}

# llmlint over what this branch changed since it forked from origin/master (the blocking PR check).
lint-llm-diff base="origin/master" *args:
    @command -v llmlint >/dev/null 2>&1 || { echo "llmlint not installed — run 'just setup-llmlint'" >&2; exit 1; }
    llmlint --diff --diff-base "{{ base }}" {{ args }}

# ---- performance ----
#
# Informational, never a gate: timings are noisy on shared hardware, so these
# report numbers rather than block. The CI Performance workflow runs them on
# every PR and posts a sticky comment. See benches/AGENTS.md.

# Engine micro-benchmarks (Criterion); saves the `current` baseline for bench-compare.
bench:
    bash scripts/nx.sh run gh-secrets-bench:bench --baseline=current

# Save current engine benchmarks as the `base` baseline (run on the comparison point).
bench-base:
    bash scripts/nx.sh run gh-secrets-bench:bench --baseline=base

# Diff the latest `bench` run against `base` (run `bench-base` first; needs critcmp).
bench-compare:
    critcmp base current

# End-to-end CLI latency for the offline verbs (hyperfine); writes target/bench/results.*.
bench-cli:
    @bash scripts/nx.sh run gh-secrets-bench:bench-cli

# Fast smoke check of the CLI benchmark harness (one run, no warmup, no stable numbers).
bench-cli-smoke:
    @bash scripts/nx.sh run gh-secrets-bench:bench-cli --mode=--dry-run

# Deterministic engine allocation counts (counting allocator; exact, comparable across commits).
bench-allocs:
    @bash scripts/nx.sh run gh-secrets-bench:bench-allocs

# Deterministic end-to-end CLI instruction counts (cachegrind; Linux-only, needs valgrind).
bench-instructions:
    @bash scripts/nx.sh run gh-secrets-bench:bench-instructions

# Run the portable benchmark layers (Criterion + hyperfine + allocation counts).
bench-all: bench bench-cli bench-allocs
