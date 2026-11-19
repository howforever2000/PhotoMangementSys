"""CPU 拓扑探测（P 核 / E 核）—— 线程口径的唯一来源

## 为什么需要

`os.cpu_count() // 2` 是「按逻辑核猜物理核」，它只在「每个物理核都有超线程、
且核心同质」时成立。Intel 从 Alder Lake 起是**大小核且桌面版无超线程**：

    本机 Intel Ultra 5 245K → `os.cpu_count() == 14`，`14 // 2 == 7`，
    但真实是 6 个 P 核 + 8 个 E 核。

ONNX Runtime 的 intra_op 线程越过 P 核后会溢到 E 核，而 ORT 的 spin-wait 在 E 核上
会把 P 核一起拖垮。实测（同一台 245K，`python/vcr/config.py` 记录，BUG-2026-1027-003）：

    det 单次推理  6 线程 32.8ms · 8 线程 39.9ms · 10 线程 54.2ms · 14 线程 110.2ms

即「线程越多越慢」。所以默认值必须取 **P 核数**，而不是逻辑核数或它的一半。

## 为什么本模块是 VCR `_cpu_topology()` 的精简副本而不直接 import

`ai_studio` 与 `vcr` 是两个独立进程、独立打包（`ai-studio.spec` / `vcr-server.spec`）。
`vcr` 的模块顶部会拉起 OpenVINO / 设备拓扑 / 模型注册表，`ai_studio` 一旦 import 它，
就把整条 VCR 运行时拖进了工坊进程。项目里已有同类先例：`ai_studio/facedetect.py`
就是 `vcr/services/face_service.py` 解码逻辑的「跨微服务独立精简版」。

**两处口径必须保持一致**，改这里时同步看 `python/vcr/config.py::_cpu_topology`。

## 取值纪律

    threads = min(P 核数, 逻辑核数 // 2)

- 取 P 核数是硬上限（越过就溢到 E 核，越加越慢）；
- 同时不超过逻辑核一半：给宿主的 UI / 缩略图 / DB / 另一个 Python 服务留余量。
  本机 `min(6, 7) = 6`；8C/16T 全大核 `min(8, 8) = 8`；4C/4T 小机器 `min(4, 2) = 2`。
- 拿不到拓扑时回落 `logical // 2`（不比旧口径差）。
"""
from __future__ import annotations

import os
import struct

# 线程数硬边界：下限 2（1 线程在 24MP 图上是纯亏），上限 16（再多只会抢内存带宽）
THREADS_MIN = 2
THREADS_MAX = 16

_topo_cache: tuple[int, int, int] | None = None


def logical_cores() -> int:
    """逻辑核数（含超线程）。"""
    return os.cpu_count() or 4


def _read_windows_topology() -> tuple[int, int, int]:
    """Windows：读 GetLogicalProcessorInformationEx(RelationProcessorCore)。

    每个核带 EfficiencyClass（越大越强），据此区分 P / E 核。
    返回 (物理核数, P 核数, E 核数)；任何异常都抛出去，由调用方回落。
    """
    import ctypes
    from ctypes import wintypes

    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    fn = k32.GetLogicalProcessorInformationEx
    # RelationProcessorCore = 0
    buf = ctypes.create_string_buffer(0)
    length = wintypes.DWORD(0)
    fn(0, buf, ctypes.byref(length))
    if length.value <= 0:
        raise OSError("GetLogicalProcessorInformationEx 未给出长度")
    buf = ctypes.create_string_buffer(length.value)
    if not fn(0, buf, ctypes.byref(length)):
        raise OSError("GetLogicalProcessorInformationEx 调用失败")

    total = 0
    classes: dict[int, int] = {}
    off = 0
    raw = buf.raw
    while off < length.value:
        # SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX: DWORD Relationship; DWORD Size; <union>
        rel, size = struct.unpack_from("II", raw, off)
        if size <= 0:
            break
        if rel == 0:  # RelationProcessorCore
            # PROCESSOR_RELATIONSHIP: BYTE Flags; BYTE EfficiencyClass; BYTE Reserved[20]; ...
            eff = raw[off + 8 + 1]
            classes[eff] = classes.get(eff, 0) + 1
            total += 1
        off += size
    if total <= 0:
        raise OSError("未解析到任何物理核")
    if len(classes) == 1:
        # 同质核（AMD 全大核 / 老 Intel / 无超线程）→ 物理核 = 总核
        return total, total, 0
    best = max(classes)
    p = classes[best]
    return total, p, total - p


def cpu_topology() -> tuple[int, int, int]:
    """(物理核数, P 核数, E 核数)；进程内只探一次。"""
    global _topo_cache
    if _topo_cache is not None:
        return _topo_cache
    guess = max(1, logical_cores() // 2)
    topo = (guess, guess, 0)
    if os.name == "nt":
        try:
            topo = _read_windows_topology()
        except Exception:  # noqa: BLE001 —— 探测失败不致命，回落旧口径
            topo = (guess, guess, 0)
    _topo_cache = topo
    return topo


def p_cores() -> int:
    """P 核数（同质核机器上等于物理核数）。"""
    return cpu_topology()[1]


def threads() -> int:
    """生效线程数 = min(P 核数, 逻辑核 // 2)，夹在 [THREADS_MIN, THREADS_MAX]。

    环境变量 `AI_STUDIO_THREADS` 可覆盖（A/B 对照与排障用，不暴露 UI）。
    """
    raw = (os.environ.get("AI_STUDIO_THREADS") or "").strip()
    if raw.isdigit():
        return max(THREADS_MIN, min(THREADS_MAX, int(raw)))
    n = min(p_cores(), max(1, logical_cores() // 2))
    return max(THREADS_MIN, min(THREADS_MAX, n))


def small_model_threads() -> int:
    """小模型（SCRFD / peppa / BiSeNet / OCR rec）的 intra_op 线程数。

    这些模型单次前向只有 3~60ms，线程铺满时**线程调度与同步开销占大头**
    （实测 SCRFD 640 单次：6 线程已接近最优，再高纯亏）。取 `threads()` 的一半，
    下限 2 —— 与 `vcr.config.clip_threads()` 的「一半但设下限」同一条纪律。
    """
    return max(2, min(threads(), threads() // 2))


def info() -> dict:
    """给日志/健康检查的结构化快照（不含任何路径）。"""
    physical, p, e = cpu_topology()
    return {
        "logical": logical_cores(),
        "physical": physical,
        "p_cores": p,
        "e_cores": e,
        "hybrid": e > 0,
        "threads": threads(),
        "small_model_threads": small_model_threads(),
    }


__all__ = [
    "THREADS_MAX",
    "THREADS_MIN",
    "cpu_topology",
    "info",
    "logical_cores",
    "p_cores",
    "small_model_threads",
    "threads",
]
