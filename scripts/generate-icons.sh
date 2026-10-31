#!/usr/bin/env bash
#
# Generate all icon variants from the single source icon (icon.png in project root).
# Requires: sips + iconutil (macOS) or ImageMagick (convert) on Linux.
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/icon.png"

if [ ! -f "$SRC" ]; then
  echo "Error: source icon not found at $SRC" >&2
  exit 1
fi

# Create a square padded version of the source icon for consistent resizing.
# This avoids distortion and ensures iconutil/ico generation works correctly.
SQUARE_SRC=$(mktemp /tmp/splitty-icon-square-XXXXXX.png)
trap 'rm -f "$SQUARE_SRC"' EXIT

make_square() {
  if command -v magick &>/dev/null; then
    magick "$SRC" -gravity center -background none -extent "%[fx:max(w,h)]x%[fx:max(w,h)]" "$SQUARE_SRC"
  elif command -v convert &>/dev/null; then
    convert "$SRC" -gravity center -background none -extent "$(identify -format '%[fx:max(w,h)]x%[fx:max(w,h)]' "$SRC")" "$SQUARE_SRC"
  elif command -v sips &>/dev/null; then
    # sips can't pad, so just use the source as-is (may produce non-square icons)
    cp "$SRC" "$SQUARE_SRC"
  else
    echo "Error: need ImageMagick (magick/convert) or sips" >&2
    exit 1
  fi
}
make_square

resize() {
  local size="$1" out="$2"
  mkdir -p "$(dirname "$out")"
  if command -v magick &>/dev/null; then
    magick "$SQUARE_SRC" -resize "${size}x${size}" "$out"
  elif command -v convert &>/dev/null; then
    convert "$SQUARE_SRC" -resize "${size}x${size}" "$out"
  elif command -v sips &>/dev/null; then
    cp "$SQUARE_SRC" "$out"
    sips -Z "$size" "$out" --out "$out" &>/dev/null
  else
    echo "Error: need ImageMagick (magick/convert) or sips" >&2
    exit 1
  fi
  echo "  ${size}px → $out"
}

generate_icns() {
  local out="$1"
  mkdir -p "$(dirname "$out")"
  local iconset
  iconset=$(mktemp -d)/splitty.iconset
  mkdir -p "$iconset"

  for size in 16 32 128 256 512; do
    resize "$size" "$iconset/icon_${size}x${size}.png"
    local retina=$((size * 2))
    if [ "$retina" -le 1024 ]; then
      resize "$retina" "$iconset/icon_${size}x${size}@2x.png"
    fi
  done

  if command -v iconutil &>/dev/null; then
    iconutil -c icns "$iconset" -o "$out"
    echo "  icns → $out"
  else
    echo "  ⚠ iconutil not available, skipping $out"
  fi
  rm -rf "$(dirname "$iconset")"
}

generate_ico() {
  local out="$1"
  mkdir -p "$(dirname "$out")"
  if command -v magick &>/dev/null; then
    magick "$SQUARE_SRC" \
      \( -clone 0 -resize 16x16 \) \
      \( -clone 0 -resize 32x32 \) \
      \( -clone 0 -resize 48x48 \) \
      \( -clone 0 -resize 64x64 \) \
      \( -clone 0 -resize 128x128 \) \
      \( -clone 0 -resize 256x256 \) \
      -delete 0 "$out"
    echo "  ico → $out"
  elif command -v convert &>/dev/null; then
    convert "$SQUARE_SRC" \
      \( -clone 0 -resize 16x16 \) \
      \( -clone 0 -resize 32x32 \) \
      \( -clone 0 -resize 48x48 \) \
      \( -clone 0 -resize 64x64 \) \
      \( -clone 0 -resize 128x128 \) \
      \( -clone 0 -resize 256x256 \) \
      -delete 0 "$out"
    echo "  ico → $out"
  else
    echo "  ⚠ ImageMagick not available, skipping $out"
  fi
}

echo "Generating icons from $SRC ..."

# ── Tauri icons ──
TAURI_ICONS="$ROOT/crates/splitty-tauri/icons"
resize 32  "$TAURI_ICONS/32x32.png"
resize 128 "$TAURI_ICONS/128x128.png"
resize 256 "$TAURI_ICONS/128x128@2x.png"
resize 512 "$TAURI_ICONS/icon.png"
generate_icns "$TAURI_ICONS/icon.icns"
generate_ico  "$TAURI_ICONS/icon.ico"

# ── macOS icon (project root) ──
generate_icns "$ROOT/icon-macos.icns"

# ── AppImage / Electron source icon (1024px) ──
resize 1024 "$ROOT/packaging/linux/splitty.png"

# ── Electron ──
resize 512 "$ROOT/packaging/electron/icon.png"

# ── Frontend (React) — favicon + PWA icons ──
resize 48  "$ROOT/frontend/public/favicon.png"
resize 192 "$ROOT/frontend/public/icon-192.png"
resize 512 "$ROOT/frontend/public/icon-512.png"

# ── Frontend asset (used in UI) ──
resize 256 "$ROOT/frontend/src/assets/splitty.png"

# ── Frontend (Svelte) — favicon + PWA icons ──
if [ -d "$ROOT/frontend-svelte/public" ]; then
  resize 48  "$ROOT/frontend-svelte/public/favicon.png"
  resize 192 "$ROOT/frontend-svelte/public/icon-192.png"
  resize 512 "$ROOT/frontend-svelte/public/icon-512.png"
fi

echo "Done."
