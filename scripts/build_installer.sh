#!/bin/bash
# Build the full Vocalm installer for macOS:
#   dist/Vocalm-<version>-Installer.dmg  containing  Vocalm Installer.pkg
# The pkg installs Vocalm.app AND the two Vocalm virtual audio devices, then
# restarts CoreAudio — after install, "Vocalm Microphone" / "Vocalm Speaker"
# simply exist system-wide, exactly like Krisp's devices.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

echo "==> building release binary"
cargo build --release --bin vocalm

echo "==> building drivers (if missing)"
if [ ! -d drivers/build/VocalmMic.driver ] || [ ! -d drivers/build/VocalmSpeaker.driver ]; then
    drivers/build_drivers.sh
fi

echo "==> assembling Vocalm.app"
rm -rf dist
APP=dist/root/Applications/Vocalm.app
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
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Vocalm removes background noise from your microphone in real time.</string>
</dict>
</plist>
PLIST
codesign --force --deep -s - "$APP"

echo "==> staging drivers"
mkdir -p "dist/root/Library/Audio/Plug-Ins/HAL"
cp -R drivers/build/VocalmMic.driver drivers/build/VocalmSpeaker.driver \
      "dist/root/Library/Audio/Plug-Ins/HAL/"

echo "==> building pkg"
mkdir -p dist/pkg-scripts
cat > dist/pkg-scripts/postinstall <<'POST'
#!/bin/bash
# Load the new audio devices
killall -9 coreaudiod 2>/dev/null || true
exit 0
POST
chmod +x dist/pkg-scripts/postinstall

pkgbuild \
    --root dist/root \
    --scripts dist/pkg-scripts \
    --identifier ai.vocalm.installer \
    --version "$VERSION" \
    --install-location / \
    "dist/Vocalm Installer.pkg" >/dev/null

echo "==> building DMG"
STAGE=dist/dmg-stage
mkdir -p "$STAGE"
cp "dist/Vocalm Installer.pkg" "$STAGE/"
hdiutil create -volname "Vocalm" -srcfolder "$STAGE" -ov -format UDZO \
    "dist/Vocalm-$VERSION-Installer.dmg" >/dev/null
rm -rf "$STAGE" dist/root dist/pkg-scripts

echo "==> done: dist/Vocalm-$VERSION-Installer.dmg"
echo "    Double-click the pkg inside; it asks for your password (drivers install"
echo "    into /Library/Audio/Plug-Ins/HAL) and restarts CoreAudio."
echo "    Unsigned pkg: right-click → Open on first run."
