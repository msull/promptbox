#!/bin/sh
# Regenerates assets/PromptBox.icns from assets/PromptBox.svg using only
# tools that ship with macOS (qlmanage, sips, iconutil).
set -eu
cd "$(dirname "$0")/.."
WORK=$(mktemp -d)
qlmanage -t -s 1024 -o "$WORK" assets/PromptBox.svg >/dev/null 2>&1
SRC="$WORK/PromptBox.svg.png"
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
