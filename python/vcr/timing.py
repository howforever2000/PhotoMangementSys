"""细粒度耗时记账（模型段级）

为什么单独一个模块（而不是写在 pipeline 里）：
  - pipeline 记的是「通道 wall 段」（det 从进入到出来多少 ms），回答不了
    「这 28ms 里是预处理、还是 ONNX 前向、还是后处理」；
  - 拆到**模型段**必须由 service 自己打点（preprocess / run / postprocess 都在
    它内部），但 service 不能 import pipeline（会造成循环依赖）⇒ 抽一个只管
    记账的基础设施模块，谁都能 import。

设计纪律：
  - **纯记账**：不参与任何判断分支，失败最多是少一行日志（record 内部吞异常）。
  - **线程安全**：/classify_batch 是 P 路并发的，累加器必须带锁。
    ⚠️ 锁只在 record 的极短临界区内持有，**绝不**包住任何推理调用 ——
    否则测出来的就是「串行推理」，数据直接作废。
  - **跟张数成对**：`take()` 同时返回样本张数，调用方才能算均摊；
    绝不跨批次拼接读数（BUG-2026-0922-006 的教训）。

段命名约定（channel.stage）：
  det.pre / det.run / det.post           YOLOv8n 预处理 / ONNX 前向 / NMS+统计
  face.pre / face.det / face.post        SCRFD 预处理 / 前向 / 解码+NMS
  face.align / face.rec                  5 点对齐 / ArcFace 前向
  face.store                             人脸库 match+写入（持锁那段）
  ocr.pre / ocr.run / ocr.post           PaddleOCR 预处理 / 前向 / DB 后处理
  clip.open / clip.pre / clip.fwd        PIL 解码 / CLIP 预处理 / 图像塔前向
"""

import threading
import time

_LOCK = threading.Lock()
_ACC: dict[str, float] = {}
_CNT: dict[str, int] = {}


def record(key: str, ms: float) -> None:
    """累加一个耗时段（异常静默吞掉，绝不打断主流程）。

    同时累加该段的**调用次数**：均摊必须按「这段自己被调用了几次」算，
    而不是整批张数 —— 例如 `face.*` 只在人脸命中时触发（可能 40%），
    `clip.open` 只统计成功解码的张数。用整批张数去除会把条件触发的
    通道平均成假的低值（这正是旧日志里 face 占比读不准的原因之一）。
    """
    try:
        with _LOCK:
            _ACC[key] = _ACC.get(key, 0.0) + float(ms)
            _CNT[key] = _CNT.get(key, 0) + 1
    except Exception:  # noqa: BLE001
        pass


class span:
    """上下文管理器：`with span("det.run"): ...` 自动记耗时。

    异常安全：即使块内抛异常也照常记账（用 try/finally），
    因为「这次推理花了多久」本身就是我们想知道的。
    """

    def __init__(self, key: str):
        self.key = key
        self.t0 = 0.0

    def __enter__(self):
        self.t0 = time.perf_counter()
        return self

    def __exit__(self, exc_type, exc, tb):
        record(self.key, (time.perf_counter() - self.t0) * 1000.0)
        return False  # 不吞异常，语义与原来完全一致


def take() -> tuple[dict[str, float], dict[str, int]]:
    """取出并清空累加器 → (各段合计 ms, 各段调用次数)。"""
    with _LOCK:
        acc, cnt = dict(_ACC), dict(_CNT)
        _ACC.clear()
        _CNT.clear()
    return acc, cnt


def fmt(ms: float) -> str:
    """毫秒格式化（纯函数）：大数取整，小数留 1~2 位。"""
    if ms >= 100:
        return f"{ms:.0f}"
    if ms >= 10:
        return f"{ms:.1f}"
    return f"{ms:.2f}"


def render_groups(acc: dict[str, float], cnt: dict[str, int],
                  groups: dict[str, tuple[str, ...]],
                  batch_keys: frozenset[str] = frozenset()) -> str:
    """按组渲染「内部明细」（纯函数，可单测）。

    groups: {"det": ("det.pre", "det.run", "det.post"), ...}
    输出形如：`det x120(pre 4.1/run 22.9/post 1.3) · face x48(det 40.2/align 2.2/rec 22.5)`

    - 每段**均摊**到自己被调用的次数（不是整批张数，理由见 record 注释）；
    - 组名后带 `x次数`：人脸这类条件触发的通道，不看次数就会把「只有 40% 的图跑了」
      误读成「每张图只花了一点」；
    - 只输出**有数据**的段：某通道未触发时不刷一堆 0.00；
    - `batch_keys`：这些段是**整批只调用一次**的（典型是 CLIP 图像塔——一次前向吃
      掉整批 N 张，而不是逐张跑）。它们的 `acc/cnt` 是「整批合计」，若直接挂在
      `xN` 下面会被读成「每张 958ms」。这类段额外折算成每张值：
      `fwd 958/批≈120/张`。
    """
    out: list[str] = []
    for gname, keys in groups.items():
        parts: list[str] = []
        # 组内次数必须先算完：批级段的 cnt 是 1，要拿组内最大次数去折每张值
        calls = max((cnt.get(k, 0) for k in keys), default=0)
        for k in keys:
            v = acc.get(k)
            c = cnt.get(k, 0)
            if v is None or c <= 0:
                continue
            short = k.split(".", 1)[1]
            if k in batch_keys:
                # 整批合计 + 折算每张（calls=0 时无从折算，只给合计）
                per = f"≈{fmt(v / calls)}/张" if calls > 0 else "/批"
                parts.append(f"{short} {fmt(v / c)}/批{per}")
            else:
                parts.append(f"{short} {fmt(v / c)}")
        if parts:
            out.append(f"{gname} x{calls}({'/'.join(parts)})")
    return " · ".join(out)
