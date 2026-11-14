#!/usr/bin/env bash
# Builds a menu-bar-only Frosty.app (no Dock icon, no Terminal window).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
APP="dist/Frosty.app"
TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin")
SIGNING_IDENTITY="${SIGNING_IDENTITY:--}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"

if [ "$SIGNING_IDENTITY" = "-" ] && [ -n "$NOTARY_PROFILE" ]; then
    echo "NOTARY_PROFILE requires a Developer ID signing identity." >&2
    exit 1
fi

for target in "${TARGETS[@]}"; do
    if ! rustup target list --installed | grep -Fxq "$target"; then
        echo "Missing Rust target: $target" >&2
        echo "Install it with: rustup target add $target" >&2
        exit 1
    fi
done

./scripts/make-icns.sh "$ROOT/app/assets/app-icon.png"

for target in "${TARGETS[@]}"; do
    cargo build --release -p winter-app --target "$target"
done

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create \
    "target/aarch64-apple-darwin/release/frosty" \
    "target/x86_64-apple-darwin/release/frosty" \
    -output "$APP/Contents/MacOS/frosty"
cp macos/Info.plist "$APP/Contents/Info.plist"
cp macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

if [ "$SIGNING_IDENTITY" = "-" ]; then
    codesign --force --deep --sign - "$APP"
    echo "Ad-hoc signed. This build is not notarized and may be blocked by Gatekeeper on other Macs."
else
    codesign --force --deep --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" "$APP"
    if [ -n "$NOTARY_PROFILE" ]; then
        NOTARY_TMP="$(mktemp -d)"
        trap 'rm -rf "$NOTARY_TMP"' EXIT
        ditto -c -k --keepParent "$APP" "$NOTARY_TMP/Frosty.zip"
        xcrun notarytool submit "$NOTARY_TMP/Frosty.zip" \
            --keychain-profile "$NOTARY_PROFILE" --wait
        xcrun stapler staple "$APP"
    else
        echo "Signed with Developer ID but not notarized; set NOTARY_PROFILE to notarize."
    fi
fi

codesign --verify --deep --strict --verbose=2 "$APP"
lipo -info "$APP/Contents/MacOS/frosty"

echo "Done: $APP"
echo "Run:  open $APP"
echo "Install: cp -R $APP /Applications/"
