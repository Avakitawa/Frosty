#!/usr/bin/env bash
# Usage: ./scripts/make-icns.sh path/to/icon.png
# Converts a PNG into macos/AppIcon.icns.
set -euo pipefail
cd "$(dirname "$0")/.."
SRC="${1:?Usage: $0 icon-1024.png}"
if [ ! -f "$SRC" ]; then
    echo "Icon file not found: $SRC" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
SET="$TMP_DIR/AppIcon.iconset"
mkdir -p "$SET"
WIDTH="$(sips -g pixelWidth "$SRC" | awk '/pixelWidth/ { print $2 }')"
HEIGHT="$(sips -g pixelHeight "$SRC" | awk '/pixelHeight/ { print $2 }')"
CROP_SIZE="$WIDTH"
if [ "$HEIGHT" -lt "$CROP_SIZE" ]; then
    CROP_SIZE="$HEIGHT"
fi
sips -c "$CROP_SIZE" "$CROP_SIZE" "$SRC" --out "$TMP_DIR/square.png" >/dev/null

for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$TMP_DIR/square.png" --out "$SET/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$TMP_DIR/square.png" --out "$SET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o macos/AppIcon.icns
echo "Created macos/AppIcon.icns"
