#!/usr/bin/env sh
# Install cargo-llvm-cov for the host platform if a working one isn't present.
#
# Every project's `test` target runs under cargo-llvm-cov and `workspace:coverage`
# enforces the line floor over the union (scripts/coverage.sh), so a clean clone
# needs it before `just check`. CI installs it with taiki-e/install-action, so this
# no-ops there; wired into `just bootstrap` for everyone else. Same shape as
# scripts/install-nextest.sh: skip when present, else the prebuilt release archive
# for this host, else build from source.
set -eu

if cargo llvm-cov --version >/dev/null 2>&1; then
    exit 0
fi

bindir="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bindir"

target=""
case "$(uname -s)/$(uname -m)" in
    Linux/x86_64 | Linux/amd64) target="x86_64-unknown-linux-gnu" ;;
    Linux/aarch64 | Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Darwin/x86_64) target="x86_64-apple-darwin" ;;
esac

if [ -n "$target" ] && command -v curl >/dev/null 2>&1; then
    url="https://github.com/taiki-e/cargo-llvm-cov/releases/latest/download/cargo-llvm-cov-$target.tar.gz"
    echo "Installing cargo-llvm-cov from $url …" >&2
    tmp="$(mktemp)"
    if curl -LsSf "$url" -o "$tmp" && tar -xzf "$tmp" -C "$bindir"; then
        rm -f "$tmp"
        exit 0
    fi
    rm -f "$tmp"
    echo "Prebuilt download failed; building cargo-llvm-cov from source instead." >&2
else
    echo "No prebuilt cargo-llvm-cov for this host ($(uname -s)/$(uname -m)); building from source." >&2
fi

cargo install cargo-llvm-cov --locked
