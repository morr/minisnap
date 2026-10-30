#!/bin/bash
# Cargo runner (see .cargo/config.toml): run dev binaries inside a signed
# .app bundle so the TCC Screen Recording grant keyed to the stable
# identity follows cargo run. A bare binary is not the granted client: TCC
# attributes it to the responsible process (the terminal), the preflight
# reports "not granted" and the system prompt targets the terminal, not
# minisnap. A binary inside a bundle with the same bundle id and signing
# identity matches the grant's designated requirement regardless of where
# the bundle lives, so the runner keeps a scratch bundle next to the
# binary, refreshed on every run.
#
# Test harness binaries (minisnap-<hash>) and runs without a stable
# identity (MINISNAP_CODESIGN_IDENTITY= forces ad-hoc) pass through
# untouched — no bundle, no signing.
set -euo pipefail

BIN="$1"
if [ "$(basename "$BIN")" != "minisnap" ]; then
	exec "$@"
fi

IDENTITY="${MINISNAP_CODESIGN_IDENTITY-}"
if [ -z "$IDENTITY" ]; then
	IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null |
		awk -F'"' '/^[[:space:]]*[0-9]+\) [0-9A-Fa-f]+ / {print $2; exit}') || true
fi
if [ -z "$IDENTITY" ]; then
	exec "$@"
fi

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP_DIR="$(dirname "$BIN")/minisnap.app"
mkdir -p "$APP_DIR/Contents/MacOS"
cp "$ROOT/scripts/Info.plist" "$APP_DIR/Contents/Info.plist"
# Remove before copy: replacing a running executable's file in place fails.
rm -f "$APP_DIR/Contents/MacOS/minisnap"
cp "$BIN" "$APP_DIR/Contents/MacOS/minisnap"
if ! codesign --force --sign "$IDENTITY" "$APP_DIR" >/dev/null 2>&1; then
	echo "codesign-run: signing failed; TCC may not recognize this binary" >&2
fi
exec "$APP_DIR/Contents/MacOS/minisnap" "${@:2}"
