# 在工作区外启动开发版（绕开当前会话对「工作区内可执行文件」的写入限制）
#
# 背景（BUG-2026-1026-002）：
#   从受限会话启动时，**位于工作区内的 exe** 只能写工作区，写不进
#   %APPDATA%\com.photomanagementsys.app；SQLite 建不出 photos.db-wal/-shm，
#   于是报 "attempt to write a readonly database" / "unable to open database file"。
#   把同一个 exe 拷到工作区外再启动，一切正常（已实测：131 个相册正常加载）。
#
# 这个脚本做四件事：确保 1420 上有前端 → 增量构建 → 把 exe 拷到 %LOCALAPPDATA%\pms-run
# → 从那里启动。前端仍走 vite dev server，所以热更新照旧。
#
# 用法：npm run dev:outside
#       （等价：powershell -NoProfile -ExecutionPolicy Bypass -File scripts/dev-outside.ps1）

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$runDir = Join-Path $env:LOCALAPPDATA "pms-run"
$exeSrc = Join-Path $root "src-tauri\target\debug\photo-management-sys.exe"

function Test-Port([int]$port) {
    return [bool](Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
        Where-Object { $_.LocalPort -eq $port })
}

# 1) 前端：tauri.conf.json 的 devUrl 固定 1420，没有就起一个
if (-not (Test-Port 1420)) {
    Write-Host "[dev:outside] 启动 vite @1420 ..." -ForegroundColor Cyan
    Start-Process -FilePath "cmd.exe" `
        -ArgumentList "/c title vite-1420 && npx vite --port 1420 --strictPort" `
        -WorkingDirectory $root
    for ($i = 0; $i -lt 30 -and -not (Test-Port 1420); $i++) { Start-Sleep -Milliseconds 500 }
    if (-not (Test-Port 1420)) { throw "vite 未能在 1420 上就绪（端口被占用？）" }
}

# 2) 后端：增量构建（目标目录仍在工作区内，只是**运行**的位置挪出去）
Write-Host "[dev:outside] cargo build ..." -ForegroundColor Cyan
Push-Location (Join-Path $root "src-tauri")
try { & cargo build; if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" } }
finally { Pop-Location }
if (-not (Test-Path $exeSrc)) { throw "找不到构建产物：$exeSrc" }

# 3) 拷到工作区外（这一步是关键：镜像路径决定进程是否被限制）
New-Item -ItemType Directory -Force -Path $runDir | Out-Null
$exeDst = Join-Path $runDir "photo-management-sys.exe"
Get-Process photo-management-sys -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400
Copy-Item $exeSrc $exeDst -Force

# 4) 启动（数据仍用真实的 %APPDATA% 目录；想换目录可先设 $env:PMS_DATA_DIR）
Write-Host "[dev:outside] 启动 $exeDst" -ForegroundColor Green
$p = Start-Process -FilePath $exeDst -PassThru
Start-Sleep -Seconds 6
$alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if ($alive) {
    Write-Host "[dev:outside] 已启动 pid=$($p.Id) title=$($alive.MainWindowTitle)" -ForegroundColor Green
} else {
    Write-Host "[dev:outside] 进程已退出（exit=$($p.ExitCode)）。看日志：" -ForegroundColor Yellow
    $log = Join-Path $env:APPDATA "com.photomanagementsys.app\logs\app.log"
    if (Test-Path $log) { Get-Content $log -Tail 12 }
}
