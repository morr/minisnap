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

# Sign with a stable identity when one exists. An ad-hoc signature changes
# with every rebuild, and TCC then invalidates the Screen Recording grant
# after each build. Override with MINISNAP_CODESIGN_IDENTITY (empty string
# forces ad-hoc).
IDENTITY="${MINISNAP_CODESIGN_IDENTITY-}"
if [ -z "$IDENTITY" ]; then
	IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null |
		awk -F'"' '/^[[:space:]]*[0-9]+\) [0-9A-Fa-f]+ / {print $2; exit}') || true
fi
if [ -n "$IDENTITY" ]; then
	codesign --force --sign "$IDENTITY" "$APP"
else
	codesign --force --sign - "$APP"
	echo "warning: no codesigning identity found, signed ad-hoc; the" >&2
	echo "warning: Screen Recording grant will not survive rebuilds" >&2
fi
echo "Built $APP"
