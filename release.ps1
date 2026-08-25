# ============================================================================
# PhotoManagementSys 发布打包脚本（Windows）
#
# 流程：
#   1. 构建 VCR 视觉识别微服务单文件 exe（python/build_vcr_exe.ps1）
#   2. 校验/补齐模型文件（python/models/，download_models.py 下载 face+ocr；
#      cls/det 需 ultralytics 导出，缺失则给出提示）
#   3. 校验前端构建依赖（node_modules）
#   4. tauri build --config release.tauri.conf.json —— 产出 MSI / NSIS 安装包，
#      内嵌 vcr-server.exe + 模型（release 专用配置合并，避免污染开发用的基础配置）
#
# 用法（在项目根目录）：
#   powershell -ExecutionPolicy Bypass -File release.ps1 [-SkipExe] [-SkipModels]
#
# 可选参数：
#   -SkipExe    : 跳过 PyInstaller 重建（用 python/dist/vcr-server.exe 现有产物）
#   -SkipModels : 跳过模型校验（不推荐，MSI 将不含模型）
# ============================================================================

param(
    [string]$Mirror = "https://pypi.tuna.tsinghua.edu.cn/simple",
    [switch]$SkipExe,
    [switch]$SkipModels
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path      # 项目根
$PythonDir = Join-Path $Root "python"
$ModelDir  = Join-Path $PythonDir "models"
$ReleaseConfig = Join-Path $Root "src-tauri\release.tauri.conf.json"

Write-Host "==> PhotoManagementSys 发布打包" -ForegroundColor Cyan
Write-Host "    项目根 : $Root"

# --- 1. 构建 VCR exe ---
if ($SkipExe) {
    Write-Host "[1/4] 跳过 VCR exe 构建（使用现有 python/dist/vcr-server.exe）" -ForegroundColor Yellow
} else {
    Write-Host "[1/4] 构建 VCR 微服务 exe..." -ForegroundColor Green
    & powershell -ExecutionPolicy Bypass -File (Join-Path $PythonDir "build_vcr_exe.ps1") -Mirror $Mirror
    if ($LASTEXITCODE -ne 0) { throw "VCR exe 构建失败" }
}

# --- 2. 校验模型 ---
if ($SkipModels) {
    Write-Host "[2/4] 跳过模型校验（发布包将不含模型）" -ForegroundColor Yellow
} else {
    Write-Host "[2/4] 校验模型文件..." -ForegroundColor Green
    $required = @(
        "yolov8n-cls.onnx", "yolov8n-det.onnx",
        "det_500m.onnx", "w600k_mbf.onnx",
        "paddleocr-det.onnx"
    )
    $missing = @()
    foreach ($m in $required) {
        if (-not (Test-Path (Join-Path $ModelDir $m))) { $missing += $m }
    }
    if ($missing.Count -gt 0) {
        Write-Host "    缺失模型: $($missing -join ', ')" -ForegroundColor Yellow
        Write-Host "    尝试用 download_models.py 补齐 face/ocr..." -ForegroundColor Yellow
        Push-Location $PythonDir
        try {
            & python download_models.py
            if ($LASTEXITCODE -ne 0) { throw "download_models.py 执行失败" }
        } finally { Pop-Location }
    }
    # 复检
    $stillMissing = @()
    foreach ($m in $required) {
        if (-not (Test-Path (Join-Path $ModelDir $m))) { $stillMissing += $m }
    }
    if ($stillMissing.Count -gt 0) {
        throw @"
仍缺失以下模型：$($stillMissing -join ', ')
  - 人脸/OCR：请确认网络可访问，或手动执行 python/download_models.py
  - 分类/检测：需 ultralytics 导出（python/export_model.py 导出 cls；det 同理用 ultralytics）
  - 也可从已有开发机直接复制 python/models/ 下对应 .onnx 文件
"@
    }
    Write-Host "    模型校验通过（5 个必需 ONNX 齐全）" -ForegroundColor Green
}

# --- 3. 前端依赖 ---
Write-Host "[3/4] 校验前端依赖..." -ForegroundColor Green
if (-not (Test-Path (Join-Path $Root "node_modules"))) {
    Write-Host "    node_modules 缺失，执行 npm install..." -ForegroundColor Yellow
    Push-Location $Root
    try { & npm install; if ($LASTEXITCODE -ne 0) { throw "npm install 失败" } }
    finally { Pop-Location }
}

# --- 4. tauri build（合并 release 配置以带上 vcr 资源） ---
Write-Host "[4/4] 执行 tauri build（--config 合并 release 配置，产出 MSI / NSIS，含内嵌微服务与模型）..." -ForegroundColor Green
if (-not (Test-Path $ReleaseConfig)) {
    throw "缺少 release 配置: $ReleaseConfig"
}
Push-Location $Root
try {
    & npx tauri build --config $ReleaseConfig
    if ($LASTEXITCODE -ne 0) { throw "tauri build 失败" }
} finally { Pop-Location }

Write-Host ""
Write-Host "==> 构建完成。" -ForegroundColor Cyan
Write-Host "    安装包位于 src-tauri/target/release/bundle/msi/ 与 nsis/。" -ForegroundColor Cyan
Write-Host "    请将以下两处上传到 GitHub Release（与源码 release_body.json 一致）：" -ForegroundColor Cyan
Write-Host "      - PhotoManagementSys_<version>_x64_en-US.msi"
Write-Host "      - PhotoManagementSys_<version>_x64-setup.exe"
