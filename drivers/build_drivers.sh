#!/bin/bash
# Build the Vocalm virtual audio drivers (rebranded BlackHole, GPL-3 — see NOTICE).
# Produces: drivers/build/VocalmMic.driver and drivers/build/VocalmSpeaker.driver
#
# "Vocalm Microphone": the app renders cleaned mic audio into it; meeting apps
#                      select it as their microphone.
# "Vocalm Speaker":    meeting apps select it as their speaker; the app captures
#                      it, denoises, and plays to the real output device.
set -euo pipefail

cd "$(dirname "$0")"
SRC=${BLACKHOLE_SRC:-build/BlackHole-src}
mkdir -p build

if [ ! -d "$SRC" ]; then
    git clone --depth 1 https://github.com/ExistentialAudio/BlackHole.git "$SRC"
fi

build_driver() {
    local product="$1" device_name="$2" bundle_id="$3"
    echo "==> building $product (\"$device_name\")"
    local work="build/src-$product"
    rm -rf "$work"
    cp -R "$SRC" "$work"
    local c="$work/BlackHole/BlackHole.c"
    # Patch the #ifndef defaults: fixed device name (no "%ich" suffix), our IDs.
    sed -i '' \
        -e "s|kDriver_Name  *\"BlackHole\"|kDriver_Name \"$product\"|" \
        -e "s|kPlugIn_BundleID  *\"audio.existential.BlackHole2ch\"|kPlugIn_BundleID \"$bundle_id\"|" \
        -e "s|kHas_Driver_Name_Format  *true|kHas_Driver_Name_Format false|" \
        -e "s|kDevice_Name  *kDriver_Name \" \"|kDevice_Name \"$device_name\"|" \
        "$c"
    grep -q "\"$device_name\"" "$c" || { echo "patch failed"; exit 1; }

    xcodebuild \
        -project "$work/BlackHole.xcodeproj" \
        -scheme BlackHole \
        -configuration Release \
        -derivedDataPath "build/dd-$product" \
        PRODUCT_BUNDLE_IDENTIFIER="$bundle_id" \
        PRODUCT_NAME="$product" \
        CODE_SIGN_IDENTITY="-" \
        build >"build/$product.log" 2>&1
    rm -rf "build/$product.driver"
    cp -R "build/dd-$product/Build/Products/Release/$product.driver" "build/"
    echo "    ok: build/$product.driver"
}

build_driver "VocalmMic" "Vocalm Microphone" "ai.vocalm.driver.mic"
build_driver "VocalmSpeaker" "Vocalm Speaker" "ai.vocalm.driver.speaker"

echo
echo "To install locally:"
echo "  sudo cp -R build/VocalmMic.driver build/VocalmSpeaker.driver /Library/Audio/Plug-Ins/HAL/"
echo "  sudo killall -9 coreaudiod"
