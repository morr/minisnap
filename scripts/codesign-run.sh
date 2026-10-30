#!/bin/bash
# Cargo runner (see .cargo/config.toml): launch dev builds of minisnap so
# the TCC Screen Recording grant applies. Two TCC facts drive the design:
#
# - A process is attributed to its "responsible process": a binary exec'd
#   from a terminal belongs to that terminal, not to minisnap, so the
#   grant keyed to the app never covers it and the system prompt targets
#   the terminal. Signing the bare binary does not change that.
# - A process launched through Launch Services is attributed to its own
#   app bundle, whose bundle id and signing identity match the grant's
#   designated requirement wherever the bundle lives.
#
# The runner therefore keeps a signed minisnap.app next to the dev binary
# (refreshed on every run) and launches it via `open`, streaming the app's
# output into the calling terminal. Ctrl-C stops the app.
#
# Test harness binaries (minisnap-<hash>) and runs without a stable
# identity (MINISNAP_CODESIGN_IDENTITY= forces ad-hoc) pass through
# untouched — no bundle, no Launch Services launch.
set -uo pipefail

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
	echo "codesign-run: no codesigning identity found; running the bare binary (Screen Recording will not follow it)" >&2
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
	echo "codesign-run: signing failed; Screen Recording may not recognize this build" >&2
fi

# `open` merely activates an already running instance, which would keep
# the stale binary alive: restart the previous run's instance first.
pkill -f "$APP_DIR/Contents/MacOS/minisnap" 2>/dev/null || true

# Launch through Launch Services and stream the app's output back. -W
# blocks until the app exits; Ctrl-C stops the app by SIGTERM (an
# AppleScript quit would trigger an automation permission prompt).
LOG="$(mktemp -t minisnap-run)"
open -W "$APP_DIR" --stdout "$LOG" --stderr "$LOG" &
OPEN_PID=$!
tail -n +1 -f "$LOG" &
TAIL_PID=$!
trap 'pkill -f "$APP_DIR/Contents/MacOS/minisnap" 2>/dev/null' INT TERM
wait "$OPEN_PID"
STATUS=$?
kill "$TAIL_PID" 2>/dev/null
rm -f "$LOG"
exit "$STATUS"
