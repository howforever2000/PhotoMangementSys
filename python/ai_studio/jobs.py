"""作业管理（FEAT-152）

进程内作业注册表 + 线程池执行 + 进度/取消。服务是无状态的，
作业进度由 Rust 轮询 /jobs/{id} 拿到，再经 Tauri 事件/轮询推给前端。
"""
from __future__ import annotations

import threading
import traceback
import uuid
from concurrent.futures import ThreadPoolExecutor

from . import config
from .logutil import elog, log
from .workflows import REGISTRY
from .workflows.base import Cancelled

_jobs: dict[str, dict] = {}
_lock = threading.Lock()
_pool = ThreadPoolExecutor(max_workers=config.JOB_WORKERS, thread_name_prefix="ai-job")

_TERMINAL = ("done", "failed", "cancelled")


def _new_id() -> str:
    return uuid.uuid4().hex[:12]


def create(req) -> str:
    job_id = _new_id()
    cancel = threading.Event()
    with _lock:
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
            "_cancel": cancel,
        }
    fn = REGISTRY.get(req.tool)
    if fn is None:
        with _lock:
            _jobs[job_id].update(status="failed", stage="失败", error=f"未知工具: {req.tool}")
        return job_id

    def worker() -> None:
        with _lock:
            _jobs[job_id]["status"] = "running"
            _jobs[job_id]["stage"] = "开始"
        log(f"job {job_id} 启动 tool={req.tool} n={len(req.paths)}")

        def progress(pct: int, stage: str, message: str = "") -> None:
            with _lock:
                j = _jobs.get(job_id)
                if j and j["status"] == "running":
                    j["progress"] = max(0, min(100, int(pct)))
                    j["stage"] = stage
                    j["message"] = message

        try:
            res = fn(req, progress, cancel)
            with _lock:
                j = _jobs[job_id]
                j["status"] = "done"
                j["progress"] = 100
                j["stage"] = "完成"
                j["outputs"] = res.get("outputs", []) or []
                j["warnings"] = res.get("warnings", []) or []
                j["meta"] = res.get("meta", {}) or {}
            log(f"job {job_id} 完成 outputs={len(_jobs[job_id]['outputs'])}")
        except Cancelled:
            with _lock:
                _jobs[job_id].update(status="cancelled", stage="已取消")
            log(f"job {job_id} 已取消")
        except Exception as e:  # noqa: BLE001 —— 作业边界必须兜住一切，转成失败态
            elog(f"job {job_id} 失败: {e}\n{traceback.format_exc()}")
            with _lock:
                _jobs[job_id].update(status="failed", stage="失败", error=str(e))

    _pool.submit(worker)
    return job_id


def get(job_id: str):
    with _lock:
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
