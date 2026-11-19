"""ONNX 会话串行执行锁（DirectML 设备级约束的收口处）。

为什么需要（.dsh/knowledge/gpu-dml-constraints.md §「DML 串行锁是设备级硬约束」）：
    DirectML 的非线程安全不在「同一会话内」，而在「同一 D3D12 设备上下文」层面 ——
    多个 DML 会话并发 `run` 会踩同一份资源，表现是随机 `ACCESS_VIOLATION` /
    进程直接消失。ai_studio 里「作业 worker ×2 + 预览线程池」都能同时进推理，
    所以每个 `sess.run` 都必须经过这里串行化。

为什么单独成模块（而不是放进 localmodels）：
    `localmodels` 顶层 import `facedetect`，而 `facedetect` 也需要这把锁；
    锁若定义在 `localmodels` 里就成了 `facedetect → localmodels → facedetect` 的环。
    本模块只依赖 onnxruntime 的会话对象（鸭子类型），不 import 任何业务模块。

策略：
    - 只有**实测绑到非 CPU provider** 的会话才加锁（`sess.get_providers()`）；
      纯 CPU 会话不过锁，零开销。
    - provider 信息读不到时**保守当作 GPU**：错锁的代价只是串行一档，
      漏锁的代价是段错误（与 `python/vcr/model_registry.py::_gpu_backed` 同口径）。
"""
from __future__ import annotations

import threading

GPU_RUN_LOCK = threading.Lock()


def is_gpu(sess) -> bool:
    """该会话是否绑到非 CPU provider（决定是否需要串行）。"""
    try:
        provs = list(sess.get_providers())
    except Exception:  # noqa: BLE001 —— 拿不到事实就保守加锁
        return True
    if not provs:
        return True
    return any(not str(p).startswith("CPU") for p in provs)


def run(sess, feed: dict):
    """串行执行一次 `sess.run`；CPU 会话不加锁。"""
    if is_gpu(sess):
        with GPU_RUN_LOCK:
            return sess.run(None, feed)
    return sess.run(None, feed)
