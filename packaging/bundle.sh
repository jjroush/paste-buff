#!/bin/sh
# Assemble paste-buff.app around a built binary.
#
#   packaging/bundle.sh <binary> <version> <out-dir>
#
# Produces <out-dir>/paste-buff.app (unsigned). The release workflow signs it,
# wraps it in a DMG and notarizes; for a local build, an ad-hoc signature is
# enough to run it: codesign -s - --force <out-dir>/paste-buff.app
set -eu

[ $# -eq 3 ] || { echo "usage: $0 <binary> <version> <out-dir>" >&2; exit 2; }
bin="$1"
version="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
app="$out/paste-buff.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/paste-buff"
cp "$here/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "$app"
