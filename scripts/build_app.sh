#!/bin/bash
# Build the minisnap .app bundle around the cargo binary.
# Usage: scripts/build_app.sh [release|debug]
set -euo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-release}"
if [ "$MODE" = "debug" ]; then
	cargo build
	BIN="target/debug/minisnap"
	APP="target/debug/minisnap.app"
else
	cargo build --release
	BIN="target/release/minisnap"
	APP="target/release/minisnap.app"
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp scripts/Info.plist "$APP/Contents/Info.plist"
cp "$BIN" "$APP/Contents/MacOS/minisnap"
codesign --force --sign - "$APP"
echo "Built $APP"
