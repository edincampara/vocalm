#!/bin/bash
# Build Vocalm.app and a distributable DMG.
# Usage: scripts/package_mac.sh
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP=dist/Vocalm.app
DMG=dist/Vocalm-$VERSION.dmg

echo "==> building release binary"
cargo build --release --bin vocalm

echo "==> assembling $APP"
rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/vocalm "$APP/Contents/MacOS/Vocalm"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Vocalm</string>
    <key>CFBundleDisplayName</key><string>Vocalm</string>
    <key>CFBundleIdentifier</key><string>com.vocalm.app</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key><string>Vocalm</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>Vocalm</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Vocalm removes background noise from your microphone in real time.</string>
</dict>
</plist>
PLIST

# Icon (optional): generate a simple one from an emoji-ish SF-symbol-free path if
# an icns doesn't exist yet. Skipped silently when iconutil inputs are missing.
if [ -f assets/Vocalm.icns ]; then
    cp assets/Vocalm.icns "$APP/Contents/Resources/Vocalm.icns"
fi

echo "==> ad-hoc code signing"
codesign --force --deep -s - "$APP"

echo "==> building DMG"
STAGE=dist/dmg-stage
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Vocalm" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"

echo "==> done: $DMG"
echo "    (ad-hoc signed: first launch needs right-click → Open, or"
echo "     System Settings → Privacy & Security → Open Anyway)"
