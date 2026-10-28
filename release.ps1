# ============================================================================
# PhotoManagementSys 发布打包脚本（Windows）
#
# 流程：
#   1. 构建 VCR 视觉识别微服务单文件 exe（python/build_vcr_exe.ps1）
#   2. 校验/补齐模型文件（python/models/，download_models.py 下载 face+ocr+clip；
#      cls/det 需 ultralytics 导出，缺失则给出提示）
#   3. 校验前端构建依赖（node_modules）
#   4. tauri build --config release.tauri.conf.json —— 产出 MSI / NSIS 安装包，
#      内嵌 vcr-server.exe + 模型（release 专用配置合并，避免污染开发用的基础配置）
#   5. 产物自检：解包安装包，逐个核对 release 配置里声明的资源真的在里面
#      （BUG-2026-1028-011：0.4.1~0.4.3 是用 release.tauri.conf.no-models.json 打的包，
#       一个模型都没带，而应用内下载表当时又漏了 yolov8n-det.onnx —— 新装用户
#       「人物通道不可用」且无法自救。若你确实要发轻量包（只带 vcr-server.exe），
#       必须确认 model_dl::specs() 覆盖 person-det / ocr / face / clip 全部四类，
#       否则别发 —— 本脚本第 5 步的自检会拦住无模型包的误发。）
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
    Write-Host "[1/5] 跳过 VCR exe 构建（使用现有 python/dist/vcr-server.exe）" -ForegroundColor Yellow
} else {
    Write-Host "[1/5] 构建 VCR 微服务 exe..." -ForegroundColor Green
    & powershell -ExecutionPolicy Bypass -File (Join-Path $PythonDir "build_vcr_exe.ps1") -Mirror $Mirror
    if ($LASTEXITCODE -ne 0) { throw "VCR exe 构建失败" }
}

# --- 2. 校验模型 ---
if ($SkipModels) {
    Write-Host "[2/5] 跳过模型校验（发布包将不含模型）" -ForegroundColor Yellow
} else {
    Write-Host "[2/5] 校验模型文件..." -ForegroundColor Green
    # v5：分类模型（yolov8*-cls / Places365）已下线，改为要求「人物检测 + 人脸 + OCR」
    #     以及语义模型（CLIP，缺它则语义搜索/语义分类在安装包里不可用）
    # 人脸模型两档都内置（性能设置里可切，换档需重建人物库）；与 config.FACE_MODEL_META 一致
    $required = @(
        "yolov8n-det.onnx",
        "det_10g.onnx", "w600k_r50.onnx",
        "det_500m.onnx", "w600k_mbf.onnx",
        "paddleocr-det.onnx",
        # 语义模型：双塔整图 + 伴随文件（与 release.tauri.conf.json 的
        # bundle.resources 显式清单严格对应；缺整图会让 tauri build 在打包阶段失败）
        "chinese-clip/onnx/model_fp16.onnx",
        "chinese-clip/tokenizer.json", "chinese-clip/vocab.txt"
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
  - 人物检测/人脸/OCR/语义模型：python download_models.py --tasks face,ocr,clip
  - 语义模型拆分件由服务首次使用时自动生成（也可先跑 python extract_clip_subgraphs.py b16）
  - 也可从已有开发机直接复制 python/models/ 下对应文件
"@
    }
    Write-Host "    模型校验通过（人物检测/人脸/OCR/语义模型齐全）" -ForegroundColor Green
}

# --- 3. 前端依赖 ---
Write-Host "[3/5] 校验前端依赖..." -ForegroundColor Green
if (-not (Test-Path (Join-Path $Root "node_modules"))) {
    Write-Host "    node_modules 缺失，执行 npm install..." -ForegroundColor Yellow
    Push-Location $Root
    try { & npm install; if ($LASTEXITCODE -ne 0) { throw "npm install 失败" } }
    finally { Pop-Location }
}

# --- 4. tauri build（合并 release 配置以带上 vcr 资源） ---
Write-Host "[4/5] 执行 tauri build（--config 合并 release 配置，产出 MSI / NSIS，含内嵌微服务与模型）..." -ForegroundColor Green
if (-not (Test-Path $ReleaseConfig)) {
    throw "缺少 release 配置: $ReleaseConfig"
}
Push-Location $Root
try {
    & npx tauri build --config $ReleaseConfig
    if ($LASTEXITCODE -ne 0) { throw "tauri build 失败" }
} finally { Pop-Location }

# --- 5. 产物自检：安装包内必须真的带上配置声明的资源（BUG-2026-1028-011）---
Write-Host "[5/5] 校验安装包内容..." -ForegroundColor Green
$cfg = Get-Content $ReleaseConfig -Raw | ConvertFrom-Json
$expectTargets = @($cfg.bundle.resources.PSObject.Properties | ForEach-Object { [string]$_.Value })
$setup = Get-ChildItem (Join-Path $Root "src-tauri\target\release\bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) {
    Write-Host "    ⚠ 未找到 NSIS 安装包，跳过内容自检" -ForegroundColor Yellow
} else {
    $sevenZip = @("C:\Program Files\7-Zip\7z.exe", "C:\Program Files (x86)\7-Zip\7z.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $sevenZip) {
        Write-Host "    ⚠ 未装 7-Zip，无法解包自检。请人工确认 $($setup.Name) 内含 vcr\models\*.onnx" -ForegroundColor Yellow
    } else {
        $listing = (& $sevenZip l $setup) -join "`n"
        $missing = @()
        foreach ($rel in $expectTargets) {
            $want = $rel -replace '/', '\'
            if ($listing -notmatch [regex]::Escape($want)) { $missing += $want }
        }
        if ($missing.Count -gt 0) {
            throw @"
安装包缺少以下资源：$($missing -join ', ')
  大概率是用错了 --config（不带模型的配置）打的包。
  无模型包只有一种情况可以发：model_dl::specs() 已覆盖 person-det / ocr / face / clip，
  且你要接受新装用户必须先联网下载模型。
"@
        }
        Write-Host "    自检通过：$($expectTargets.Count) 项资源全部命中" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "==> 构建完成。" -ForegroundColor Cyan
Write-Host "    安装包位于 src-tauri/target/release/bundle/msi/ 与 nsis/。" -ForegroundColor Cyan
Write-Host "    请将以下两处上传到 GitHub Release（与源码 release_body.json 一致）：" -ForegroundColor Cyan
Write-Host "      - PhotoManagementSys_<version>_x64_en-US.msi"
Write-Host "      - PhotoManagementSys_<version>_x64-setup.exe"
