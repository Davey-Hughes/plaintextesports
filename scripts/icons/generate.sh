#!/usr/bin/env bash
# Rasterize the pte icon set into the runtime drop-in directory (default ./icons).
# Requires: rsvg-convert (librsvg), optipng, icotool (icoutils). Output is gitignored.
# Note: pte.svg renders "pte" with the generic `monospace` family, so the exact
# glyph shapes (and thus 16px legibility) depend on whichever monospace font
# fontconfig resolves on the build host. Output is not byte-reproducible across
# machines; eyeball icon-512.png / a 32px render after generating on a new host.
set -euo pipefail

SRC="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SRC/../.." && pwd)"
OUT="${1:-$ROOT/icons}"
mkdir -p "$OUT"

# PWA / apple-touch rasters from the standard mark.
rsvg-convert -w 512 -h 512 "$SRC/pte.svg" -o "$OUT/icon-512.png"
rsvg-convert -w 192 -h 192 "$SRC/pte.svg" -o "$OUT/icon-192.png"
rsvg-convert -w 180 -h 180 "$SRC/pte.svg" -o "$OUT/apple-touch-icon.png"
# Maskable (Android) from the safe-zone mark.
rsvg-convert -w 512 -h 512 "$SRC/pte-maskable.svg" -o "$OUT/icon-512-maskable.png"
optipng -quiet -o2 "$OUT/icon-512.png" "$OUT/icon-192.png" \
  "$OUT/apple-touch-icon.png" "$OUT/icon-512-maskable.png"

# Multi-size favicon.ico (16/32/48) via icotool.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT  # clean up even if rsvg/icotool fails under `set -e`
for s in 16 32 48; do
  rsvg-convert -w "$s" -h "$s" "$SRC/pte.svg" -o "$tmp/favicon-$s.png"
done
icotool -c -o "$OUT/favicon.ico" \
  "$tmp/favicon-16.png" "$tmp/favicon-32.png" "$tmp/favicon-48.png"

# SVG favicon is the master, served as-is.
cp "$SRC/pte.svg" "$OUT/favicon.svg"

# Web app manifest.
cat > "$OUT/manifest.webmanifest" <<'JSON'
{
  "name": "plaintextesports",
  "short_name": "pte",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "background_color": "#0d0d0d",
  "theme_color": "#0d0d0d",
  "icons": [
    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png" },
    { "src": "/icon-512-maskable.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
  ]
}
JSON

echo "icons written to $OUT"
