#!/bin/sh
# Package the GNOME Shell extension as a zip that `gnome-extensions install`
# accepts, with compiled GSettings schemas.
#
#   packaging/build-extension-zip.sh OUTPUT.zip
set -eu
REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(realpath -m "${1:?usage: build-extension-zip.sh OUTPUT.zip}")
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
cd "$REPO/gnome-extension"
cp -r metadata.json extension.js prefs.js dbus.js format.js stylesheet.css schemas icons "$STAGE/"
glib-compile-schemas --strict "$STAGE/schemas"
mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
cd "$STAGE"
zip -qr "$OUT" .
echo "$OUT"
