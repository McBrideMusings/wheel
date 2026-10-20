#!/bin/bash
# Xcode build phase: build wheeld for this configuration and put it inside the app, with the launch
# agent the app registers through SMAppService and the settings the app reads to reach it.
# Release runs the `wheel` instance on 7420; Debug (the dev build) runs `wheel-dev` on 7422 (7421 is the dev app's debug endpoint), so the
# two apps never share a daemon, a token or a layout. This script is the one place those live.
set -euo pipefail

if [ "$CONFIGURATION" = "Release" ]; then
  PROFILE=release CARGO_FLAGS=--release
  INSTANCE=wheel LISTEN=127.0.0.1:7420 LABEL=com.piercemakes.wheel.daemon
else
  PROFILE=debug CARGO_FLAGS=
  INSTANCE=wheel-dev LISTEN=127.0.0.1:7422 LABEL=com.piercemakes.wheel.dev.daemon
fi

REPO="$SRCROOT/.."
CONTENTS="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH"

# Xcode's environment (SDKROOT, deployment targets, its own PATH) leaks into cargo's C and Zig
# builds; build from a clean one.
env -i HOME="$HOME" USER="$USER" \
  PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  cargo build --manifest-path "$REPO/Cargo.toml" -p wheeld --bin wheeld -p wheel-cli --bin wheel $CARGO_FLAGS

mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Library/LaunchAgents" "$CONTENTS/Resources"
# -p keeps the build's modification time: the app restarts a daemon that started before it.
cp -p "$REPO/target/$PROFILE/wheeld" "$CONTENTS/MacOS/wheeld"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$CONTENTS/MacOS/wheeld"
# The `wheel` command beside it: wheeld links it into the bin directory its terminals and agents
# put on PATH, and `wheel install` links it into ~/.local/bin.
cp -p "$REPO/target/$PROFILE/wheel" "$CONTENTS/MacOS/wheel"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$CONTENTS/MacOS/wheel"

AGENT="$CONTENTS/Library/LaunchAgents/$LABEL.plist"
rm -f "$AGENT"
plutil -create xml1 "$AGENT"
plutil -insert Label -string "$LABEL" "$AGENT"
plutil -insert BundleProgram -string Contents/MacOS/wheeld "$AGENT"
plutil -insert ProgramArguments -json "[\"wheeld\",\"--instance\",\"$INSTANCE\",\"--listen\",\"$LISTEN\"]" "$AGENT"
plutil -insert AssociatedBundleIdentifiers -json "[\"$PRODUCT_BUNDLE_IDENTIFIER\"]" "$AGENT"
plutil -insert RunAtLoad -bool true "$AGENT"
plutil -insert KeepAlive -bool true "$AGENT"

INFO="$CONTENTS/Resources/WheelDaemon.plist"
rm -f "$INFO"
plutil -create xml1 "$INFO"
plutil -insert Label -string "$LABEL" "$INFO"
plutil -insert Instance -string "$INSTANCE" "$INFO"
plutil -insert Listen -string "$LISTEN" "$INFO"
echo "embedded wheeld ($PROFILE) as $LABEL: instance $INSTANCE on $LISTEN"
