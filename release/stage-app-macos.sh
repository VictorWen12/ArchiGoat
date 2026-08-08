#!/usr/bin/env bash
# Precomputes the icon and metadata half of the macOS App so the release job only signs.
set -euo pipefail

: "${ARCHIGOAT_BUNDLE_ID:?ARCHIGOAT_BUNDLE_ID required}"
: "${ARCHIGOAT_URL_SCHEME:?ARCHIGOAT_URL_SCHEME required}"

VERSION="${1:?version required}"
COMMIT="${2:?release commit required}"
STAGE_INPUT="${3:?stage directory required}"
SOURCE="$(cd "$(dirname "$0")" && pwd)"
ICON="$SOURCE/archigoat-icon.png"
WORK="$(mktemp -d)"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
[[ "$COMMIT" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]
test -f "$ICON"
install -d "$STAGE_INPUT"
STAGE="$(cd "$STAGE_INPUT" && pwd)"

ICONSET="$WORK/ArchiGoat.iconset"
install -d "$ICONSET"
ICON_SIZES=(16 32 64 128 256 512 1024)
for size in "${ICON_SIZES[@]}"; do
  sips -z "$size" "$size" "$ICON" --out "$ICONSET/$size.png" >/dev/null
done
node "$SOURCE/package-icns.mjs" "$ICONSET" "$STAGE/ArchiGoat.icns"

install -m 644 "$SOURCE/Info.plist" "$STAGE/Info.plist"
plutil -replace CFBundleDisplayName -string "ArchiGoat" "$STAGE/Info.plist"
plutil -replace CFBundleIdentifier -string "$ARCHIGOAT_BUNDLE_ID" "$STAGE/Info.plist"
plutil -replace CFBundleURLTypes.0.CFBundleURLName -string "$ARCHIGOAT_BUNDLE_ID" "$STAGE/Info.plist"
plutil -replace CFBundleURLTypes.0.CFBundleURLSchemes.0 -string "$ARCHIGOAT_URL_SCHEME" "$STAGE/Info.plist"
plutil -replace NSAppDataUsageDescription -string "ArchiGoat launches your selected Agent with its native permissions. Each Work starts in its own workspace." "$STAGE/Info.plist"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$STAGE/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$STAGE/Info.plist"
plutil -replace ArchiGoatCommit -string "$COMMIT" "$STAGE/Info.plist"
plutil -lint "$STAGE/Info.plist"
test -s "$STAGE/ArchiGoat.icns" -a -s "$STAGE/Info.plist"
