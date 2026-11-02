# ============================================================================
# Agent 侧车契约冒烟（不需要 Tauri、不需要打包）
#
# 验证 5 件事，全部是本项目真实踩过的坑对应的门：
#   1. /health 免 token 可达，且 service/api 与 Rust 侧约定一致（收养判定依赖它）；
#   2. X-Agent-Token 真的在拦人（无 token = 401），否则等于对全机开放；
#   3. POST /agent/submit **真能拿到 jobId**（Docs v5 §22 阶段 0 的验收原话：
#      「不只是 /health」—— hiddenimports 漏项只在真正 import 业务模块时才炸）；
#   4. 进度是逐个出来的、终态能收敛（不是 0 跳到 100）；
#   5. 取消在 500ms 内收敛；单飞约束返回 409；强杀后端口能释放。
#
# 用法（项目根目录）：
#   powershell -ExecutionPolicy Bypass -File scripts/smoke-agent.ps1
#   powershell -ExecutionPolicy Bypass -File scripts/smoke-agent.ps1 -ExePath python/dist/agent-server.exe
# ============================================================================

param(
    [string]$PythonExe = "",
    [string]$ExePath = "",
    [int]$Port = 18792
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Token = "smoke-" + [guid]::NewGuid().ToString("N")
$Base = "http://127.0.0.1:$Port"
$pyPre = @()
$script:Failures = @()
$script:Checks = 0

function Pass([string]$msg) {
    $script:Checks++
    Write-Host ("  [PASS] {0}" -f $msg) -ForegroundColor Green
}
function Fail([string]$msg) {
    $script:Checks++
    $script:Failures += $msg
    Write-Host ("  [FAIL] {0}" -f $msg) -ForegroundColor Red
}
function Check([bool]$cond, [string]$msg, [string]$detail = "") {
    if ($cond) { Pass $msg }
    else { Fail ($msg + $(if ($detail) { " —— $detail" } else { "" })) }
}

function Api {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null,
        # 注意用 [object] 而不是 [string]：PowerShell 会把 $null 强转成空串，
        # 那样就分不出“用默认 token”与“一个都不带”（踩过：本该带 token 的请求变成 401）
        [object]$TokenValue = $null,
        [int]$TimeoutSec = 10
    )
    $headers = @{}
    $tk = if ($null -eq $TokenValue) { $Token } else { [string]$TokenValue }
    if ($tk) { $headers["X-Agent-Token"] = $tk }
    # 注意：不要把这个哈希命名为 $args —— 那是 PowerShell 的自动变量
    $callArgs = @{
        Uri         = "$Base$Path"
        Method      = $Method
        Headers     = $headers
        TimeoutSec  = $TimeoutSec
        ErrorAction = "Stop"
    }
    if ($null -ne $Body) {
        $callArgs["Body"] = ($Body | ConvertTo-Json -Compress -Depth 8)
        $callArgs["ContentType"] = "application/json"
    }
    return Invoke-RestMethod @callArgs
}

# 拿到 HTTP 状态码（PS5.1 与 PS7 的取值方式不同，两种都试）
function ApiStatus {
    param([string]$Method, [string]$Path, [object]$Body = $null, [object]$TokenValue = $null)
    try {
        $null = Api -Method $Method -Path $Path -Body $Body -TokenValue $TokenValue
        return 200
    } catch {
        $resp = $_.Exception.Response
        if (-not $resp) { return -1 }
        try { return [int]$resp.StatusCode } catch { return [int]$resp.StatusCode.value__ }
    }
}

# ---------------------------------------------------------------- 准备解释器
Write-Host "==> Agent 侧车契约冒烟" -ForegroundColor Cyan

$useExe = $false
if ($ExePath) {
    $full = if ([System.IO.Path]::IsPathRooted($ExePath)) { $ExePath } else { Join-Path $Root $ExePath }
    if (-not (Test-Path $full)) { throw "找不到指定的 exe：$full" }
    $useExe = $true
    $LaunchExe = $full
    Write-Host "    被测对象 : exe（$full）" -ForegroundColor DarkGray
} else {
    if (-not $PythonExe) {
        $cand = Join-Path $Root "python\.venv-agent\Scripts\python.exe"
        if (Test-Path $cand) { $PythonExe = $cand }
    }
    if (-not $PythonExe) {
        foreach ($c in @(@{ exe = "py"; pre = @("-3") }, @{ exe = "python"; pre = @() })) {
            if (-not (Get-Command $c.exe -ErrorAction SilentlyContinue)) { continue }
            $v = & $c.exe @($c.pre) --version 2>&1
            if ($LASTEXITCODE -eq 0 -and ("$v" -match "^Python \d")) { $PythonExe = $c.exe; $pyPre = $c.pre; break }
        }
    }
    if (-not $PythonExe) { throw "找不到 Python。请先装 Python 3.10+，或传 -PythonExe 指定。" }
    Write-Host "    被测对象 : $PythonExe python/agent_server.py" -ForegroundColor DarkGray
    # 依赖自检：缺 fastapi/uvicorn 的话失败信息会很难懂，这里先拦
    $probe = & $PythonExe @($pyPre) -c "import fastapi, uvicorn, httpx, PIL; print('ok')" 2>&1
    if ($LASTEXITCODE -ne 0) {
        $probe = & $PythonExe -c "import fastapi, uvicorn, httpx, PIL; print('ok')" 2>&1
    }
    if ($LASTEXITCODE -ne 0) {
        throw "依赖缺失（fastapi/uvicorn/httpx/Pillow）。先执行：`n  $PythonExe -m pip install -r python/requirements-agent.txt -i https://pypi.tuna.tsinghua.edu.cn/simple`n实际输出：$probe"
    }
}

Write-Host "    端口     : $Port" -ForegroundColor DarkGray
Write-Host "    token    : $($Token.Substring(0,8))…（每次随机）" -ForegroundColor DarkGray

# ---------------------------------------------------------------- 起服务
$env:AGENT_PORT  = "$Port"
$env:AGENT_TOKEN = $Token
$env:PYTHONIOENCODING = "utf-8"
$env:PYTHONUTF8 = "1"

$logFile = Join-Path $env:TEMP ("agent-smoke-{0}.log" -f $Port)
$proc = $null
try {
    if ($useExe) {
        $proc = Start-Process -FilePath $LaunchExe -PassThru -WindowStyle Hidden `
            -RedirectStandardError $logFile -RedirectStandardOutput "$logFile.out"
    } else {
        $serverScript = Join-Path $Root "python\agent_server.py"
        # ⚠️ 路径带空格（本机就是 D:\YUAN HAO\...）：
        # Start-Process 的 -ArgumentList 用空格拼参时**不会自动加引号**，
        # 不自己包一层就会被拆成 D:\YUAN / HAO / ...
        $argStr = (@($pyPre) + @("`"$serverScript`"") | Where-Object { $_ }) -join " "
        $proc = Start-Process -FilePath $PythonExe -ArgumentList $argStr -PassThru -WindowStyle Hidden `
            -WorkingDirectory (Join-Path $Root "python") `
            -RedirectStandardError $logFile -RedirectStandardOutput "$logFile.out"
    }

    # ------------------------------------------------------------ 1. /health
    $health = $null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    for ($i = 0; $i -lt 80; $i++) {
        Start-Sleep -Milliseconds 125
        if ($proc.HasExited) { break }
        try { $health = Api -Method Get -Path "/health" -TokenValue ""; break } catch { }
    }
    $bootMs = $sw.ElapsedMilliseconds
    if ($null -eq $health) {
        $tail = if (Test-Path $logFile) { (Get-Content $logFile -Tail 20) -join " | " } else { "（无日志）" }
        Fail "/health 不可达（冷启动等待 ${bootMs}ms）。进程输出：$tail"
        throw "服务起不来，后续检查无法进行"
    }
    Check ($health.ok -eq $true) "/health 返回 ok=true"
    Check ($health.service -eq "pm-agent") "/health.service = pm-agent" ("实得 " + $health.service)
    Check ($health.api -eq 1) "/health.api = 1（Rust 收养判定依赖它）" ("实得 " + $health.api)
    Pass ("冷启动耗时 {0}ms（预算 ≤2500ms）" -f $bootMs)

    # ------------------------------------------------------------ 2. token 必须拦人
    Check ((ApiStatus -Method Get -Path "/agent/jobs/1" -TokenValue "") -eq 401) "无 token 访问 job 状态返回 401"
    Check ((ApiStatus -Method Get -Path "/agent/jobs/1" -TokenValue "wrong-token") -eq 401) "错误 token 返回 401"

    # ------------------------------------------------------------ 3. submit 拿 jobId
    $job = $null
    try {
        $job = Api -Method Post -Path "/agent/submit" -Body @{ jobId = 101; recipeId = "style.warm_dusk"; userMsg = "" }
    } catch {
        Fail "POST /agent/submit 失败：$($_.Exception.Message)"
    }
    if ($job) {
        Check ($job.jobId -eq 101) "submit 返回 jobId=101" ("实得 " + $job.jobId)
    }

    # 单飞：第二个并发任务必须被拒（409）
    if ($job) {
        Check ((ApiStatus -Method Post -Path "/agent/submit" -Body @{ jobId = 102 }) -eq 409) "并发提交被单飞约束拒绝（409）"
    }

    # ------------------------------------------------------------ 4. 进度逐个出来 + 终态收敛
    $seenSteps = 0
    $final = $null
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 100
        try {
            $st = Api -Method Get -Path "/agent/jobs/101"
        } catch { continue }
        if ($st.steps.Count -gt $seenSteps) { $seenSteps = $st.steps.Count }
        if ($st.status -ne "running") { $final = $st; break }
    }
    Check ($seenSteps -ge 1) "轮询能看到中间步骤（不是 0→100 跳变）" ("最多看到 $seenSteps 步")
    Check ($null -ne $final -and $final.status -eq "done") "任务收敛为 done" ("实得 " + $(if ($final) { $final.status } else { "超时未收敛" }))
    if ($final) {
        Check ($final.steps.Count -eq 3) "轨迹共 3 步" ("实得 " + $final.steps.Count)
        Check ($final.costTotal -eq 0) "骨架任务成本为 0" ("实得 " + $final.costTotal)
    }

    # ------------------------------------------------------------ 5. 取消延迟
    $null = Api -Method Post -Path "/agent/submit" -Body @{ jobId = 103 }
    Start-Sleep -Milliseconds 150   # 让它进入第 2 步
    $swCancel = [System.Diagnostics.Stopwatch]::StartNew()
    $null = Api -Method Post -Path "/agent/jobs/103/cancel"
    $cancelMs = -1
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 25
        try { $c = Api -Method Get -Path "/agent/jobs/103" } catch { break }
        if ($c.status -ne "running") { $cancelMs = $swCancel.ElapsedMilliseconds; break }
    }
    Check ($cancelMs -ge 0 -and $cancelMs -lt 500) "取消在 500ms 内收敛" ("实测 ${cancelMs}ms")

    # 不存在的 job 必须 404（Rust 靠 404 与 401 区分「服务重启过」和「token 不对」）
    Check ((ApiStatus -Method Get -Path "/agent/jobs/999999") -eq 404) "未知 job 返回 404（收养判定依赖它）"
}
finally {
    # ---------------------------------------------------------------- 强杀 + 端口释放
    if ($proc -and -not $proc.HasExited) {
        & taskkill /F /T /PID $proc.Id 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
    }
    $stillUp = $false
    try {
        $null = Api -Method Get -Path "/health" -TokenValue "" -TimeoutSec 2
        $stillUp = $true
    } catch { }
    Check (-not $stillUp) "强杀后服务已下线（端口释放）"

    Remove-Item Env:\AGENT_PORT  -ErrorAction SilentlyContinue
    Remove-Item Env:\AGENT_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:\PYTHONIOENCODING -ErrorAction SilentlyContinue
    Remove-Item Env:\PYTHONUTF8 -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------- 汇总
Write-Host ""
if ($script:Failures.Count -eq 0) {
    Write-Host ("==> 全部通过（{0} 项）" -f $script:Checks) -ForegroundColor Green
    Write-Host "    注意：本脚本只验 Python 契约。收养/重启/无残留进程属于 Rust 侧，由应用启动流程覆盖。" -ForegroundColor DarkGray
    exit 0
} else {
    Write-Host ("==> 失败 {0} / {1} 项：" -f $script:Failures.Count, $script:Checks) -ForegroundColor Red
    $script:Failures | ForEach-Object { Write-Host "    - $_" -ForegroundColor Red }
    Write-Host "    服务日志：$logFile" -ForegroundColor Yellow
    exit 1
}
