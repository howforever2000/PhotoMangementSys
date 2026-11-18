# 工作区完整性标签自检 / 修复（幂等）
#
# 为什么需要（BUG-2026-1027-007，2026-11-12 复发）：
#   工作区根目录一旦被打上 Low 完整性标签（`Mandatory Label\Low Mandatory Level`），
#   src-tauri\target\** 下的 exe 全部继承 Low IL；Windows 强制完整性控制（MIC）
#   默认 no-write-up —— Low IL 进程写不了 Medium 的 %APPDATA% / %TEMP%，
#   所有写入统一返回 ACCESS_DENIED(5)，应用启动即死在「数据目录不可写」。
#   这与 ACL、杀软、目录占用全都无关，而 icacls 默认把 Mandatory Label 排在
#   输出最后一行，肉眼极易漏看 —— 所以在每次 dev 启动前机械地查一次。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fix-workspace-integrity.ps1
#   powershell ... -File scripts/fix-workspace-integrity.ps1 -Check   # 只检查：Low 则 exit 1
#
# 退出码：修复模式恒为 0（修不了也不能阻断 dev，应用侧会给出明确报错）；
#         -Check 模式检测到 Low 时 exit 1。

param([switch]$Check)

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "src-tauri\target\debug\photo-management-sys.exe"

# 完整性标签只在 icacls 文本里可见（Get-Acl 的 Sddl 不含 ML 段），
# 因此同时匹配英文显示名与 SID，避免 Windows 语言/版本差异导致漏判。
function Test-LowIntegrity([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $out = icacls $path 2>&1 | Out-String
    return [bool]($out -match 'Low Mandatory Level' -or $out -match 'S-1-16-4096')
}

if (-not (Test-LowIntegrity $root)) {
    if (-not $Check) { Write-Host "[integrity] 工作区完整性标签正常（非 Low）" -ForegroundColor DarkGray }
    exit 0
}

if ($Check) {
    Write-Host "[integrity][FAIL] 工作区根目录带 Low 完整性标签：$root" -ForegroundColor Red
    Write-Host "[integrity] 修复：icacls \"$root\" /setintegritylevel (OI)(CI)M" -ForegroundColor Yellow
    exit 1
}

Write-Host "[integrity] 检测到 Low 完整性标签，提升到 Medium：$root" -ForegroundColor Yellow
try {
    icacls $root /setintegritylevel "(OI)(CI)M" | Out-Null
} catch {
    Write-Host "[integrity][WARN] 提升失败（$($_.Exception.Message)），继续启动；应用若报「数据目录不可写」请以管理员身份重试" -ForegroundColor Red
    exit 0
}

# 继承标签会在设置根目录时自动传播；但历史上被**显式**打过 Low 的子对象不会变，
# 其中最关键的是待运行的 exe —— 单独补一刀，避免「根目录已 Medium、exe 仍 Low」。
if ((Test-Path -LiteralPath $exe) -and (Test-LowIntegrity $exe)) {
    try { icacls $exe /setintegritylevel M | Out-Null } catch { }
}

if (Test-LowIntegrity $root) {
    Write-Host "[integrity][WARN] 根目录仍为 Low，请以管理员身份执行并按提示处理" -ForegroundColor Red
} else {
    Write-Host "[integrity] 已提升为 Medium" -ForegroundColor Green
}
exit 0
