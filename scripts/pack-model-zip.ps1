# ============================================================================
# 生成离线模型包 model.zip（BUG-2026-1119-002）
#
# 为什么要有这个脚本：
#   离线包一直是**手工制作 + 手工上传**（`.gitignore` 里有 `/*.zip`，提交正文里
#   手工记着「361.1 MB（7 个文件），字节级复核与源一致」）。手工的后果已经发生过：
#   包的体积口径在三处互不相等（UI「529MB」/ README「856MB」/ 代码实算 979MB），
#   且 v0.4.5 换了 fp32 语义档之后没人确认包里的结构还是不是 `vcr/models/…`。
#
# 文件清单**不在这里手抄**：直接从 `src-tauri/src/model_dl.rs` 的注册表解析。
#   那是「应用内下载」与「离线包」唯一共同的真相源 —— 下载表里有什么、落在哪个
#   相对路径，离线包就必须同构。抄一份清单到脚本里，下次加模型时必然漏改，
#   而漏改的表现是「用户解压完仍然缺模型」，且不报错。
#
# 分区口径与前端 `src/utils/modelCatalog.ts` 严格一致：
#   · `local-*`              → 创意工坊本地增强模型，**不进**离线包（走应用内下载）
#   · `openvino-runtime`     → 运行时组件，不是模型，**不进**离线包
#   · 其余（person-det / chinese-clip-fp32 / face-* / ocr） → 进包
#
# 用法（在项目根目录）：
#   powershell -ExecutionPolicy Bypass -File scripts/pack-model-zip.ps1
#   powershell -ExecutionPolicy Bypass -File scripts/pack-model-zip.ps1 -Out model.zip
#
# 产出：<项目根>/model.zip（内层已是 `vcr/models/…`，用户解压到模型目录之父即可）
# ============================================================================

param(
    [string]$Out = "",
    # 只校验清单与源文件是否齐备，不实际压缩（发布前预检用）
    [switch]$CheckOnly
)

$ErrorActionPreference = "Stop"

$Root     = Split-Path -Parent $PSScriptRoot
$ModelDir = Join-Path $Root "python\models"
$SpecRs   = Join-Path $Root "src-tauri\src\model_dl.rs"

if (-not $Out) { $Out = Join-Path $Root "model.zip" }

if (-not (Test-Path $SpecRs)) { throw "找不到模型注册表: $SpecRs" }
if (-not (Test-Path $ModelDir)) { throw "找不到模型目录: $ModelDir" }

Write-Host "==> 解析模型注册表 $SpecRs" -ForegroundColor Cyan
$src = Get-Content $SpecRs -Raw

# --- 1. 收集每个 `const X: &[DlFile] = &[...]` 里声明的 local 落位路径 ---
# 注册表把文件清单抽成 const（PERSON_DET / CLIP_FP32 / FACE_PRECISE / ...），
# 再由 `files: &X` 引用 —— 所以必须两段解析再关联，不能按行顺序猜。
# 注意 Singleline：Rust 里这些声明是**跨多行**的，.NET 的 `.` 默认不匹配换行，
# 不加 Singleline 会一个都匹配不到（第一次写就踩了这个，报「解析不出清单」）。
$fileLists = @{}
$constPattern = [regex]::new(
    'const\s+([A-Z0-9_]+)\s*:\s*&\[DlFile\]\s*=\s*&\[(.*?)\];',
    [System.Text.RegularExpressions.RegexOptions]::Singleline
)
foreach ($m in $constPattern.Matches($src)) {
    $constName = $m.Groups[1].Value
    $body      = $m.Groups[2].Value
    $locals = @()
    foreach ($lm in [regex]::Matches($body, 'local:\s*"([^"]+)"')) {
        $locals += $lm.Groups[1].Value
    }
    if ($locals.Count -gt 0) { $fileLists[$constName] = $locals }
}
Write-Host "    声明了落位路径的文件组: $($fileLists.Count) 个"

if ($fileLists.Count -eq 0) {
    throw "解析不出任何 DlFile 清单 —— model_dl.rs 的结构可能已改，请同步更新本脚本"
}

# --- 2. 逐个 DlSpec 取 name 与它引用的 const，按分区规则过滤 ---
# 每个 DlSpec 以 `DlSpec {` 开头、以 8 空格缩进 + `},` 收尾，同样需 Singleline。
# 字段写的是 `files: PERSON_DET,`（**没有** & 前缀 —— 结构体声明 `files: &'static [DlFile]`
# 是另一回事），正则用 &? 兼容两种写法。
$specPattern = [regex]::new(
    'DlSpec\s*\{(.*?)\n {8}\},',
    [System.Text.RegularExpressions.RegexOptions]::Singleline
)
$wanted = @()   # 相对模型根的落位路径
$skipped = @()

foreach ($m in $specPattern.Matches($src)) {
    $body = $m.Groups[1].Value
    $nameM = [regex]::Match($body, 'name:\s*"([^"]+)"')
    $fileM = [regex]::Match($body, 'files:\s*&?([A-Z0-9_]+)')
    if (-not $nameM.Success -or -not $fileM.Success) { continue }

    $name = $nameM.Groups[1].Value
    $list = $fileM.Groups[1].Value

    if (-not $fileLists.ContainsKey($list)) {
        # 清单缺失时必须显式失败：静默跳过会让离线包少文件而无人察觉
        throw "模型 '$name' 引用了未知文件组 '&$list' —— 解析失败，请检查 model_dl.rs 结构"
    }

    # 与 utils/modelCatalog.ts 同一条分区判据
    if ($name.StartsWith("local-") -or $name -eq "openvino-runtime") {
        $skipped += $name
        continue
    }
    $wanted += $fileLists[$list]
}

if ($wanted.Count -eq 0) { throw "过滤后没有任何模型进入离线包 —— 分区判据可能写错了" }
Write-Host "    进包模型文件: $($wanted.Count) 个（跳过 $($skipped -join ', ')）" -ForegroundColor Green

# --- 3. 校验源文件齐备（缺一个就不打包，避免产出「看起来成功但少文件」的包）---
$missing = @()
foreach ($rel in $wanted) {
    $full = Join-Path $ModelDir ($rel -replace '/', '\')
    if (-not (Test-Path $full)) { $missing += $rel }
}
if ($missing.Count -gt 0) {
    Write-Host ""
    Write-Host "缺少以下模型文件，无法生成离线包：" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    throw @"
请先补齐 python/models/ 下的模型再打包：
  - 应用内「性能设置 → 模型管理」一键下载（推荐，落位与注册表一致）
  - 或 python/download_models.py --tasks face,ocr,clip
  - 人物检测 yolov8n-det.onnx 需从已有开发机复制
产出不含缺失文件的 zip 没有意义：用户解压后仍会看到「模型缺失」。
"@
}

# 解压后总体积（十进制 MB，与前端 fmtMB 同口径），供人工核对
$total = 0
foreach ($rel in $wanted) {
    $total += (Get-Item (Join-Path $ModelDir ($rel -replace '/', '\'))).Length
}
$totalMB = [math]::Round($total / 1e6, 1)
Write-Host "    解压后体积: $totalMB MB" -ForegroundColor Green
Write-Host "    （前端 utils/modelOfflinePackage.ts 展示的 OFFLINE_PACKAGE_UNPACKED_MB 应与本值一致；"
Write-Host "      改动了注册表的识别链路模型后，记得同步那里的常量）" -ForegroundColor DarkGray

if ($CheckOnly) {
    Write-Host "==> -CheckOnly：清单与源文件校验通过，未生成 zip" -ForegroundColor Cyan
    return
}

# --- 4. 暂存成 vcr/models/… 结构再压缩 ---
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("pms-model-pack-" + [guid]::NewGuid().ToString("N"))
try {
    $stageRoot = Join-Path $stage "vcr\models"
    New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null

    foreach ($rel in $wanted) {
        $srcFile = Join-Path $ModelDir ($rel -replace '/', '\')
        $dstFile = Join-Path $stageRoot ($rel -replace '/', '\')
        $dstDir  = Split-Path -Parent $dstFile
        if (-not (Test-Path $dstDir)) { New-Item -ItemType Directory -Force -Path $dstDir | Out-Null }
        Copy-Item -LiteralPath $srcFile -Destination $dstFile -Force
    }

    if (Test-Path $Out) { Remove-Item $Out -Force }
    Write-Host "==> 压缩 -> $Out（大文件，请耐心等待）" -ForegroundColor Cyan
    # Optimal 而非 Fastest：这是给网络受限用户下的兜底包，体积优先
    Compress-Archive -Path (Join-Path $stage "vcr") -DestinationPath $Out -CompressionLevel Optimal
}
finally {
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue }
}

$zipMB = [math]::Round((Get-Item $Out).Length / 1e6, 1)
Write-Host ""
Write-Host "==> 完成：model.zip $zipMB MB（解压后 $totalMB MB / $($wanted.Count) 个文件）" -ForegroundColor Green
Write-Host "    内层结构：vcr\models\…（用户解压到模型目录之父即可，不要多套一层）" -ForegroundColor Green
Write-Host ""
Write-Host "    上传提醒：GitHub Release 里务必附上 model.zip。" -ForegroundColor Yellow
Write-Host "    应用内用的是 releases/latest/download/model.zip（稳定别名），" -ForegroundColor Yellow
Write-Host "    该版本若缺这个资产，用户点「离线包」会 404。" -ForegroundColor Yellow
