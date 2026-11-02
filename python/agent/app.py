"""FastAPI 应用：4 个端点，全部只在 127.0.0.1。

契约（docs/v5.md §5，只在回环上的唯一约定）：
    GET  /health                        → {ok, service, api, planner, recipes}
    POST /agent/submit                  → {jobId}
    GET  /agent/jobs/{id}               → AgentStatus
    POST /agent/jobs/{id}/cancel        → {ok}

`/health` 免 token（宿主探活用，不记日志），其余三个必须带 `X-Agent-Token`。

## 执行模型（本版本）

submit 时**同步编译**（配方查找 + 规格合并 + 冲突裁决 + P1a 规则映射），
编译不过就当场 400，把可行动的中文原因回给前端；编译通过后建 job，
后台按 RunSpec 确定性执行（P0/P1a/P1c 三条零成本路径）。

为什么编译要在 submit 里同步做：这三条路径的编译是纯本地计算（毫秒级），
若塞进后台，用户点「开始」后先看到 1~2 秒的"running 但没反应"，
而这恰恰是产品最要避免的观感（"点一下就出活"）。
"""
from __future__ import annotations

import asyncio
import time

from fastapi import Depends, FastAPI, HTTPException, Request

from . import API_VERSION, SERVICE_NAME
from . import logutil
from .auth import require_token
from .budget import BudgetGuard, PriceTable
from .jobs import STORE, TERMINAL, Job
from .planner import Planner
from .prompt import CompileError, CompileInput, compile_prompt
from .providers.base import Endpoint
from .providers.openai_compat import OpenAICompat
from .providers.probe import probe as probe_endpoint_cfg
from .providers.registry import resolve_size
from .recipes.schema import RecipeError, registry
from .runtime import RunEnv, apply_compile_meta, run_job
from .validators import ValidationError

app = FastAPI(title="PM Agent", docs_url=None, redoc_url=None)

STARTED_AT = time.time()

# 取消检查粒度：执行器在每张图之间查一次取消（Rust 要求 <300ms 可见）
# 单飞：同一时刻只允许一个 job 在跑（Rust 侧也有一道闸，这里是第二道）
_MAX_RUNNING = 1

# 必须持有后台任务的强引用（asyncio.create_task 的结果若无人引用，
# 任务可能在执行中被 GC 掉 —— 表现为"job 永远停在 running"）。
_TASKS: set[asyncio.Task] = set()


def _start_task(coro) -> None:
    task = asyncio.create_task(coro)
    _TASKS.add(task)
    task.add_done_callback(_TASKS.discard)


def _endpoint(raw: object) -> Endpoint | None:
    """从请求体里的 per-job 凭据造 Endpoint（明文只活在这一层与这次请求里）。"""
    if not isinstance(raw, dict):
        return None
    ep = Endpoint(
        base_url=str(raw.get("baseUrl") or ""),
        api_key=str(raw.get("apiKey") or ""),
        model=str(raw.get("model") or ""),
        unit_price=float(raw.get("unitPrice") or 0.0),
        kind=str(raw.get("kind") or "image"),
        async_poll=bool(raw.get("asyncPoll")),
    )
    return ep if ep.base_url else None


def _build_env(body: dict) -> tuple[RunEnv, Endpoint | None, Endpoint | None]:
    """构造出网环境。凭据只进内存，job 结束后随 env 一起释放。"""
    tier = str(body.get("tier") or "draft")
    table = PriceTable.from_dict(body.get("priceTable"))
    b = body.get("budget") or {}
    guard = BudgetGuard(
        max_cost=float(b.get("maxCost") or 0.0),
        max_calls=int(b.get("maxCalls") or 0),
        table=table,
        spent=float(b.get("spent") or 0.0),
        calls=int(b.get("calls") or 0),
    )

    img_ep = _endpoint(body.get("provider"))
    plan_ep = _endpoint(body.get("planner"))
    # 单端点两种活都能干：用户只填一个 kind=both 时复用
    if plan_ep is None and img_ep is not None and img_ep.kind == "both":
        plan_ep = img_ep

    provider = OpenAICompat(img_ep) if img_ep is not None else None
    planner = Planner(OpenAICompat(plan_ep), guard) if plan_ep is not None else None
    return RunEnv(
        provider=provider,
        planner=planner,
        budget=guard,
        size=resolve_size(tier),
        tier=tier,
    ), img_ep, plan_ep


# ---------------------------------------------------------------- 端点


@app.get("/health")
def health() -> dict:
    """宿主探活。不校验 token、不写日志（每次 ensure 都会打一次）。"""
    try:
        recipes = len(registry().all())
    except Exception:  # noqa: BLE001 —— 配方坏了也不能让探活失败，否则会被判"服务没起来"
        recipes = 0
    return {
        "ok": True,
        "service": SERVICE_NAME,
        "api": API_VERSION,
        "planner": False,  # 规划层接入后改为「是否已配置可用 endpoint」
        "recipes": recipes,
        "jobs": STORE.count(),
        "uptimeSec": int(time.time() - STARTED_AT),
    }


@app.post("/agent/submit", dependencies=[Depends(require_token)])
async def submit(request: Request) -> dict:
    """编译 + 建 job，立刻返回 jobId（执行在后台，宿主靠轮询拿进度）。"""
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

    paths = [p for p in (body.get("paths") or []) if isinstance(p, str) and p.strip()]

    # ① 出网环境（per-job 凭据）：先建，因为 provider_ready 会影响编译结果
    env, img_ep, plan_ep = _build_env(body)
    provider_ready = bool(img_ep or plan_ep)

    # ② 配方 + 编译（纯本地，毫秒级）。任何一条失败都当场 400，带可行动文案。
    try:
        recipe = registry().get(str(body.get("recipeId") or ""))
        compiled = compile_prompt(
            CompileInput(
                recipe=recipe,
                user_text=str(body.get("userMsg") or ""),
                tray_paths=paths,
                user_edited_prompt=body.get("userEditedPrompt"),
                person_names=[str(n) for n in (body.get("personNames") or [])],
                run_spec_override=body.get("runSpecOverride"),
                provider_ready=provider_ready,
            )
        )
        # 具体缺哪个端点在编译后才知道，这里补一次精确校验
        if recipe.needs_provider and recipe.run_spec.style and recipe.run_spec.style.engine == "remote" and img_ep is None:
            raise CompileError("该预设需要图像模型端点，但请求里没有带图像端点配置（provider）")
        if compiled.plan_pending and plan_ep is None:
            raise CompileError("该预设需要规划模型端点（planner），但请求里没有带")
    except (RecipeError, CompileError, ValidationError) as e:
        logutil.err(f"[submit] 编译失败 recipe={body.get('recipeId')!r}: {e}")
        raise HTTPException(status_code=400, detail=str(e)) from e

    if STORE.running_count() >= _MAX_RUNNING:
        raise HTTPException(status_code=409, detail="已有 Agent 任务在运行（单飞约束）")

    # ② 建 job，把编译结果先落进快照（前端立刻能看到"这次走 P1a / 已映射强度"）
    job = STORE.create(job_id, recipe.id)
    apply_compile_meta(job, compiled)
    _start_task(run_job(job, recipe, compiled, body, env))
    return {"jobId": job_id, "p1Path": compiled.p1_path}


@app.post("/agent/probe", dependencies=[Depends(require_token)])
async def probe_endpoint(request: Request) -> dict:
    """端点探针（第 5 个端点，只用于“填完地址点测试”）。

    为什么它必须在 Python 而不是 Rust：探针要测的就是**真实调用会走的那段客户端代码**，
    在 Rust 里另写一份 HTTP 探测，等于测了一条不是产品用的路径。
    它不扩大权限面：不出图（除非 deep）、不落盘、不需要 DB。
    """
    try:
        body = await request.json()
    except Exception as e:  # noqa: BLE001
        raise HTTPException(status_code=400, detail=f"请求体不是合法 JSON: {e}") from e
    if not isinstance(body, dict):
        raise HTTPException(status_code=400, detail="请求体必须是 JSON 对象")
    raw = body.get("endpoint") if isinstance(body.get("endpoint"), dict) else body
    logutil.log_request("/agent/probe", raw)
    ep = _endpoint(raw)
    if ep is None:
        raise HTTPException(status_code=400, detail="没有填写模型地址")
    return await probe_endpoint_cfg(ep, deep=bool(body.get("deep")))


@app.get("/agent/jobs/{job_id}", dependencies=[Depends(require_token)])
def job_status(job_id: int) -> dict:
    """快照。job 不存在返回 **404** —— 宿主靠它与 401 区分「服务重启过」和「token 不对」。"""
    snap = STORE.snapshot(job_id)
    if snap is None:
        raise HTTPException(status_code=404, detail="job 不存在（服务可能已重启）")
    return snap


@app.post("/agent/jobs/{job_id}/cancel", dependencies=[Depends(require_token)])
def job_cancel(job_id: int) -> dict:
    """置取消标记。执行器在下一张图边界收敛为 `cancelled`。"""
    job: Job | None = STORE.get(job_id)
    if job is None:
        raise HTTPException(status_code=404, detail="job 不存在（服务可能已重启）")
    job.request_cancel()
    snap = job.snapshot()
    return {"ok": True, "status": snap["status"], "terminal": snap["status"] in TERMINAL}
