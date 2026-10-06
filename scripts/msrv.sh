#!/usr/bin/env bash
# Check the gh-secrets crate against the minimum Rust version it declares.
#
# The MSRV is read from the crate's own manifest (`rust-version`, inherited from
# [workspace.package]) via cargo metadata, never restated here, and clippy.toml's
# `msrv` must agree with it so clippy flags too-new APIs at the same floor. The
# toolchain is installed on demand (minimal profile) and used only for this check
# — `cargo +<msrv>` ignores rust-toolchain.toml.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() {
  echo "msrv: $*" >&2
  exit 1
}

msrv="$(cargo metadata --format-version 1 --no-deps --locked | python3 -I -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
print(next((p.get("rust_version") or "") for p in packages if p["name"] == "gh-secrets"))
' || true)"
printf '%s' "$msrv" | grep -Eq '^[0-9]+\.[0-9]+(\.[0-9]+)?$' \
  || fail "could not read gh-secrets' rust-version from cargo metadata (got '${msrv}'); run 'cargo metadata --no-deps --locked' to see why, fix the manifest, then re-run 'just msrv'."

clippy_msrv="$(sed -n 's/^msrv *= *"\([^"]*\)".*/\1/p' clippy.toml)"
[ "$clippy_msrv" = "$msrv" ] \
  || fail "clippy.toml msrv '${clippy_msrv}' differs from the crate's rust-version '${msrv}'; make them equal."

if ! rustup run "$msrv" rustc --version >/dev/null 2>&1; then
  echo "msrv: installing Rust $msrv (minimal profile)" >&2
  rustup toolchain install "$msrv" --profile minimal >&2 \
    || fail "could not install Rust $msrv via rustup (output above); check the network, or run 'rustup toolchain install $msrv --profile minimal' by hand and re-run 'just msrv'."
fi

exec cargo "+$msrv" check -p gh-secrets --locked --all-targets --all-features
