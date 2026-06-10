# Canonical command surface for gh-secrets.
#
# `just bootstrap` works from a clean clone; `just check` is the strict gate
# (no warnings-only mode). E2E runs as part of the gate.

# List available recipes.
default:
    @just --list

# Set up from a clean clone: fetch toolchain components + pre-fetch crates.
bootstrap:
    rustup component add rustfmt clippy
    cargo fetch --locked

# Full quality gate: format check, clippy, unit + integration, and e2e.
check: format-check lint test test-e2e

# Fast unit + binary tests (the inline `#[cfg(test)]` modules).
test:
    cargo nextest run --lib --bins

# End-to-end tests: drive the compiled binary against a mock GitHub server.
# Also compiles+runs the live-test binary (`e2e_live`); each live test
# early-returns as a no-op when `GH_SECRETS_LIVE_TEST` is unset, so the gate
# catches breakage in the live test code without paying for network calls.
test-e2e:
    cargo nextest run --test e2e --test e2e_manifest --test e2e_live

# Live end-to-end tests against the real GitHub API. Requires `GH_TOKEN` with
# `repo` scope (covers `secrets:write`); creates and reuses a private sandbox
# repo `gh-secrets-e2e-sandbox` on the authenticated user's account.
test-live:
    GH_SECRETS_LIVE_TEST=1 cargo nextest run --test e2e_live --no-fail-fast

# Lint with clippy. Warnings are errors.
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Format the codebase in place.
format:
    cargo fmt --all

# Format check (used by the gate; does not write files).
format-check:
    cargo fmt --all -- --check

# Update dependencies, then re-run the full gate.
upgrade:
    cargo update
    @just check

# Build a release binary.
release:
    cargo build --release --locked
