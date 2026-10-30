#!/bin/bash
# Cargo runner (see .cargo/config.toml): re-sign the binary cargo is about
# to launch with the same stable identity as the release .app, so the TCC
# Screen Recording grant keyed to that identity covers debug builds too.
# Ad-hoc signatures (cargo's default) are keyed to the exact binary and
# would re-prompt after every rebuild. Override with
# MINISNAP_CODESIGN_IDENTITY (empty string forces ad-hoc / no signing).
IDENTITY="${MINISNAP_CODESIGN_IDENTITY-}"
if [ -z "$IDENTITY" ]; then
	IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null |
		awk -F'"' '/^[[:space:]]*[0-9]+\) [0-9A-Fa-f]+ / {print $2; exit}') || true
fi
if [ -n "$IDENTITY" ]; then
	# The bundle id of the release .app: the identifier goes into the
	# designated requirement, and TCC matches Screen Recording grants by
	# that requirement — without it a dev binary is a different client
	# than the bundle and never sees the bundle's grant.
	if ! codesign --force --sign "$IDENTITY" --identifier app.minisnap.minisnap "$1" >/dev/null 2>&1
	then
		echo "codesign-run: signing failed; TCC may not recognize this binary" >&2
	fi
fi
exec "$@"
