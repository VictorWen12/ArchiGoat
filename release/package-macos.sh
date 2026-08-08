#!/usr/bin/env bash
# Builds one signed universal ArchiGoat App inside one DMG.
set -euo pipefail

verify_image() (
  set -euo pipefail
  local dmg="$1"
  local version="$2"
  local commit="$3"
  local identity="$4"
  local team_id="$5"
  local verify_work
  verify_work="$(mktemp -d)"
  local mount="$verify_work/mount"
  local app="$mount/ArchiGoat.app"
  local daemon_pid=""

  cleanup_verify() {
    if [[ -n "$daemon_pid" ]]; then
      kill "$daemon_pid" 2>/dev/null || true
      wait "$daemon_pid" 2>/dev/null || true
    fi
    /usr/bin/hdiutil detach "$mount" -force >/dev/null 2>&1 || true
    rm -rf "$verify_work"
  }
  trap cleanup_verify EXIT

  test -s "$dmg"
  /usr/bin/hdiutil verify "$dmg"
  install -d "$mount"
  local attach_output
  if ! attach_output="$(/usr/bin/hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$dmg" 2>&1)"; then
    printf '%s\n' "$attach_output" >&2
    exit 1
  fi
  printf '%s\n' "$attach_output"
  test -d "$app"

  local binary name archs
  for name in archigoat archigoat-shell; do
    binary="$app/Contents/MacOS/$name"
    test -x "$binary"
    /usr/bin/codesign --verify --strict --verbose=2 "$binary"
    "$binary" --verify-release "$version" "$commit"
    archs="$(/usr/bin/lipo -archs "$binary")"
    [[ "$archs" = "arm64 x86_64" || "$archs" = "x86_64 arm64" ]]
  done
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"
  [[ "$(/usr/bin/codesign --display --verbose=4 "$app" 2>&1 | sed -n 's/^Authority=//p' | head -n 1)" = "$identity" ]]
  [[ "$(/usr/bin/codesign --display --verbose=4 "$app" 2>&1 | sed -n 's/^TeamIdentifier=//p')" = "$team_id" ]]
  /usr/sbin/spctl --assess --type execute --verbose=2 "$app"

  bundle_value() { /usr/bin/plutil -extract "$1" raw -o - "$app/Contents/Info.plist"; }
  /usr/bin/plutil -lint "$app/Contents/Info.plist"
  [[ "$(bundle_value CFBundleExecutable)" = "archigoat-shell" ]]
  [[ "$(bundle_value CFBundleIdentifier)" = "$ARCHIGOAT_BUNDLE_ID" ]]
  [[ "$(bundle_value CFBundleShortVersionString)" = "$version" ]]
  [[ "$(bundle_value CFBundleVersion)" = "$version" ]]
  [[ "$(bundle_value ArchiGoatCommit)" = "$commit" ]]

  local existing_health
  existing_health="$(/usr/bin/curl --silent --max-time 1 --output /dev/null --write-out '%{http_code}' http://127.0.0.1:17891/v1/health || true)"
  if [[ "$existing_health" != "000" ]]; then
    echo "loopback port 17891 is already occupied" >&2
    exit 1
  fi
  local health="$verify_work/health.json"
  local log="$verify_work/daemon.log"
  ARCHIGOAT_BIND=127.0.0.1:17891 ARCHIGOAT_KEEPALIVE=off ARCHIGOAT_STATE="$verify_work/state/archigoat.json" \
    "$app/Contents/MacOS/archigoat" --autostart >"$log" 2>&1 &
  daemon_pid=$!
  local healthy=false
  for _ in $(seq 1 120); do
    if /usr/bin/curl --fail --silent --show-error "http://127.0.0.1:17891/v1/health" -o "$health"; then
      healthy=true
      break
    fi
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
      cat "$log" >&2
      exit 1
    fi
    sleep 0.25
  done
  if [[ "$healthy" != true ]]; then
    cat "$log" >&2
    exit 1
  fi
  node - "$health" "$version" <<'NODE'
const fs = require("node:fs");
const [file, version] = process.argv.slice(2);
const health = JSON.parse(fs.readFileSync(file, "utf8"));
if (typeof health.registered !== "boolean" || health.version !== version) {
  process.stderr.write("daemon health identity is invalid\n");
  process.exit(1);
}
NODE
  echo "verified mounted ArchiGoat.app and /v1/health"
)

if [[ "${1:-}" = "--verify" ]]; then
  : "${ARCHIGOAT_BUNDLE_ID:?ARCHIGOAT_BUNDLE_ID required}"
  verify_image "${2:?DMG required}" "${3:?version required}" "${4:?commit required}" "${5:?signing identity required}" "${6:?team id required}"
  exit 0
fi

: "${ARCHIGOAT_BUNDLE_ID:?ARCHIGOAT_BUNDLE_ID required}"
: "${ARCHIGOAT_URL_SCHEME:?ARCHIGOAT_URL_SCHEME required}"
: "${ARCHIGOAT_ASSET_STEM:?ARCHIGOAT_ASSET_STEM required}"

INPUT_DAEMON="${1:?ArchiGoat daemon required}"
INPUT_SHELL="${2:?ArchiGoat shell required}"
DAEMON="$(cd "$(dirname "$INPUT_DAEMON")" && pwd)/$(basename "$INPUT_DAEMON")"
SHELL="$(cd "$(dirname "$INPUT_SHELL")" && pwd)/$(basename "$INPUT_SHELL")"
ASSET="${3:?asset name required}"
VERSION="${4:?version required}"
SIGNING_MODE="${5:?signing mode required: adhoc or developer-id}"
COMMIT="${6:?release commit required}"
# Optional pre-staged bundle (ArchiGoat.icns, Info.plist) built by stage-app-macos.sh.
STAGE="${7:-}"
OUTPUT="dist/$ASSET"
WORK="$(mktemp -d)"

# Cleanup removes only isolated release state.
cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

[[ "$ASSET" = "$ARCHIGOAT_ASSET_STEM-macos.dmg" ]]
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
[[ "$COMMIT" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]
[[ "$ARCHIGOAT_URL_SCHEME" =~ ^[a-z][a-z0-9+.-]*$ ]]
test -x "$DAEMON" -a -x "$SHELL"
"$DAEMON" --verify-release "$VERSION" "$COMMIT"
"$SHELL" --verify-release "$VERSION" "$COMMIT"
case "$SIGNING_MODE" in
  adhoc) ;;
  developer-id)
    : "${MACOS_APP_IDENTITY:?MACOS_APP_IDENTITY required for developer-id signing}"
    : "${APPLE_TEAM_ID:?APPLE_TEAM_ID required for developer-id signing}"
    ;;
  *) echo "unknown signing mode: $SIGNING_MODE" >&2; exit 2 ;;
esac

for binary in "$DAEMON" "$SHELL"; do
  ARCHS="$(lipo -archs "$binary")"
  [[ "$ARCHS" = "arm64 x86_64" || "$ARCHS" = "x86_64 arm64" ]]
done

APP="$WORK/image/ArchiGoat.app"
SOURCE="$(cd "$(dirname "$0")" && pwd)"
ICON="$SOURCE/archigoat-icon.png"
ICONSET="$WORK/ArchiGoat.iconset"
test -f "$ICON"
install -d "$APP/Contents/MacOS" "$APP/Contents/Resources" dist
install -m 755 "$DAEMON" "$APP/Contents/MacOS/archigoat"
install -m 755 "$SHELL" "$APP/Contents/MacOS/archigoat-shell"
BUNDLE_SHELL="$APP/Contents/MacOS/archigoat-shell"
if [[ -n "$STAGE" ]]; then
  STAGE="$(cd "$STAGE" && pwd)"
  test -s "$STAGE/ArchiGoat.icns" -a -s "$STAGE/Info.plist"
  install -m 644 "$STAGE/Info.plist" "$APP/Contents/Info.plist"
  install -m 644 "$STAGE/ArchiGoat.icns" "$APP/Contents/Resources/ArchiGoat.icns"
else
  install -d "$ICONSET"
  install -m 644 "$SOURCE/Info.plist" "$APP/Contents/Info.plist"
  ICON_SIZES=(16 32 64 128 256 512 1024)
  for size in "${ICON_SIZES[@]}"; do
    sips -z "$size" "$size" "$ICON" --out "$ICONSET/$size.png" >/dev/null
  done
  node "$SOURCE/package-icns.mjs" "$ICONSET" "$APP/Contents/Resources/ArchiGoat.icns"
  plutil -replace CFBundleDisplayName -string "ArchiGoat" "$APP/Contents/Info.plist"
  plutil -replace CFBundleIdentifier -string "$ARCHIGOAT_BUNDLE_ID" "$APP/Contents/Info.plist"
  plutil -replace CFBundleURLTypes.0.CFBundleURLName -string "$ARCHIGOAT_BUNDLE_ID" "$APP/Contents/Info.plist"
  plutil -replace NSAppDataUsageDescription -string "ArchiGoat launches your selected Agent with its native permissions. Each Work starts in its own workspace." "$APP/Contents/Info.plist"
  plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
  plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
  plutil -replace ArchiGoatCommit -string "$COMMIT" "$APP/Contents/Info.plist"
fi

# Canonicalize even a pre-staged plist so LaunchServices receives one exact deep-link scheme.
plutil -replace CFBundleURLTypes.0.CFBundleURLSchemes -json '[]' "$APP/Contents/Info.plist"
plutil -insert CFBundleURLTypes.0.CFBundleURLSchemes.0 -string "$ARCHIGOAT_URL_SCHEME" "$APP/Contents/Info.plist"

# The bundle identity is re-proved on both paths so a stale stage artifact can never ship.
plutil -lint "$APP/Contents/Info.plist"
[[ "$(plutil -extract CFBundleExecutable raw -o - "$APP/Contents/Info.plist")" = "archigoat-shell" ]]
[[ "$(plutil -extract CFBundleShortVersionString raw -o - "$APP/Contents/Info.plist")" = "$VERSION" ]]
[[ "$(plutil -extract CFBundleVersion raw -o - "$APP/Contents/Info.plist")" = "$VERSION" ]]
[[ "$(plutil -extract ArchiGoatCommit raw -o - "$APP/Contents/Info.plist")" = "$COMMIT" ]]
[[ "$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist")" = "$ARCHIGOAT_BUNDLE_ID" ]]
[[ "$(plutil -extract CFBundleURLTypes.0.CFBundleURLSchemes json -o - "$APP/Contents/Info.plist")" = "[\"$ARCHIGOAT_URL_SCHEME\"]" ]]
test -x "$BUNDLE_SHELL"
SHELL_ARCHS="$(lipo -archs "$BUNDLE_SHELL")"
[[ "$SHELL_ARCHS" = "arm64 x86_64" || "$SHELL_ARCHS" = "x86_64 arm64" ]]

# Inside-out signing preserves the complete App seal.
if [[ "$SIGNING_MODE" = developer-id ]]; then
  codesign --force --options runtime --timestamp --sign "$MACOS_APP_IDENTITY" "$APP/Contents/MacOS/archigoat"
  codesign --force --options runtime --timestamp --sign "$MACOS_APP_IDENTITY" "$APP/Contents/MacOS/archigoat-shell"
  codesign --force --options runtime --entitlements "$SOURCE/archigoat.entitlements" --timestamp --sign "$MACOS_APP_IDENTITY" "$APP"
else
  codesign --force --sign - "$APP/Contents/MacOS/archigoat"
  codesign --force --sign - "$APP/Contents/MacOS/archigoat-shell"
  codesign --force --entitlements "$SOURCE/archigoat.entitlements" --sign - "$APP"
fi
codesign --verify --deep --strict --verbose=2 "$APP"
if [[ "$SIGNING_MODE" = developer-id ]]; then
  ACTUAL_TEAM_ID="$(codesign --display --verbose=4 "$APP" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
  [[ "$ACTUAL_TEAM_ID" = "$APPLE_TEAM_ID" ]]
fi

# The release workflow notarizes this signed container before sealing the public image.
ln -s /Applications "$WORK/image/Applications"
rm -f "$OUTPUT"
hdiutil create -fs HFS+ -format UDZO -volname "ArchiGoat" -srcfolder "$WORK/image" "$OUTPUT"
test -s "$OUTPUT"
/usr/bin/hdiutil verify "$OUTPUT"
