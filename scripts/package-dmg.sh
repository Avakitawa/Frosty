#!/usr/bin/env bash
# Builds a drag-to-Applications Frosty DMG from an already packaged app.
# Usage: SIGNING_IDENTITY="Developer ID Application: ..." ./scripts/package-dmg.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP="${APP:-dist/Frosty.app}"
IDENTITY="${SIGNING_IDENTITY:--}"
STAGE="$(mktemp -d)/Frosty"
IMAGE="$(mktemp -d)/Frosty-rw.dmg"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
DMG="${DMG:-dist/Frosty-$VERSION.dmg}"

if [ ! -d "$APP" ]; then
    echo "App not found: $APP" >&2
    echo "Run ./scripts/package.sh first." >&2
    exit 1
fi

cleanup() {
    if [ -d "/Volumes/Frosty" ]; then
        hdiutil detach "/Volumes/Frosty" -force >/dev/null 2>&1 || true
    fi
    rm -rf "$STAGE" "$(dirname "$IMAGE")"
}
trap cleanup EXIT

mkdir -p "$STAGE/.background" "$(dirname "$DMG")"
ditto "$APP" "$STAGE/Frosty.app"
ln -s /Applications "$STAGE/Applications"
cp macos/dmg-background.png "$STAGE/.background/background.png"

hdiutil create -volname "Frosty" -srcfolder "$STAGE" -ov -format UDRW "$IMAGE" >/dev/null
if [ -d "/Volumes/Frosty" ]; then
    hdiutil detach "/Volumes/Frosty" -force >/dev/null 2>&1 || true
fi
hdiutil attach "$IMAGE" -readwrite -noverify -noautoopen >/dev/null
sleep 2

osascript <<'APPLESCRIPT'
tell application "Finder"
    tell disk "Frosty"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 200, 1000, 700}
        set theViewOptions to the icon view options of container window
        set arrangement of theViewOptions to not arranged
        set icon size of theViewOptions to 128
        set text size of theViewOptions to 13
        set background picture of theViewOptions to file ".background:background.png"
        set position of item "Frosty.app" of container window to {220, 250}
        set position of item "Applications" of container window to {580, 250}
        close
        open
        update without registering applications
        delay 2
        close
    end tell
end tell
APPLESCRIPT

sync
hdiutil detach "/Volumes/Frosty" >/dev/null
hdiutil convert "$IMAGE" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null

if [ "$IDENTITY" = "-" ]; then
    codesign --force --sign - "$DMG"
    echo "Ad-hoc signed DMG. Gatekeeper will warn on other Macs."
else
    codesign --force --sign "$IDENTITY" --timestamp "$DMG"
    echo "Signed DMG with: $IDENTITY"
fi

codesign --verify --verbose=2 "$DMG"
hdiutil verify "$DMG" >/dev/null
echo "Done: $DMG"
