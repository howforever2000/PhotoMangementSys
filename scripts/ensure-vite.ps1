# Ensure a Vite dev server is listening on port 1420 before Tauri starts.
# Reuses an existing Vite (e.g. left over from a previous run) instead of
# letting `npm run dev` crash with "Port 1420 is already in use".
#
# Port detection logic mirrors the Test-Port helper in scripts/dev-outside.ps1.
# This file is deliberately ASCII-only: no BOM needed, so it parses correctly
# under both Windows PowerShell 5.1 (GBK default) and PowerShell 7.
#
# Wired in as tauri.conf.json -> build.beforeDevCommand.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# beforeDevCommand no longer goes through `npm run dev`, so npm's `predev`
# hook (fix-workspace-integrity.ps1) would be skipped. Call it explicitly.
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "fix-workspace-integrity.ps1")

function Test-Port([int]$port) {
    return [bool](Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
        Where-Object { $_.LocalPort -eq $port })
}

if (Test-Port 1420) {
    Write-Host "[ensure-vite] Vite already listening on 1420 - reusing it."
    exit 0
}

Write-Host "[ensure-vite] No listener on 1420, starting Vite ..." -ForegroundColor Cyan
Start-Process -FilePath "cmd.exe" `
    -ArgumentList "/c title vite-1420 && npx vite --port 1420 --strictPort" `
    -WorkingDirectory $root

for ($i = 0; $i -lt 30 -and -not (Test-Port 1420); $i++) {
    Start-Sleep -Milliseconds 500
}
if (-not (Test-Port 1420)) {
    Write-Host "[ensure-vite] FAILED: Vite did not bind port 1420 within 15s." -ForegroundColor Red
    exit 1
}
Write-Host "[ensure-vite] Vite ready on 1420." -ForegroundColor Green
exit 0
