#!/usr/bin/env bash
# Cross-build slideforge for all platforms the skill dispatcher supports.
#
# Prerequisites (one-time):
#   1. rustup with std for every target:
#        rustup target add aarch64-apple-darwin x86_64-apple-darwin \
#                         x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
#                         x86_64-pc-windows-gnu
#   2. zig 0.13 (official tarball is fine; do NOT `brew install zig` — it
#      compiles llvm from source for hours):
#        curl -LO https://ziglang.org/download/0.13.0/zig-macos-x86_64-0.13.0.tar.xz
#        tar xf zig-macos-x86_64-0.13.0.tar.xz && mv zig-macos-x86_64-0.13.0 /usr/local/opt/zig-0.13.0
#   3. cargo install cargo-zigbuild
#
# Usage: scripts/cross-build.sh [--install]
#   --install  also copy the artifacts into skill/slideforge-ppt/bin/
#
# Native targets (apple-darwin) build with plain `cargo build`; Linux and
# Windows cross builds go through cargo-zigbuild (zig provides the libc /
# CRT, so no Linux sysroot or mingw installation is needed on the host).
set -euo pipefail
cd "$(dirname "$0")/.."

ZIG_HOME="${ZIG_HOME:-/usr/local/opt/zig-0.13.0}"
export PATH="$ZIG_HOME:$HOME/.cargo/bin:$PATH"

TARGETS=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-pc-windows-gnu
)

for t in "${TARGETS[@]}"; do
  echo "== building $t"
  case "$t" in
    *-apple-darwin) cargo build --release --target "$t" ;;
    *)              cargo zigbuild --release --target "$t" ;;
  esac
done

if [[ "${1:-}" == "--install" ]]; then
  BIN=skill/slideforge-ppt/bin
  cp target/aarch64-apple-darwin/release/slideforge      "$BIN/slideforge-aarch64-apple-darwin"
  cp target/x86_64-apple-darwin/release/slideforge       "$BIN/slideforge-x86_64-apple-darwin"
  cp target/x86_64-unknown-linux-gnu/release/slideforge  "$BIN/slideforge-x86_64-unknown-linux-gnu"
  cp target/aarch64-unknown-linux-gnu/release/slideforge "$BIN/slideforge-aarch64-unknown-linux-gnu"
  # The dispatcher probes the msvc triple on Windows; the zig-built GNU exe
  # (static CRT via zig) runs on any x86_64 Windows 10+, so land it there.
  cp target/x86_64-pc-windows-gnu/release/slideforge.exe "$BIN/slideforge-x86_64-pc-windows-msvc.exe"
  echo "== installed into $BIN:"
  ls -la "$BIN"
  file "$BIN"/slideforge-* | sed 's/, for Mac.*//;s/, for free.*//;s/, version.*//'
fi
