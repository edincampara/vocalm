# Vocalm — Windows install-from-source (no .exe installers to click).
#
#   git clone https://github.com/edincampara/vocalm.git
#   cd vocalm
#   powershell -ExecutionPolicy Bypass -File .\install.ps1
#
# Installs build prerequisites via winget (Rust, CMake, VS Build Tools, Git if
# missing), builds Vocalm in release mode, and creates a Start Menu shortcut.
# Everything AI runs on-device; the Whisper model auto-downloads on first launch.
#
# NOTE: for the virtual audio device, install VB-CABLE once (free):
#   winget install --id VB-Audio.VBCable    (or from https://vb-audio.com/Cable/)
# If winget can't install it on your machine, Vocalm still runs — you just need
# some virtual audio cable for meeting apps to pick up the cleaned audio.

$ErrorActionPreference = "Stop"

function Ensure-Tool($cmd, $wingetId, $name) {
    if (Get-Command $cmd -ErrorAction SilentlyContinue) {
        Write-Host "[ok] $name found" -ForegroundColor Green
        return
    }
    Write-Host "[..] installing $name via winget" -ForegroundColor Yellow
    winget install --id $wingetId --silent --accept-package-agreements --accept-source-agreements
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "Machine") + ";" +
                [System.Environment]::GetEnvironmentVariable("Path", "User")
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        Write-Host "[!!] $name still not on PATH — open a new terminal and re-run this script." -ForegroundColor Red
        exit 1
    }
}

Write-Host "== Vocalm installer ==" -ForegroundColor Cyan

Ensure-Tool "git"   "Git.Git"          "Git"
Ensure-Tool "cargo" "Rustlang.Rustup"  "Rust (rustup)"
Ensure-Tool "cmake" "Kitware.CMake"    "CMake"

# MSVC linker (needed by Rust + whisper.cpp). Detect via cl.exe or vswhere.
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasMsvc = (Get-Command cl -ErrorAction SilentlyContinue) -or
           ((Test-Path $vswhere) -and (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath))
if (-not $hasMsvc) {
    Write-Host "[..] installing Visual Studio Build Tools (C++ workload) — this is the big one" -ForegroundColor Yellow
    winget install --id Microsoft.VisualStudio.2022.BuildTools --silent `
        --accept-package-agreements --accept-source-agreements `
        --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
}

rustup default stable | Out-Null

Write-Host "[..] building Vocalm (first build takes a few minutes)" -ForegroundColor Yellow
cargo build --release --bin vocalm
if ($LASTEXITCODE -ne 0) { Write-Host "[!!] build failed" -ForegroundColor Red; exit 1 }

$exe = Join-Path $PSScriptRoot "target\release\vocalm.exe"
Write-Host "[ok] built: $exe" -ForegroundColor Green

# Start Menu shortcut (per-user, no admin needed)
$startMenu = [Environment]::GetFolderPath("StartMenu")
$lnk = Join-Path $startMenu "Programs\Vocalm.lnk"
$ws = New-Object -ComObject WScript.Shell
$sc = $ws.CreateShortcut($lnk)
$sc.TargetPath = $exe
$sc.WorkingDirectory = Split-Path $exe
$sc.Description = "Vocalm — AI noise cancellation"
$sc.Save()
Write-Host "[ok] Start Menu shortcut created" -ForegroundColor Green

Write-Host ""
Write-Host "Done! Launching Vocalm..." -ForegroundColor Cyan
Write-Host "Reminder: install VB-CABLE for the virtual microphone if you haven't:"
Write-Host "  winget install --id VB-Audio.VBCable"
Write-Host "Then in Teams/Meet: microphone = 'CABLE Output', speaker = second cable."
Start-Process $exe
