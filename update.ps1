# Vocalm — one-command update on Windows:
#   powershell -ExecutionPolicy Bypass -File .\update.ps1
# Pulls the latest code from GitHub and rebuilds. Settings, recordings and
# models are stored outside the repo, so updates never touch them.
$ErrorActionPreference = "Stop"

Write-Host "== Updating Vocalm ==" -ForegroundColor Cyan
git pull --ff-only
cargo build --release --bin vocalm
if ($LASTEXITCODE -ne 0) { Write-Host "[!!] build failed" -ForegroundColor Red; exit 1 }
Write-Host "[ok] updated. Restart Vocalm to use the new version." -ForegroundColor Green
