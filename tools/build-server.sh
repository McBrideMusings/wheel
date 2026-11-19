#!/bin/bash
# Build wheeld, wheel and wheel-runner for one configuration: build-server.sh Release|Debug.
# apple/Mac/embed-wheeld.sh runs it before packaging the server, and tools/install-app.sh runs it
# beside the Swift build so the embed step finds the binaries already built.
set -euo pipefail
case "${1:-}" in
  Release) CARGO_FLAGS=(--release) ;;
  Debug) CARGO_FLAGS=() ;;
  *) echo "usage: build-server.sh Release|Debug" >&2; exit 2 ;;
esac
REPO="$(cd "$(dirname "$0")/.." && pwd)"
# Xcode's environment (SDKROOT, deployment targets, its own PATH) leaks into cargo's C and Zig
# builds; build from a clean one.
env -i HOME="$HOME" USER="$USER" \
  PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  cargo build --manifest-path "$REPO/Cargo.toml" -p wheeld --bin wheeld -p wheel-cli --bin wheel -p wheel-runner --bin wheel-runner ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
