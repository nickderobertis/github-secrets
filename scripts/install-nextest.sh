#!/usr/bin/env sh
# Install cargo-nextest for the host platform if a working one isn't present.
#
# `just check` runs the suite through cargo-nextest (the inline test modules
# mutate process-global env vars, so they need a process per test — plain
# `cargo test` races and fails). CI installs nextest via a setup action, but a
# fresh local clone has nothing, and a binary copied from another machine is
# the wrong architecture and dies with "cannot execute binary file". This
# script gives every platform one reliable, idempotent install path; it is
# wired into `just bootstrap`.
#
# Strategy: skip if a working nextest is already on PATH; otherwise download the
# prebuilt binary for this host from get.nexte.st (fast, covers Linux x86_64 +
# arm64, macOS universal, and Windows under a POSIX shell); fall back to
# building from source if the platform is unrecognized or the download fails.
set -eu

if cargo nextest --version >/dev/null 2>&1; then
    echo "cargo-nextest already installed ($(cargo nextest --version 2>/dev/null | head -1))."
    exit 0
fi

bindir="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bindir"

# Map host OS/arch to the matching get.nexte.st alias. macOS is a single
# universal archive; Linux splits by arch; Windows ships a tar.gz for `tar`.
url=""
case "$(uname -s)" in
    Darwin) url="https://get.nexte.st/latest/mac" ;;
    Linux)
        case "$(uname -m)" in
            x86_64 | amd64) url="https://get.nexte.st/latest/linux" ;;
            aarch64 | arm64) url="https://get.nexte.st/latest/linux-arm" ;;
        esac
        ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT) url="https://get.nexte.st/latest/windows-tar" ;;
esac

if [ -n "$url" ] && command -v curl >/dev/null 2>&1; then
    echo "Installing cargo-nextest from $url …"
    tmp="$(mktemp)"
    # Download to a file first so a failed fetch (curl -f) is caught before we
    # try to untar it — piping curl|tar would mask the HTTP error.
    if curl -LsSf "$url" -o "$tmp" && tar -xzf "$tmp" -C "$bindir"; then
        rm -f "$tmp"
        echo "Installed cargo-nextest to $bindir ($(cargo nextest --version 2>/dev/null | head -1))."
        exit 0
    fi
    rm -f "$tmp"
    echo "Prebuilt download failed; building cargo-nextest from source instead." >&2
else
    echo "No prebuilt target for this host ($(uname -s)/$(uname -m)); building from source." >&2
fi

cargo install cargo-nextest --locked
echo "Installed cargo-nextest from source ($(cargo nextest --version 2>/dev/null | head -1))."
