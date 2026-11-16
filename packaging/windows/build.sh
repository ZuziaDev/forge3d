#!/usr/bin/env bash
# Cross-compile forge3d for Windows and build the NSIS installer.
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64, nsis.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
out="${1:-$root/dist}"
version="$(grep -m1 '^version' "$root/Cargo.toml" | sed 's/.*"\(.*\)"/\1/')"
cd "$root"
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release --target x86_64-pc-windows-gnu -p forge3d-cli
mkdir -p "$out"
exe="$target_dir/x86_64-pc-windows-gnu/release/forge3d.exe"
x86_64-w64-mingw32-strip "$exe" || true
cp "$exe" "$out/forge3d.exe"
makensis -V2 -NOCD -DVERSION="$version" -DEXE="$exe" \
  -DOUTFILE="$out/forge3d-$version-setup.exe" "$root/packaging/windows/forge3d.nsi"
ls -la "$out"
