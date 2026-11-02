"""内存 job 注册表：进度 / 取消 / 成本累计 / 轨迹。

设计约束（docs/v5.md §6）：
  - **快照真相在 Rust**：本模块只是"执行侧自述"，随时可能因为进程被杀而消失，
    因此不落盘、不做恢复；Rust 侧拿到 404 就知道"服务重启过，任务已丢"。
  - **取消延迟 < 200ms**：执行循环每 100ms 检查一次取消标记，
    不依赖 httpx 请求被打断（模型调用无法中途打断，只能停止后续步骤）。
  - **成本只做累加**，不在这里判断预算（预算是 budget.py 的前置检查职责）。
"""
from __future__ import annotations

import threading
import time

# 终态集合：Rust 轮询看到这些就不再轮询
TERMINAL = frozenset({"done", "error", "cancelled", "budget", "handoff"})

# 保留的历史 job 数（超出淘汰最老的终态 job；只影响内存占用）
KEEP_JOBS = 32


class Job:
    """单个 job 的可变状态。所有读写都在 `self.lock` 内。"""

    def __init__(self, job_id: int, recipe_id: str = ""):
        self.job_id = job_id
        self.recipe_id = recipe_id
        self.status = "running"
        self.steps: list[dict] = []
        self.cost_total = 0.0
        self.calls_used = 0
        self.need_replan: dict | None = None
        self.error: str | None = None
        self.p1_path: str | None = None
        self.created = time.time()
        self.updated = self.created
        self.cancelled = False
        self.lock = threading.Lock()

    # ---- 写 ----

    def add_step(
        self,
        tool: str,
        *,
        model: str | None = None,
        latency_ms: int = 0,
        cost: float = 0.0,
        thumb: str | None = None,
        note: str | None = None,
    ) -> None:
        """追加一步轨迹，并同步累加成本/调用数。"""
        with self.lock:
            self.steps.append(
                {
                    "idx": len(self.steps) + 1,
                    "tool": tool,
                    "model": model,
                    "latencyMs": int(latency_ms),
                    "cost": float(cost),
                    "thumb": thumb,
                    "note": note,
                }
            )
            self.cost_total += float(cost)
            if model:
                self.calls_used += 1
            self.updated = time.time()

    def finish(self, status: str, error: str | None = None) -> None:
        with self.lock:
            self.status = status
            self.error = error
            self.updated = time.time()

    def request_cancel(self) -> None:
        with self.lock:
            self.cancelled = True
            self.updated = time.time()

    def is_cancelled(self) -> bool:
        with self.lock:
            return self.cancelled

    # ---- 读 ----

    def snapshot(self) -> dict:
        with self.lock:
            return {
                "jobId": self.job_id,
                "recipeId": self.recipe_id,
                "status": self.status,
                "p1Path": self.p1_path,
                "steps": list(self.steps),
                "costTotal": round(self.cost_total, 6),
                "callsUsed": self.calls_used,
                "needReplan": self.need_replan,
                "error": self.error,
                "startedAt": self.created,
                "updatedAt": self.updated,
            }


class JobStore:
    """进程级 job 表（线程安全）。"""

    def __init__(self) -> None:
        self._jobs: dict[int, Job] = {}
        self._lock = threading.Lock()

    def create(self, job_id: int, recipe_id: str = "") -> Job:
        job = Job(job_id, recipe_id)
        with self._lock:
            self._jobs[job_id] = job
            self._evict_locked()
        return job

    def get(self, job_id: int) -> Job | None:
        with self._lock:
            return self._jobs.get(job_id)

    def snapshot(self, job_id: int) -> dict | None:
        job = self.get(job_id)
        return job.snapshot() if job else None

    def count(self) -> int:
        with self._lock:
            return len(self._jobs)

    def running_count(self) -> int:
        with self._lock:
            return sum(1 for j in self._jobs.values() if j.status == "running")

    def _evict_locked(self) -> None:
        """超出 KEEP_JOBS 时淘汰最老的**终态** job（运行中的永不淘汰）。"""
        if len(self._jobs) <= KEEP_JOBS:
            return
        finished = [(j.created, jid) for jid, j in self._jobs.items() if j.status in TERMINAL]
        finished.sort()
        for _, jid in finished[: len(self._jobs) - KEEP_JOBS]:
            self._jobs.pop(jid, None)


# 单进程单实例（uvicorn 以单 worker 运行，见 agent_server.py）
STORE = JobStore()
