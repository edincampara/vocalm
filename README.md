# Vocalm

**Calm your calls.** Free, on-device, real-time AI noise cancellation and meeting
recorder for macOS and Windows — a [Krisp](https://krisp.ai)-style virtual microphone
with no cloud, no subscription, and no audio ever leaving your machine.

```
your mic ──► Vocalm (AI denoise) ──► virtual device ──► Zoom / Teams / Meet / Discord
their audio ◄── real speakers ◄── Vocalm (AI denoise) ◄── virtual device ◄── meeting app
```

Both directions are cleaned: your voice going out, and the noise coming in from
everyone else's microphones.

## Features

- **Two-way noise cancellation** — DeepFilterNet3 (Krisp-class quality, ~40 ms) or
  RNNoise (~10 ms, ultra-light), switchable live. Adjustable suppression strength.
- **Meeting recording** — one click; when incoming cleanup is on, recordings are
  stereo: you on the left channel, everyone else on the right (diarization-ready).
- **On-device transcription** — Whisper (whisper.cpp, Metal-accelerated), timestamped
  transcripts, auto language detection. Model (~148 MB) downloads once.
- **Meetings library** — rename, tag participants, reveal in Finder, view transcripts.
- **Simple by default, tweakable when wanted** — one master toggle for non-technical
  users; engines, routing, and DSP stats live under Advanced.

## Setup

### macOS — one installer, exactly like Krisp

Run `scripts/build_installer.sh` and open the resulting
`dist/Vocalm-<version>-Installer.dmg`. The pkg inside installs **Vocalm.app plus its
own virtual audio devices** — "Vocalm Microphone" and "Vocalm Speaker" — and restarts
CoreAudio (asks for your password once). No BlackHole, no extra downloads.

Then:
- In your meeting app: microphone → **Vocalm Microphone**, speaker → **Vocalm Speaker**
- In Vocalm: pick your real mic and your real speakers. Done.

The devices are rebranded builds of the GPL-3 BlackHole driver (see
`drivers/NOTICE`); Vocalm also auto-detects plain BlackHole or VB-CABLE if you
already use those.

### Windows — install from source with one shell command

No .exe installers needed — everything builds from source:

```powershell
git clone https://github.com/edincampara/vocalm.git
cd vocalm
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

The script installs prerequisites via winget (Rust, CMake, VS Build Tools), builds
Vocalm, creates a Start Menu shortcut, and launches it. The Whisper model
auto-downloads on first launch; the noise-removal models are embedded in the binary.

For the virtual microphone, install the free VB-CABLE once:
`winget install --id VB-Audio.VBCable` — then in Teams/Meet set microphone =
`CABLE Output`. (Windows can't have branded "Vocalm" devices yet: virtual audio
there means a kernel driver that must be Microsoft-signed, which requires an EV
certificate — money. The app itself is fully free and works today with VB-CABLE.)

### Updating (any platform)

Updates ship through GitHub — from the repo folder:

```powershell
# Windows
powershell -ExecutionPolicy Bypass -File .\update.ps1
```

```sh
# macOS
git pull --ff-only && scripts/build_installer.sh   # rebuilds app + installer DMG
```

Settings, recordings, and models live outside the repo, so updating never touches
them. Every push to `main` is build-verified on Windows and macOS by GitHub Actions
(including a DeepFilterNet3/RNNoise smoke test); tagging `v*` publishes prebuilt
binaries as a GitHub Release.

## Building

Rust 1.70+ and CMake (`brew install rustup cmake`, or rustup.rs + CMake on Windows).

```sh
cargo build --release          # → target/release/vocalm
scripts/package_mac.sh         # → dist/Vocalm.app + dist/Vocalm-<version>.dmg
```

The DMG is ad-hoc signed (no paid Apple developer account): first launch is
right-click → Open. Notarization would need an Apple Developer ID ($99/yr).

Windows builds use the same code (WASAPI via cpal): `cargo build --release` with the
MSVC toolchain.

## Verifying quality offline

```sh
cargo run --release --bin denoise-wav -- noisy.wav clean.wav df       # DeepFilterNet3
cargo run --release --bin denoise-wav -- noisy.wav clean.wav rnnoise  # RNNoise
vocalm --transcribe <meeting-folder>                                  # headless transcription
```

## How it works

1. `cpal` captures audio (CoreAudio / WASAPI), downmixed to mono.
2. Audio is resampled to 48 kHz (rubato) and chopped into 10 ms hops.
3. Each hop runs through the neural engine (DeepFilterNet3 via `tract` ONNX inference,
   or RNNoise via `nnnoiseless`) on a dedicated DSP thread — lock-free ring buffers on
   both sides, so audio callbacks never block. Measured: 0.37 ms per 10 ms frame.
4. The clean signal goes to a virtual loopback device (outgoing) or your real
   speakers (incoming). A second identical pipeline handles the reverse direction.

Same architecture as Krisp (virtual device + on-device DNN); Vocalm uses open models
and standard free loopback drivers instead of a proprietary model + signed driver.

## Roadmap

- Speaker diarization ("who said what") via an on-device model (e.g. sherpa-onnx);
  stereo recordings already separate you from everyone else.
- Google / Outlook calendar integration (auto-name and auto-record meetings).
- Menu-bar / tray mode, start at login; notarized installers bundling the drivers.
