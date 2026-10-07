#!/usr/bin/env bash
# Syntax-check every script with the shell that runs it: bash for the *.sh
# scripts and the nx wrapper, POSIX sh for install.sh and install-nextest.sh.
#
# A script rather than an inline Nx command because Nx runs inline commands
# through cmd.exe on Windows, which cannot parse a shell loop.
#
# Usage: lint.sh [dir]  (default: this script's own directory)
set -euo pipefail

dir="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"

for f in "$dir"/*.sh "$dir"/nx; do
  bash -n "$f" || { echo "lint: $f has a bash syntax error (above); fix it and re-run 'nx run scripts:lint'." >&2; exit 1; }
done
for f in "$dir"/install.sh "$dir"/install-nextest.sh; do
  sh -n "$f" || { echo "lint: $f is not POSIX sh (above); keep it portable and re-run 'nx run scripts:lint'." >&2; exit 1; }
done
