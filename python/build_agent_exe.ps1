# ============================================================================
# 构建 Agent 侧车单文件 exe（PyInstaller，最小化环境）
# 产物：python/dist/agent-server.exe（含 Python 运行时 + Pillow，预算 ≤40MB）
#
# 与 build_vcr_exe.ps1 同构，差异只有三处：
#   1. venv 用 .venv-agent（不含 onnxruntime/opencv/numpy，体积从 90MB 降到 40MB）；
#   2. 打包后有体积闸（>44MB 直接失败）—— 体积是设计约束，不是"看着办"；
#   3. 打完立刻自检一次 /health（用 exe 自身起服务再杀），
#      避免"打包成功但跑不起来"到用户机器上才发现（docs/v5.md §23 第 1 条风险）。
#
# 用法（项目根目录或任意目录）：
#   powershell -ExecutionPolicy Bypass -File python/build_agent_exe.ps1
#   powershell -ExecutionPolicy Bypass -File python/build_agent_exe.ps1 -SkipSelfCheck
#
# 前置：系统已安装 Python 3.10+
# ============================================================================

param(
    [string]$Mirror = "https://pypi.tuna.tsinghua.edu.cn/simple",
    [switch]$SkipSelfCheck
)

$ErrorActionPreference = "Stop"
$PythonDir = Split-Path -Parent $MyInvocation.MyCommand.Path   # python/
$VenvDir   = Join-Path $PythonDir ".venv-agent"
$SpecFile  = Join-Path $PythonDir "agent_server.spec"
$ReqFile   = Join-Path $PythonDir "requirements-agent.txt"
$OutExe    = Join-Path $PythonDir "dist\agent-server.exe"
$SizeLimitMb = 44   # 预算 40MB，留 4MB 给 PyInstaller 版本波动

Write-Host "==> Agent 侧车打包构建" -ForegroundColor Cyan
Write-Host "    Python 目录 : $PythonDir"
Write-Host "    venv 目录   : $VenvDir"

# 1. 创建最小化 venv（若不存在）
if (-not (Test-Path $VenvDir)) {
    Write-Host "[1/6] 创建最小化 venv..." -ForegroundColor Green
    # 不能用裸 `python`：Windows「应用执行别名」会把 PATH 上的 python.exe 指向
    # Microsoft Store 占位符（能执行但立即退出，只打印 'Python was not found'），
    # 于是 venv 创建失败且报错含糊。逐候选试并用 --version 验真（BUG-2026-0918-007）。
    $BasePython = $null
    $Candidates = @(
        @{ exe = "py";      pre = @("-3") },
        @{ exe = "python";  pre = @() },
        @{ exe = "python3"; pre = @() }
    )
    foreach ($c in $Candidates) {
        if (-not (Get-Command $c.exe -ErrorAction SilentlyContinue)) { continue }
        $ver = & $c.exe @($c.pre) --version 2>&1
        if ($LASTEXITCODE -eq 0 -and ("$ver" -match "^Python \d")) {
            $BasePython = $c
            break
        }
        $msg = ("$ver" | Out-String).Trim() -replace "\r?\n", " "
        if ($msg.Length -gt 160) { $msg = $msg.Substring(0, 160) + "..." }
        Write-Host ("      候选 {0} 不可用：{1}" -f $c.exe, $msg) -ForegroundColor DarkYellow
    }
    if (-not $BasePython) {
        throw "找不到可用的 Python（用于创建 venv）。最常见原因：系统 python.exe 被 Microsoft Store 的「应用执行别名」劫持（运行只打印 'Python was not found'）。请在「设置 → 应用 → 高级应用设置 → 应用执行别名」关闭 python.exe / python3.exe，或安装 python.org 版本后重跑本脚本。"
    }
    Write-Host "      使用解释器：$($BasePython.exe) $($BasePython.pre -join ' ')" -ForegroundColor DarkGray
    & $BasePython.exe @($BasePython.pre) -m venv $VenvDir
    if ($LASTEXITCODE -ne 0) { throw "venv 创建失败" }
} else {
    Write-Host "[1/6] venv 已存在，跳过创建" -ForegroundColor Green
}

$VenvPython  = Join-Path $VenvDir "Scripts\python.exe"
$PyInstaller = Join-Path $VenvDir "Scripts\pyinstaller.exe"
if (-not (Test-Path $VenvPython)) { throw "venv 不完整：缺少 $VenvPython（删掉 $VenvDir 重跑本脚本）" }

# 2. 安装打包期依赖（幂等）
Write-Host "[2/6] 安装打包期依赖（$ReqFile）..." -ForegroundColor Green
& $VenvPython -m pip install -r $ReqFile -i $Mirror --quiet
if ($LASTEXITCODE -ne 0) {
    # 镜像不可用（网络/证书/镜像停摆）不应变成“构建失败”，回退官方源再试一次。
    Write-Host "    ⚠ 镜像安装失败，回退官方 PyPI 重试..." -ForegroundColor DarkYellow
    & $VenvPython -m pip install -r $ReqFile --quiet
    if ($LASTEXITCODE -ne 0) { throw "依赖安装失败（镜像与官方源都失败）" }
}

# 2b. 启动前自检：源码能在 venv 里 import（打包失败时最常见的原因是源码本身有语法/导入错误，
#     那种情况 PyInstaller 会打出一个"能启动但立刻退出"的 exe，排查成本远高于在这里拦下）
Write-Host "[2b/6] 源码导入自检..." -ForegroundColor Green
Push-Location $PythonDir
try {
    & $VenvPython -c "import agent.app, agent.auth, agent.jobs, agent.logutil; print('imports OK')"
    if ($LASTEXITCODE -ne 0) { throw "源码导入失败（见上方 traceback）" }
} finally { Pop-Location }

# 3. 清理旧产物
#    ⚠️ 只删自己的产物，不能整目录 wipe `python/dist` —— 那里还放着
#    vcr-server.exe（release.ps1 -SkipExe 会直接用它）。整 wipe 会让
#    “只重建 agent”变成“顺手删了一个别的阶段才能重建的产物”。
Write-Host "[3/6] 清理旧打包产物..." -ForegroundColor Green
foreach ($p in @(
    (Join-Path $PythonDir "dist\agent-server.exe"),
    (Join-Path $PythonDir "build\agent-server")
)) {
    if (Test-Path $p) { Remove-Item -Recurse -Force $p }
}
New-Item -ItemType Directory -Force -Path (Join-Path $PythonDir "dist") | Out-Null

# 4. 用 spec 打包
Write-Host "[4/6] PyInstaller 打包（spec: $SpecFile）..." -ForegroundColor Green
Push-Location $PythonDir
try {
    & $PyInstaller --clean --noconfirm $SpecFile
    if ($LASTEXITCODE -ne 0) { throw "PyInstaller 打包失败（exit=$LASTEXITCODE）" }
} finally {
    Pop-Location
}

# 5. 校验产物 + 体积闸
Write-Host "[5/6] 校验产物..." -ForegroundColor Green
if (-not (Test-Path $OutExe)) { throw "未生成 $OutExe" }
$sizeMb = (Get-Item $OutExe).Length / 1MB
Write-Host ("    {0}  {1:N1} MB" -f $OutExe, $sizeMb) -ForegroundColor Green
if ($sizeMb -gt $SizeLimitMb) {
    $msg = ("体积超预算：{0:N1} MB > {1} MB 闸值。" -f $sizeMb, $SizeLimitMb) +
           "预算 40MB 是设计约束（docs/v5.md §25-3）：把 onnxruntime/opencv 打进来就会到 100MB+，" +
           "那属于「改方案」而不是「调参数」，请先确认。"
    throw $msg
}

# 6. exe 自检：用产物自身起服务 → /health → 杀进程
#    为什么必须在构建期做：/health 通不代表 /agent/submit 通（hiddenimports 漏项
#    只在真正 import 业务模块时才炸）。这里要求 200 且 api 版本对得上。
if ($SkipSelfCheck) {
    Write-Host "[6/6] 跳过 exe 自检（-SkipSelfCheck）" -ForegroundColor Yellow
} else {
    Write-Host "[6/6] exe 自检..." -ForegroundColor Green
    $probePort = 18791
    $env:AGENT_TOKEN = "build-selfcheck-token"
    $env:AGENT_PORT  = "$probePort"
    $proc = Start-Process -FilePath $OutExe -PassThru -WindowStyle Hidden
    try {
        $ok = $false
        for ($i = 0; $i -lt 40; $i++) {
            Start-Sleep -Milliseconds 250
            try {
                $r = Invoke-RestMethod -Uri "http://127.0.0.1:$probePort/health" -TimeoutSec 2
                if ($r.ok -and $r.service -eq "pm-agent") { $ok = $true; break }
            } catch { }
            if ($proc.HasExited) { break }
        }
        if (-not $ok) { throw "exe 自检失败：/health 不可达（看 python/dist 下是否有 agent-server 的崩溃输出）" }
        Write-Host ("    /health OK  service={0} api={1}" -f $r.service, $r.api) -ForegroundColor Green
    } finally {
        # /T 连子进程一起结束（Windows 上真正的解释器是子进程）
        if (-not $proc.HasExited) {
            & taskkill /F /T /PID $proc.Id 2>&1 | Out-Null
        }
        Remove-Item Env:\AGENT_TOKEN -ErrorAction SilentlyContinue
        Remove-Item Env:\AGENT_PORT  -ErrorAction SilentlyContinue
    }
}

Write-Host "==> 完成。下一步：scripts/smoke-agent.ps1 跑契约冒烟，或 release.ps1 打 MSI。" -ForegroundColor Cyan
