#!/bin/sh
# Regenerates assets/PromptBox.icns from assets/PromptBox.svg using only
# tools that ship with macOS (swift + AppKit for a transparent render,
# sips, iconutil). Quick Look's thumbnailer paints a white background, so
# it is not used.
set -eu
cd "$(dirname "$0")/.."
WORK=$(mktemp -d)
SRC="$WORK/PromptBox.png"
swift scripts/render-icon.swift assets/PromptBox.svg "$SRC" 1024
SET="$WORK/PromptBox.iconset"
mkdir -p "$SET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$SRC" --out "$SET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" "$SRC" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o assets/PromptBox.icns
rm -rf "$WORK"
echo "Wrote assets/PromptBox.icns"
