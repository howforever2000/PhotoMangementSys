"""照片级并行执行器（工坊批量链路）

## 为什么需要

工坊的每个 workflow 都是 `for i, path in enumerate(paths): 读图 → 算子 → 落盘`
的逐张串行循环（`workflows/*.py`）。单张链路的活（解码 / numpy 全图运算 / 编码）
会释放 GIL，所以**多张并行**能真正吃满多核；串行时机器只用到 1~2 核。

实测（本机 6P+8E / 12 张 24MP JPEG，「读图 → 自适应调色 → 落盘 JPEG」）：

    A. 串行            6410 ms  = 1068 ms/张
    B. 照片并行 P=2    3885 ms  =  647 ms/张   1.65x
    B. 照片并行 P=4    3073 ms  =  512 ms/张   2.09x
    B. 照片并行 P=7    2754 ms  =  459 ms/张   2.33x

**但写盘会把收益吃掉**：同一批 12 张并发写同一个目录时测得只有 1.02x ——
同一块盘上的并行 `write` 会互相排队，写盘成了新的串行点。把「落盘」换成内存编码
后（只测解码 + 算子这段能并行的部分）复测：

    单线程 618 ms/张 · P=2 1.48x · P=4 1.72x · **P=6 1.95x**（结果逐字节一致）

所以：**并行收益体现在 CPU 段（解码 / numpy 全图运算 / 编码），不体现在磁盘带宽上。**
这也是为什么这里不额外做「并行写盘」的优化 —— 真正的瓶颈在磁盘，不在线程数。
用户看到的端到端加速会介于 1.0x（机械盘、大盘顺序写）与 1.95x（SSD）之间。

## 为什么要有「全局限额」而不是每个 workflow 各开一个池

`jobs.py` 的作业池是 `JOB_WORKERS=2`（两个工具可以同时跑）。若每个 workflow 内部
再各开 P 个线程，最坏情况是 `2 × P` 张图同时在跑：24MP 图每张光 RGB 就 72MB，
P=6 时 12 张并发会到 GB 级，和宿主的缩略图/扫描抢内存，反而更慢。

所以并行度分两层：
  - **每个 workflow 一个池**（`map_photos`），池大小 = `workers`；
  - **全进程一个名额闸**（`_SLOTS`），槽位数 = `photo_slots()`。
线程阻塞在闸上不占 CPU，所以「池小、闸大」不会浪费，而「池大、闸小」也不会超订。

## 与 GPU 串行锁的关系

`gpu_lock.GPU_RUN_LOCK` 是 DML 设备级硬约束（见 `knowledge/gpu-dml-constraints.md`），
不能拆。照片并行后，多张图的推理会在锁上排队 —— 这不是退步：排队期间**其它线程
的解码 / numpy 运算 / 编码仍在推进**，重叠掉的正是原先串行时的空等。
"""
from __future__ import annotations

import threading
from typing import Any, Callable, List

from . import cputopo

# 单张照片的峰值内存粗估（24MP RGB = 72MB，加上 float32 中间层按 3× 算）。
# 只在 `photo_slots` 里用于说明「为什么槽位不能开太大」，不做运行时测量。
_EST_BYTES_PER_PHOTO = 220 * 1024 * 1024

_DEFAULT_SLOTS = max(1, min(cputopo.threads(), 8))
_SLOTS = threading.BoundedSemaphore(_DEFAULT_SLOTS)


def photo_slots() -> int:
    """全进程同时处理的照片上限（= min(线程数口径, 8)）。"""
    return _DEFAULT_SLOTS


def workers_for(onnx_heavy: bool = False) -> int:
    """单批的并行线程数。

    - 纯 CPU 算子（胶片 / 自适应调色 / 参考图调色）：GIL 释放充分，取满槽位；
    - 带 ONNX 的链路（磨皮 / 形变 / 超分 / OCR）：每个会话自己还有 intra_op 线程，
      两层叠满会超订 —— 实测 ORT 并发只有 1.66x（P=4）而线程开销翻倍。
        取一半，下限 2。这是本仓库既有纪律的同一取舍：VCR
        `config.PHOTO_PARALLEL=6` 配 `intra_op=6~8`，`clip_threads()` 取「一半但设下限」。
    """
    slots = photo_slots()
    if onnx_heavy:
        return max(2, min(slots, slots // 2 + 1))
    return slots


class PhotoFailed:
    """单张失败占位：保留出错原因，由 workflow 转成 warning 继续下一张。"""

    __slots__ = ("error",)

    def __init__(self, error: BaseException | str):
        self.error = error

    def reason(self) -> str:
        e = self.error
        return str(e) if not isinstance(e, BaseException) else f"{type(e).__name__}: {e}"


def map_photos(
    paths: List[str],
    fn: Callable[[int, str], Any],
    cancel=None,
    workers: int | None = None,
    on_done: Callable[[int, int], None] | None = None,
) -> List[Any]:
    """按 `paths` 顺序返回 `fn(i, path)` 的结果；单张抛异常记为 `PhotoFailed`。

    编排纪律（调用方不用关心）：
      - **保序**：结果下标与 `paths` 一一对应，落盘/回填进度都按原顺序；
      - **取消**：每张开工前查 `cancel`，已取消的剩余项不再提交；
      - **限额**：真正的并发受 `photo_slots()` 全局闸约束，跨 workflow 共享；
      - **不吞异常**：`fn` 抛出的异常只影响它自己那一张，且原文保留在
        `PhotoFailed.error` 里（由 workflow 决定是记 warning 还是记失败）。

    `on_done(已完成数, 总数)` 只在成功/失败都计入后按完成顺序回调一次 ——
    它是给 `progress()` 用的，不保证有序（完成顺序天然乱序）。
    """
    total = len(paths)
    if total == 0:
        return []

    results: List[Any] = [None] * total
    done = 0
    done_lock = threading.Lock()
    n_workers = max(1, min(int(workers or photo_slots()), total))

    # 单张任务：先抢全局名额，再干活
    def _task(index: int, path: str) -> None:
        nonlocal done
        if _is_cancelled(cancel):
            results[index] = _CancelledSlot()
        else:
            with _SLOTS:
                if _is_cancelled(cancel):
                    results[index] = _CancelledSlot()
                else:
                    try:
                        results[index] = fn(index, path)
                    except BaseException as e:  # noqa: BLE001 —— 单张失败不拖垮整批
                        results[index] = PhotoFailed(e)
        with done_lock:
            done += 1
            finished = done
        if on_done is not None:
            on_done(finished, total)

    if n_workers == 1:
        # 单线程零开销路径：小批量（1~2 张）时不必付线程池的创建成本
        for i, p in enumerate(paths):
            _task(i, p)
        return results

    pending: List[tuple[int, str]] = list(enumerate(paths))
    pending.reverse()  # pop() 取头部 → 保持提交顺序
    pending_lock = threading.Lock()

    def _runner() -> None:
        while True:
            with pending_lock:
                if not pending:
                    return
                index, path = pending.pop()
            _task(index, path)

    threads = [
        threading.Thread(target=_runner, name=f"ai-photo-{i}", daemon=True)
        for i in range(n_workers)
    ]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    # 取消后未开工的位置（理论上 _task 都会走到，这里只是兜底成「已取消」）
    for i, r in enumerate(results):
        if r is None:
            results[i] = _CancelledSlot()
    return results


class _CancelledSlot:
    """因取消而未处理：workflow 据此停止落盘但不报失败。"""

    __slots__ = ()

    def reason(self) -> str:
        return "已取消"


def _is_cancelled(cancel) -> bool:
    return cancel is not None and cancel.is_set()


def is_failed(slot: Any) -> bool:
    return isinstance(slot, PhotoFailed)


def is_cancelled(slot: Any) -> bool:
    return isinstance(slot, _CancelledSlot)


__all__ = [
    "PhotoFailed",
    "is_cancelled",
    "is_failed",
    "map_photos",
    "photo_slots",
    "workers_for",
]
