# ============================================================================
# Agent 侧车契约冒烟（不需要 Tauri；开发版与打包 exe 都能测）
#
# 覆盖的是"交付到用户机器上会不会死"这一类问题：
#   1. /health 可达，service/api/recipes 与宿主约定一致（收养判定依赖 api 版本）；
#   2. X-Agent-Token 真的在拦人（无 token / 错 token = 401），否则等于对全机开放；
#   3. 编译期错误（未知预设）以 400 + 可行动中文文案回给前端，而不是 500；
#   4. POST /agent/submit **真能拿到 jobId**，并且真的产出文件（不只是 /health 通）；
#   5. 进度逐步可见、终态收敛、成本恒为 0（零成本路径）；
#   6. 取消在 500ms 内收敛；未知 job 返回 404（Rust 靠它与 401 区分"服务重启过"）；
#   7. 强杀后端口释放。
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
$script:Failures = @()
$script:Checks = 0
$pyPre = @()

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
    if ($cond) { Pass $msg } else { Fail ($msg + $(if ($detail) { " —— $detail" } else { "" })) }
}

function Api {
    param(
        [string]$Method,
        [string]$Path,
        [object]$Body = $null,
        # 用 [object] 而不是 [string]：PowerShell 会把 $null 强转成空串，
        # 那样就分不出"用默认 token"和"一个都不带"
        [object]$TokenValue = $null,
        [int]$TimeoutSec = 10
    )
    $headers = @{}
    $tk = if ($null -eq $TokenValue) { $Token } else { [string]$TokenValue }
    if ($tk) { $headers["X-Agent-Token"] = $tk }
    # 不要把这个哈希命名为 $args —— 那是 PowerShell 的自动变量
    $callArgs = @{
        Uri         = "$Base$Path"
        Method      = $Method
        Headers     = $headers
        TimeoutSec  = $TimeoutSec
        ErrorAction = "Stop"
    }
    if ($null -ne $Body) {
        $callArgs["Body"] = ($Body | ConvertTo-Json -Compress -Depth 10)
        $callArgs["ContentType"] = "application/json"
    }
    return Invoke-RestMethod @callArgs
}

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

# 取错误响应体（400 的 detail 要能被人读懂，这里把它打印出来核对）
function ApiErrorBody {
    param([string]$Method, [string]$Path, [object]$Body = $null)
    try {
        $null = Api -Method $Method -Path $Path -Body $Body
        return ""
    } catch {
        # PS 5.1：Invoke-RestMethod 在非 2xx 时把响应体放在 ErrorDetails.Message，
        # 直接去读 Response.GetResponseStream() 往往已经空了（被框架消费过）。
        if ($_.ErrorDetails -and $_.ErrorDetails.Message) { return $_.ErrorDetails.Message }
        try {
            $stream = $_.Exception.Response.GetResponseStream()
            $reader = New-Object System.IO.StreamReader($stream)
            return $reader.ReadToEnd()
        } catch { return "" }
    }
}

function Wait-JobTerminal {
    param([int]$JobId, [int]$MaxMs = 20000)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $seen = 0
    while ($sw.ElapsedMilliseconds -lt $MaxMs) {
        Start-Sleep -Milliseconds 100
        try { $st = Api -Method Get -Path "/agent/jobs/$JobId" } catch { continue }
        if ($st.steps.Count -gt $seen) { $seen = $st.steps.Count }
        if ($st.status -ne "running") { return @{ snap = $st; ms = $sw.ElapsedMilliseconds; seen = $seen } }
    }
    return @{ snap = $null; ms = $MaxMs; seen = $seen }
}

# 造测试图：随机色块比纯色更难压，能真实触发质量阶梯
function New-TestImages {
    param([string]$Dir, [int]$Count, [int]$W = 1200, [int]$H = 800)
    Add-Type -AssemblyName System.Drawing
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $rand = New-Object System.Random 20261102
    $paths = @()
    for ($i = 0; $i -lt $Count; $i++) {
        $bmp = New-Object System.Drawing.Bitmap $W, $H
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.Clear([System.Drawing.Color]::FromArgb(255, ($rand.Next(0, 90)), ($rand.Next(20, 120)), ($rand.Next(40, 140))))
        for ($k = 0; $k -lt 220; $k++) {
            $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(
                255, $rand.Next(0, 256), $rand.Next(0, 256), $rand.Next(0, 256)))
            $x = $rand.Next(0, $W); $y = $rand.Next(0, $H)
            $g.FillRectangle($brush, $x, $y, $rand.Next(20, 220), $rand.Next(20, 220))
            $brush.Dispose()
        }
        $g.Dispose()
        $p = Join-Path $Dir ("shot{0}.jpg" -f $i)
        $bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Jpeg)
        $bmp.Dispose()
        $paths += $p
    }
    return $paths
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
    $probe = & $PythonExe @($pyPre) -c "import fastapi, uvicorn, httpx, PIL; print('ok')" 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "依赖缺失（fastapi/uvicorn/httpx/Pillow）。先执行：`n  $PythonExe -m pip install -r python/requirements-agent.txt`n实际输出：$probe"
    }
}

# ---------------------------------------------------------------- 造素材
$ImagesDir = Join-Path $env:TEMP ("agent-smoke-img-{0}" -f $Port)
$photos = New-TestImages -Dir $ImagesDir -Count 12
$outDir = Join-Path $env:TEMP ("agent-smoke-out-{0}" -f $Port)
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
Write-Host "    测试素材 : $($photos.Count) 张 → $ImagesDir" -ForegroundColor DarkGray
Write-Host "    端口     : $Port" -ForegroundColor DarkGray

# ---------------------------------------------------------------- 起服务
$env:AGENT_PORT = "$Port"
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
        # ⚠️ 路径带空格（D:\YUAN HAO\...）：Start-Process -ArgumentList 不会自动加引号
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
        Fail "/health 不可达（等待 ${bootMs}ms）。进程输出：$tail"
        throw "服务起不来，后续检查无法进行"
    }
    Check ($health.ok -eq $true) "/health 返回 ok=true"
    Check ($health.service -eq "pm-agent") "/health.service = pm-agent" ("实得 " + $health.service)
    Check ($health.api -eq 1) "/health.api = 1（Rust 收养判定依赖它）" ("实得 " + $health.api)
    Check ($health.recipes -ge 4) "配方目录已加载（≥4 个）" ("实得 " + $health.recipes)
    Pass ("冷启动耗时 {0}ms（预算 ≤2500ms）" -f $bootMs)

    # ------------------------------------------------------------ 2. token
    Check ((ApiStatus -Method Get -Path "/agent/jobs/1" -TokenValue "") -eq 401) "无 token 返回 401"
    Check ((ApiStatus -Method Get -Path "/agent/jobs/1" -TokenValue "wrong-token") -eq 401) "错误 token 返回 401"

    # ------------------------------------------------------------ 3. 编译期错误
    Check ((ApiStatus -Method Post -Path "/agent/submit" -Body @{ jobId = 900; recipeId = "style.nope"; paths = @($photos[0]) }) -eq 400) `
        "未知预设以 400 拒绝（编译期错误，不是 500）"
    $errBody = ApiErrorBody -Method Post -Path "/agent/submit" -Body @{ jobId = 901; recipeId = "style.nope"; paths = @($photos[0]) }
    Check ($errBody -match "未知配方" -or $errBody -match "unknown") "400 响应体带可读原因" $errBody

    # ------------------------------------------------------------ 4. 真跑一个零成本任务
    $jobBody = @{
        jobId      = 101
        recipeId   = "compress.target_kb"
        userMsg    = "压小一点"
        paths      = @($photos[0], $photos[1], $photos[2])
        outputDir  = $outDir
        runSpecOverride = @{ export = @{ targetKb = 120 } }
    }
    $sub = $null
    try { $sub = Api -Method Post -Path "/agent/submit" -Body $jobBody } catch { Fail "POST /agent/submit 失败：$($_.Exception.Message)" }
    if ($sub) {
        Check ($sub.jobId -eq 101) "submit 返回 jobId=101" ("实得 " + $sub.jobId)
        Check ($sub.p1Path -eq "P1c") "带描述的不消费预设走 P1c" ("实得 " + $sub.p1Path)

        $r = Wait-JobTerminal -JobId 101
        $snap = $r.snap
        Check ($null -ne $snap -and $snap.status -eq "done") "任务收敛为 done" ("实得 " + $(if ($snap) { $snap.status + " / " + $snap.error } else { "超时未收敛" }))
        if ($snap) {
            Check ($r.seen -ge 3) "轮询能看到逐步进度" ("最多 $($r.seen) 步")
            Check ($snap.costTotal -eq 0) "零成本路径成本恒为 0" ("实得 " + $snap.costTotal)
            Check ($snap.outputs.Count -eq 3) "产出 3 个文件" ("实得 " + $snap.outputs.Count)
            $allOk = $true
            foreach ($o in $snap.outputs) {
                if (-not (Test-Path $o)) { $allOk = $false; continue }
                if ((Get-Item $o).Length -gt (120 * 1024)) { $allOk = $false }
            }
            Check $allOk "产物真实存在且不超过目标体积"
        }
    }

    # ------------------------------------------------------------ 5. 取消延迟
    $cancelBody = @{
        jobId     = 102
        recipeId  = "card.magazine_3up"
        paths     = @($photos[3..11])
        outputDir = (Join-Path $outDir "cards")
    }
    $null = Api -Method Post -Path "/agent/submit" -Body $cancelBody
    Start-Sleep -Milliseconds 60
    $swCancel = [System.Diagnostics.Stopwatch]::StartNew()
    $null = Api -Method Post -Path "/agent/jobs/102/cancel"
    $cancelMs = -1
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 25
        try { $c = Api -Method Get -Path "/agent/jobs/102" } catch { break }
        if ($c.status -ne "running") { $cancelMs = $swCancel.ElapsedMilliseconds; break }
    }
    Check ($cancelMs -ge 0 -and $cancelMs -lt 500) "取消在 500ms 内收敛" ("实测 ${cancelMs}ms")

    # ------------------------------------------------------------ 6. 端点探针（不出网）
    #                      参数不全时必须当场判不可用，而不是先发一堆请求
    $probeBad = $null
    try {
        $probeBad = Api -Method Post -Path "/agent/probe" -Body @{ endpoint = @{ baseUrl = "https://example.invalid/v1"; model = "" } }
    } catch { }
    Check ($null -ne $probeBad -and $probeBad.ok -eq $false) "探针：缺模型名时判为不可用"
    Check ((ApiStatus -Method Post -Path "/agent/probe" -Body @{ baseUrl = "" }) -eq 400) "探针：没填地址时 400"

    # ------------------------------------------------------------ 7. 404 语义
    Check ((ApiStatus -Method Get -Path "/agent/jobs/999999") -eq 404) "未知 job 返回 404（收养判定依赖它）"
}
finally {
    if ($proc -and -not $proc.HasExited) {
        & taskkill /F /T /PID $proc.Id 2>&1 | Out-Null
        Start-Sleep -Milliseconds 400
    }
    $stillUp = $false
    try { $null = Api -Method Get -Path "/health" -TokenValue "" -TimeoutSec 2; $stillUp = $true } catch { }
    Check (-not $stillUp) "强杀后服务已下线（端口释放）"

    Remove-Item Env:\AGENT_PORT -ErrorAction SilentlyContinue
    Remove-Item Env:\AGENT_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:\PYTHONIOENCODING -ErrorAction SilentlyContinue
    Remove-Item Env:\PYTHONUTF8 -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------- 汇总
Write-Host ""
if ($script:Failures.Count -eq 0) {
    Write-Host ("==> 全部通过（{0} 项）" -f $script:Checks) -ForegroundColor Green
    Write-Host "    注意：本脚本只验 Python 契约与产物。收养/重启/无残留进程属于 Rust 侧，由应用启动流程覆盖。" -ForegroundColor DarkGray
    exit 0
} else {
    Write-Host ("==> 失败 {0} / {1} 项：" -f $script:Failures.Count, $script:Checks) -ForegroundColor Red
    $script:Failures | ForEach-Object { Write-Host "    - $_" -ForegroundColor Red }
    Write-Host "    服务日志：$logFile" -ForegroundColor Yellow
    exit 1
}
