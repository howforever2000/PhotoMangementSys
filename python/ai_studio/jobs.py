"""作业管理（FEAT-152）

进程内作业注册表 + 线程池执行 + 进度/取消。服务是无状态的，
作业进度由 Rust 轮询 /jobs/{id} 拿到，再经 Tauri 事件/轮询推给前端。
"""
from __future__ import annotations

import threading
import time
import traceback
import uuid
from concurrent.futures import ThreadPoolExecutor

from . import config
from .logutil import elog, log
from .providers import preflight
from .workflows import REGISTRY
from .workflows.base import Cancelled

_jobs: dict[str, dict] = {}
_lock = threading.Lock()
_pool = ThreadPoolExecutor(max_workers=config.JOB_WORKERS, thread_name_prefix="ai-job")

_TERMINAL = ("done", "failed", "cancelled")

# 终态作业的保留时长与注册表上限：服务是常驻进程，不清理就单调增长
# （meta 里含 plans/verifications/steps，单条并不小）。取 10 分钟是为了
# 「跑完再看一眼结果」仍能拿到，同时不让一批几千张的作业永久驻留。
_JOB_TTL_SEC = 600.0
_JOBS_MAX = 200


def _sweep_locked(now: float | None = None) -> None:
    """清掉过期终态作业 + 把总量压到上限内（调用方必须已持 `_lock`）。"""
    now = time.time() if now is None else now
    for job_id in [
        jid
        for jid, j in _jobs.items()
        if j.get("status") in _TERMINAL
        and now - float(j.get("_finished_at") or now) > _JOB_TTL_SEC
    ]:
        _jobs.pop(job_id, None)
    overflow = len(_jobs) - _JOBS_MAX
    if overflow <= 0:
        return
    # 只淘汰终态里最旧的；运行中的作业不动（否则前端轮询会 404）
    terminal = sorted(
        (jid for jid, j in _jobs.items() if j.get("status") in _TERMINAL),
        key=lambda jid: float(_jobs[jid].get("_finished_at") or 0),
    )
    for job_id in terminal[:overflow]:
        _jobs.pop(job_id, None)


def _mark_terminal(job_id: str, **fields) -> None:
    """终态落库并打上完成时间（调用方必须已持 `_lock`）。"""
    j = _jobs.get(job_id)
    if j is None:
        return
    j.update(fields)
    j["_finished_at"] = time.time()


def _new_id() -> str:
    return uuid.uuid4().hex[:12]


def _providers_tag(providers) -> str:
    """已配置的 provider 压成一行摘要（不含 apiKey）：kind=model@host。

    回答「这次作业跑在哪」：云端步的 host:port、用的什么模型一眼可读；
    全空 = 纯本机（零模型或本地 ONNX）。"""
    parts: list[str] = []
    for kind in ("vision", "edit", "upscale"):
        p = getattr(providers, kind, None)
        base = (getattr(p, "baseUrl", "") or "").strip() if p else ""
        if not base:
            continue
        host = base.split("://", 1)[-1].rstrip("/")
        model = (getattr(p, "model", "") or "").strip() or "?"
        parts.append(f"{kind}={model}@{host}")
    return ",".join(parts) if parts else "本机"


def _ops_of(meta: dict) -> list[str]:
    """从作业 meta 的 plans 里收「实际执行了哪些组件（算子）」，保序去重。

    配方 / 智能修图 / 创意组合的计划都存在 meta.plans[].steps[].op；
    非计划型工作流（压缩、OCR 等本机单步）没有 plans，返回空 ——
    它们的「组件」就是 tool 本身，启动行已经带了。"""
    out: list[str] = []
    for plan in (meta or {}).get("plans") or []:
        for st in plan.get("steps") or []:
            op = str(st.get("op") or "")
            if op and op not in out:
                out.append(op)
    return out


def _stats_tag(meta: dict) -> str:
    """meta.stats 压成 `k:v,k:v` 一行（无 stats 返回空串）。"""
    stats = (meta or {}).get("stats")
    if not isinstance(stats, dict) or not stats:
        return ""
    return ",".join(f"{k}:{v}" for k, v in stats.items())


def _job_component_tag(meta: dict) -> str:
    """完成行的组件摘要尾巴：` ops=[a,b,c] stats=images:3,steps:15`（无则空串）。"""
    parts = []
    ops_list = _ops_of(meta)
    if ops_list:
        parts.append(f"ops=[{','.join(ops_list)}]")
    stats_tag = _stats_tag(meta)
    if stats_tag:
        parts.append(f"stats={stats_tag}")
    return (" " + " ".join(parts)) if parts else ""


def create(req) -> str:
    job_id = _new_id()
    cancel = threading.Event()
    with _lock:
        _sweep_locked()
        _jobs[job_id] = {
            "jobId": job_id,
            "status": "queued",
            "progress": 0,
            "stage": "排队中",
            "message": "",
            "outputs": [],
            "warnings": [],
            "error": None,
            "meta": {},
            # 结构化步骤进度（FEAT-168）：由工作流经 progress(..., steps=[...]) 上报，
            # 前端据此显示「第 N/M 步 · 耗时」；旧工作流不传时保持空数组。
            "steps": [],
            "_cancel": cancel,
        }
    fn = REGISTRY.get(req.tool)
    if fn is None:
        with _lock:
            _mark_terminal(
                job_id, status="failed", stage="失败", error=f"未知工具: {req.tool}"
            )
        return job_id

    def worker() -> None:
        t0 = time.perf_counter()  # 作业总耗时起点（FEAT-178）
        with _lock:
            _jobs[job_id]["status"] = "running"
            _jobs[job_id]["stage"] = "开始"
        log(
            f"job {job_id} 启动 tool={req.tool} n={len(req.paths)} "
            f"providers={_providers_tag(req.providers)}"
        )

        def progress(
            pct: int, stage: str, message: str = "", steps: list | None = None
        ) -> None:
            # 第 4 参可选：既有工作流仍按 3 参调用，不破坏兼容性。
            with _lock:
                j = _jobs.get(job_id)
                if j and j["status"] == "running":
                    j["progress"] = max(0, min(100, int(pct)))
                    j["stage"] = stage
                    j["message"] = message
                    if steps is not None:
                        j["steps"] = steps

        try:
            # 先探一次 provider 可达性：端口没服务时立刻失败并说清「该启动什么」，
            # 不让一批照片逐张撞同一个网络错误（BUG-2026-1122-007）。
            preflight.ensure_reachable(req.providers, req.tool)
            res = fn(req, progress, cancel)
            with _lock:
                _mark_terminal(
                    job_id,
                    status="done",
                    progress=100,
                    stage="完成",
                    outputs=res.get("outputs", []) or [],
                    warnings=res.get("warnings", []) or [],
                    meta=res.get("meta", {}) or {},
                )
            log(
                f"job {job_id} 完成 tool={req.tool} "
                f"outputs={len(_jobs[job_id]['outputs'])}"
                + _job_component_tag(_jobs[job_id].get("meta") or {})
                + f" | 总耗时 {int((time.perf_counter()-t0)*1000)}ms"
            )
        except Cancelled:
            with _lock:
                _mark_terminal(job_id, status="cancelled", stage="已取消")
            log(f"job {job_id} 已取消 tool={req.tool} | 总耗时 {int((time.perf_counter()-t0)*1000)}ms")
        except Exception as e:  # noqa: BLE001 —— 作业边界必须兜住一切，转成失败态
            elog(f"job {job_id} 失败 tool={req.tool} | 总耗时 {int((time.perf_counter()-t0)*1000)}ms: {e}\n{traceback.format_exc()}")
            with _lock:
                _mark_terminal(job_id, status="failed", stage="失败", error=str(e))

    _pool.submit(worker)
    return job_id


def get(job_id: str):
    with _lock:
        _sweep_locked()
        j = _jobs.get(job_id)
        if j is None:
            return None
        return {k: v for k, v in j.items() if not k.startswith("_")}


def cancel(job_id: str) -> bool:
    with _lock:
        j = _jobs.get(job_id)
        if j is None:
            return False
        j["_cancel"].set()
        if j["status"] not in _TERMINAL:
            j["stage"] = "正在取消…"
    return True
