#!/bin/bash
# Xcode build phase: build wheeld for this configuration and put it inside the app, with the script
# that installs and runs it and the settings the app reads to reach it.
# Release runs the `wheel` instance on 7420; Debug (the dev build) runs `wheel-dev` on 7422 (7421 is the dev app's debug endpoint), so the
# two apps never share a daemon, a token or a layout. This script is the one place those live.
set -euo pipefail

if [ "$CONFIGURATION" = "Release" ]; then
  PROFILE=release CARGO_FLAGS=--release
  INSTANCE=wheel LISTEN=127.0.0.1:7420 LABEL=com.piercemakes.wheel.daemon
  BUNDLE_ID=com.piercemakes.wheel.daemon SERVER_NAME="Wheel Server"
else
  PROFILE=debug CARGO_FLAGS=
  INSTANCE=wheel-dev LISTEN=127.0.0.1:7422 LABEL=com.piercemakes.wheel.dev.daemon
  BUNDLE_ID=com.piercemakes.wheel.dev.daemon SERVER_NAME="Wheel Dev Server"
fi

REPO="$SRCROOT/.."
CONTENTS="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH"

# Xcode's environment (SDKROOT, deployment targets, its own PATH) leaks into cargo's C and Zig
# builds; build from a clean one.
env -i HOME="$HOME" USER="$USER" \
  PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  cargo build --manifest-path "$REPO/Cargo.toml" -p wheeld --bin wheeld -p wheel-cli --bin wheel -p wheel-runner --bin wheel-runner $CARGO_FLAGS

# wheeld ships as its own bundle, Wheel Server: macOS names that bundle (its Info.plist name,
# icon and folder usage strings) in Privacy & Security, and a grant to it is what terminals and
# agents started by wheeld use. The inner executable is signed first, then the bundle.
# The bundle is called .wheelserver here, not .app, and `Resources/wheel-server.sh` copies it out of
# Wheel.app to Application Support as Wheel Server.app before it runs: macOS charges a program that
# runs from inside Wheel.app to Wheel, whatever its own bundle id says.
SERVER="$CONTENTS/Helpers/$SERVER_NAME.wheelserver"
rm -rf "$SERVER" "$CONTENTS/Helpers/$SERVER_NAME.app" "$CONTENTS/MacOS/wheeld" "$CONTENTS/Library/LaunchAgents"
mkdir -p "$SERVER/Contents/MacOS" "$SERVER/Contents/Resources" "$CONTENTS/Resources"
cp "$REPO/target/$PROFILE/wheeld" "$SERVER/Contents/MacOS/wheeld"
cp "$SRCROOT/Mac/ServerIcon/ServerIcon.icns" "$SERVER/Contents/Resources/ServerIcon.icns"
sed -e "s|@BUNDLE_ID@|$BUNDLE_ID|g" -e "s|@NAME@|$SERVER_NAME|g" "$REPO/daemon/Info.plist.in" > "$SERVER/Contents/Info.plist"
plutil -lint "$SERVER/Contents/Info.plist" >/dev/null
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$SERVER/Contents/MacOS/wheeld"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$SERVER"
# The `wheel` command: wheeld links it into the bin directory its terminals and agents put on
# PATH, and `wheel install` links it into ~/.local/bin. It sits in Helpers, not MacOS, because the
# app's own executable is `Wheel` and the volume ignores case, so `MacOS/wheel` would replace it.
mkdir -p "$CONTENTS/Helpers"
cp -p "$REPO/target/$PROFILE/wheel" "$CONTENTS/Helpers/wheel"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$CONTENTS/Helpers/wheel"
# The runner process: wheeld links it into the bin directory beside `wheel`, where the runner plugin's backend command (`wheel-runner backend`) finds it.
cp -p "$REPO/target/$PROFILE/wheel-runner" "$CONTENTS/Helpers/wheel-runner"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" "$CONTENTS/Helpers/wheel-runner"

cp -p "$SRCROOT/Mac/wheel-server.sh" "$CONTENTS/Resources/wheel-server.sh"

INFO="$CONTENTS/Resources/WheelDaemon.plist"
rm -f "$INFO"
plutil -create xml1 "$INFO"
plutil -insert Label -string "$LABEL" "$INFO"
plutil -insert Instance -string "$INSTANCE" "$INFO"
plutil -insert Listen -string "$LISTEN" "$INFO"
plutil -insert ServerName -string "$SERVER_NAME" "$INFO"
# The source the bundled wheeld was built from: the app compares it with the running wheeld's and
# offers a restart when they differ; it never restarts the daemon itself (ADR-0020).
SOURCE="$("$SERVER/Contents/MacOS/wheeld" --source-hash)"
plutil -insert SourceHash -string "$SOURCE" "$INFO"
echo "embedded wheeld ($PROFILE) as $LABEL: instance $INSTANCE on $LISTEN, source $SOURCE"
