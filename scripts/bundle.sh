#!/bin/sh
# Builds the release binary and wraps it in "Prompt Box.app", installed to
# ~/Applications (or the directory given as the first argument). The bundle
# is ad-hoc signed so macOS keeps its microphone / Accessibility grants
# across rebuilds.
set -eu
cd "$(dirname "$0")/.."
DEST="${1:-$HOME/Applications}"
APP="$DEST/Prompt Box.app"
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

cargo build --locked --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/promptbox "$APP/Contents/MacOS/promptbox"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>            <string>Prompt Box</string>
  <key>CFBundleDisplayName</key>     <string>Prompt Box</string>
  <key>CFBundleIdentifier</key>      <string>com.sadburger.promptbox</string>
  <key>CFBundleVersion</key>         <string>$VERSION</string>
  <key>CFBundleShortVersionString</key> <string>$VERSION</string>
  <key>CFBundleExecutable</key>      <string>promptbox</string>
  <key>CFBundlePackageType</key>     <string>APPL</string>
  <key>LSMinimumSystemVersion</key>  <string>13.0</string>
  <key>NSHighResolutionCapable</key> <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Prompt Box listens to your voice to dictate prompts.</string>
</dict>
</plist>
PLIST
if [ -f assets/PromptBox.icns ]; then
  cp assets/PromptBox.icns "$APP/Contents/Resources/PromptBox.icns"
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string PromptBox" "$APP/Contents/Info.plist"
fi
codesign --force --deep --sign - "$APP"
echo "Installed $APP"
