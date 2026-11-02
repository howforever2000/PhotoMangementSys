"""FastAPI 应用：4 个端点，全部只在 127.0.0.1。

契约（docs/v5.md §5，只在回环上的唯一约定）：
    GET  /health                        → {ok, service, api, planner}
    POST /agent/submit                  → {jobId}
    GET  /agent/jobs/{id}               → AgentStatus
    POST /agent/jobs/{id}/cancel        → {ok}

`/health` 免 token（宿主探活用，不记日志），其余三个必须带 `X-Agent-Token`。

本阶段（骨架）只验证**生命周期**：submit 建 job → 后台跑一个 3 步占位任务 →
status 能看到逐步进度 → cancel 在 200ms 内生效 → 终态收敛。
真正的规划循环、工具与图像算法在后续模块接入，端点形状不变。
"""
from __future__ import annotations

import asyncio
import time

from fastapi import Depends, FastAPI, HTTPException, Request

from . import API_VERSION, SERVICE_NAME
from . import logutil
from .auth import require_token
from .jobs import STORE, TERMINAL, Job

app = FastAPI(title="PM Agent", docs_url=None, redoc_url=None)

STARTED_AT = time.time()

# 取消检查粒度：Rust 侧要求 <200ms 生效，留一倍余量
_CANCEL_POLL_SEC = 0.1

# 单飞：同一时刻只允许一个 job 在跑（Rust 侧也有一道闸，这里是第二道）
_MAX_RUNNING = 1

# 必须持有后台任务的强引用（docs：asyncio.create_task 的结果若无人引用，
# 任务可能在执行中被 GC 掉 —— 表现为“job 永远停在 running”）。
_TASKS: set[asyncio.Task] = set()


# ---------------------------------------------------------------- 生命周期工具


async def _sleep_interruptible(job: Job, seconds: float) -> bool:
    """睡 `seconds` 秒，期间每 100ms 查一次取消。返回 True 表示被取消。"""
    deadline = time.monotonic() + seconds
    while True:
        if job.is_cancelled():
            return True
        left = deadline - time.monotonic()
        if left <= 0:
            return False
        await asyncio.sleep(min(_CANCEL_POLL_SEC, left))


async def _run_skeleton(job: Job) -> None:
    """占位执行体：证明「进度 / 取消 / 终态」三条链路通。

    故意每步都真的耗时（而非瞬间完成），否则前端与轮询桥接看不到中间态，
    "进度条只从 0 跳到 100"这种问题要到接真算法时才会暴露。
    """
    plan = [
        ("plan", 0.15, "骨架：规划步骤占位（尚未接入规划模型）"),
        ("select", 0.25, "骨架：选图占位（尚未接入候选池）"),
        ("render", 0.8, "骨架：执行占位（尚未接入图像算法）"),
    ]
    try:
        for tool, cost_sec, note in plan:
            if await _sleep_interruptible(job, cost_sec):
                job.finish("cancelled", "已取消")
                logutil.err(f"[job {job.job_id}] 已在 {tool} 前取消")
                return
            job.add_step(tool, latency_ms=int(cost_sec * 1000), note=note)
        job.finish("done")
        logutil.err(f"[job {job.job_id}] done，{len(job.steps)} 步")
    except Exception as e:  # noqa: BLE001 —— 任何异常都要变成 job 的终态，不能吊死轮询
        logutil.exc(f"[job {job.job_id}] 执行异常", e)
        job.finish("error", f"{type(e).__name__}: {e}")


# ---------------------------------------------------------------- 端点


@app.get("/health")
def health() -> dict:
    """宿主探活。不校验 token、不写日志（每次 ensure 都会打一次）。"""
    return {
        "ok": True,
        "service": SERVICE_NAME,
        "api": API_VERSION,
        "planner": False,  # 规划层接入后改为「是否已配置可用 endpoint」
        "jobs": STORE.count(),
        "uptimeSec": int(time.time() - STARTED_AT),
    }


@app.post("/agent/submit", dependencies=[Depends(require_token)])
async def submit(request: Request) -> dict:
    """建 job 并立刻返回 jobId（执行在后台，宿主靠轮询拿进度）。"""
    try:
        body = await request.json()
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=400, detail=f"请求体不是合法 JSON: {e}") from e
    if not isinstance(body, dict):
        raise HTTPException(status_code=400, detail="请求体必须是 JSON 对象")

    job_id = body.get("jobId")
    if not isinstance(job_id, int) or isinstance(job_id, bool) or job_id <= 0:
        raise HTTPException(status_code=400, detail="jobId 必须是正整数")

    logutil.log_request("/agent/submit", body)

    if STORE.running_count() >= _MAX_RUNNING:
        raise HTTPException(status_code=409, detail="已有 Agent 任务在运行（单飞约束）")

    job = STORE.create(job_id, str(body.get("recipeId") or ""))
    # 不 await：立即返回 jobId，执行在事件循环里继续
    task = asyncio.create_task(_run_skeleton(job))
    _TASKS.add(task)
    task.add_done_callback(_TASKS.discard)
    return {"jobId": job_id}


@app.get("/agent/jobs/{job_id}", dependencies=[Depends(require_token)])
def job_status(job_id: int) -> dict:
    """快照。job 不存在返回 **404** —— 宿主靠它与 401 区分「服务重启过」和「token 不对」。"""
    snap = STORE.snapshot(job_id)
    if snap is None:
        raise HTTPException(status_code=404, detail="job 不存在（服务可能已重启）")
    return snap


@app.post("/agent/jobs/{job_id}/cancel", dependencies=[Depends(require_token)])
def job_cancel(job_id: int) -> dict:
    """置取消标记。执行循环在下一个 100ms 检查点收敛为 `cancelled`。"""
    job = STORE.get(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="job 不存在（服务可能已重启）")
    job.request_cancel()
    # 已经终态的 job 直接把状态改成 cancelled 也行，但保持诚实：让执行体自己收敛
    return {"ok": True, "status": job.snapshot()["status"], "terminal": job.snapshot()["status"] in TERMINAL}
