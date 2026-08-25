# ============================================================================
# 构建 VCR 视觉识别微服务单文件 exe（PyInstaller，最小化环境）
# 产物：python/dist/vcr-server.exe（含 Python 运行时 + 全部依赖，约 80~90MB）
#
# 为什么用最小化 venv：
#   全局 base（Anaconda）含大量无关包，会导致 exe 巨大且启动慢。
#   本脚本创建 .venv-vcr，仅安装推理运行时依赖，再据此打包。
#
# 用法（在项目根目录，或任意目录执行均可）：
#   powershell -ExecutionPolicy Bypass -File python/build_vcr_exe.ps1 [-Mirror https://pypi.tuna.tsinghua.edu.cn/simple]
#
# 前置：
#   - 系统已安装 Python 3.10+（用于创建 venv）
#   - 模型无需在此步就绪（模型由 release 脚本 / download_models.py 处理）
# ============================================================================

param(
    [string]$Mirror = "https://pypi.tuna.tsinghua.edu.cn/simple",
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
$PythonDir = Split-Path -Parent $MyInvocation.MyCommand.Path   # python/
$VenvDir   = Join-Path $PythonDir ".venv-vcr"
$SpecFile  = Join-Path $PythonDir "vcr-server.spec"
$ReqFile   = Join-Path $PythonDir "requirements-vcr-packaging.txt"
$OutExe    = Join-Path $PythonDir "dist\vcr-server.exe"

Write-Host "==> VCR 微服务打包构建" -ForegroundColor Cyan
Write-Host "    Python 目录 : $PythonDir"
Write-Host "    venv 目录   : $VenvDir"

# 1. 创建最小化 venv（若不存在）
if (-not (Test-Path $VenvDir)) {
    Write-Host "[1/5] 创建最小化 venv..." -ForegroundColor Green
    & python -m venv $VenvDir
    if ($LASTEXITCODE -ne 0) { throw "venv 创建失败" }
} else {
    Write-Host "[1/5] venv 已存在，跳过创建" -ForegroundColor Green
}

$VenvPython = Join-Path $VenvDir "Scripts\python.exe"
$PyInstaller = Join-Path $VenvDir "Scripts\pyinstaller.exe"

# 2. 安装打包期依赖（幂等，已装则跳过）
Write-Host "[2/5] 安装打包期依赖（$ReqFile）..." -ForegroundColor Green
& $VenvPython -m pip install -r $ReqFile -i $Mirror --quiet
if ($LASTEXITCODE -ne 0) { throw "依赖安装失败" }

# 3. 清理旧产物（dist / build 缓存）
Write-Host "[3/5] 清理旧打包产物..." -ForegroundColor Green
if (Test-Path (Join-Path $PythonDir "dist")) { Remove-Item -Recurse -Force (Join-Path $PythonDir "dist") }
if (Test-Path (Join-Path $PythonDir "build")) { Remove-Item -Recurse -Force (Join-Path $PythonDir "build") }

# 4. 用 spec 打包
Write-Host "[4/5] PyInstaller 打包（spec: $SpecFile）..." -ForegroundColor Green
Push-Location $PythonDir
try {
    & $PyInstaller --clean --noconfirm $SpecFile
    if ($LASTEXITCODE -ne 0) { throw "PyInstaller 打包失败（exit=$LASTEXITCODE）" }
} finally {
    Pop-Location
}

# 5. 校验产物
Write-Host "[5/5] 校验产物..." -ForegroundColor Green
if (-not (Test-Path $OutExe)) { throw "未生成 $OutExe" }
$size = (Get-Item $OutExe).Length / 1MB
Write-Host ("    OK: {0}  ({1:N1} MB)" -f $OutExe, $size) -ForegroundColor Green
Write-Host "==> 完成。下一步：运行 release.ps1 打包 MSI（或手动 tauri build）。" -ForegroundColor Cyan
