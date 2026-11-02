"""job 注册表的形态与收敛：**跨进程契约**的回归测试。

Rust 侧 `agent::job::PythonStatus` 用 `#[serde(rename_all = "camelCase")]` 反序列化
这里 `snapshot()` 的产物。字段名一旦改成 snake_case，Rust 那边不会报错，
只会**静默拿到 None/0** —— 表现为"进度永远空、成本永远 0"。
所以这里把字段名逐个钉住。
"""
from __future__ import annotations

from agent.jobs import KEEP_JOBS, STORE, TERMINAL, Job, JobStore


def test_snapshot_uses_camel_case_keys_for_rust_contract():
    job = Job(7, "style.warm_dusk")
    job.add_step("plan", model="deepseek-v4-flash", latency_ms=1830, cost=0.0009, note="选定 20 张")
    job.p1_path = "P1a"
    snap = job.snapshot()

    for key in (
        "jobId",
        "recipeId",
        "status",
        "p1Path",
        "steps",
        "costTotal",
        "callsUsed",
        "needReplan",
        "error",
    ):
        assert key in snap, f"快照缺字段 {key}（Rust 侧按 camelCase 取值）"

    step = snap["steps"][0]
    for key in ("idx", "tool", "model", "latencyMs", "cost", "thumb", "note"):
        assert key in step, f"轨迹缺字段 {key}"


def test_add_step_accumulates_cost_and_call_count():
    job = Job(1)
    job.add_step("plan", model="m", cost=0.001)
    job.add_step("render", model="m2", cost=0.15)
    # 无模型的一步（本地算法）不该计入调用数
    job.add_step("compress", cost=0.0)
    snap = job.snapshot()
    assert snap["callsUsed"] == 2
    assert abs(snap["costTotal"] - 0.151) < 1e-9


def test_finish_sets_terminal_status():
    job = Job(2)
    assert job.snapshot()["status"] not in TERMINAL
    job.finish("done")
    assert job.snapshot()["status"] == "done"
    assert job.snapshot()["status"] in TERMINAL


def test_cancel_flag_is_observable():
    job = Job(3)
    assert not job.is_cancelled()
    job.request_cancel()
    assert job.is_cancelled()


def test_store_snapshot_of_unknown_job_is_none():
    store = JobStore()
    assert store.snapshot(123456) is None


def test_store_running_count_tracks_terminal_transition():
    store = JobStore()
    job = store.create(1)
    assert store.running_count() == 1
    job.finish("done")
    assert store.running_count() == 0


def test_store_evicts_oldest_finished_but_never_running():
    store = JobStore()
    running = store.create(1)
    for jid in range(2, KEEP_JOBS + 6):
        store.create(jid).finish("done")
    # 触发淘汰
    store.create(999).finish("done")
    assert store.get(1) is running, "运行中的 job 不得被淘汰"
    assert store.get(2) is None, "最老的终态 job 应被淘汰"
    assert store.get(999) is not None, "最新的 job 必须保留"


def test_module_level_store_exists_for_app():
    # app.py 直接从 agent.jobs 导入 STORE（单进程单实例）
    assert STORE is not None
