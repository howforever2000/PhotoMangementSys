# -*- coding: utf-8 -*-
"""AI 腿成本结构探针：一张图的时间到底花在 CPU 还是「GPU 串行锁」上。

为什么需要它（2026-10-27 的真实教训）：只看 `[scan.timing]` 的「AI识别 139009ms」
永远不可能知道瓶颈在哪 —— 它只给总墙钟。把
  · decode / det / ocr / face 各通道均摊（pipeline.take_channel_stats）
  · GPU 会话全局锁的 **持锁合计 / 等锁合计 / 加锁次数**
放在一起看，才能区分「CPU 不够」和「GPU 被串行化」。

实测结论（245K + DML，P=6，24 张原图）：
    墙钟 11203ms · 持锁 11096ms（**99.0%**）· 加锁 55 次 · 单次持锁均摊 201.7ms
  ⇒ 墙钟几乎 100% 落在锁内，所以「decode 优化」只能小幅改善墙钟，
    真正的大头是**每一张图都要串行进的 GPU 调用次数**。
  ⇒ 由此得出的两个可落地改动：OCR 前置门（砍掉 44% 的加锁次数）、
    decode 降采样（降低 CPU 侧压力，让 DML 单次持锁更短）。

用法：
    python python/bench/pipeline_cost_probe.py <图片目录或 paths.json> [--limit 24]
                                              [--photos-parallel 6] [--gpu on|off]
    VCR_DATA_DIR 必须指向人物库副本（会写人脸），否则会被拒绝运行。
"""
import argparse
import glob
import json
import os
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

import vcr.model_registry as mr                       # noqa: E402
from vcr import config                                 # noqa: E402
from vcr.model_registry import get_registry            # noqa: E402
from vcr.services.pipeline import classify_one, take_channel_stats  # noqa: E402

EXTS = (".jpg", ".jpeg", ".png", ".webp", ".bmp")


class TimedLock:
    """给 _GPU_RUN_LOCK 包一层计时，分别累计持锁与等锁。"""

    def __init__(self, inner):
        self.inner = inner
        self._m = threading.Lock()
        self.hold = 0.0
        self.wait = 0.0
        self.n = 0
        self._t = threading.local()

    def __enter__(self):
        t0 = time.perf_counter()
        self.inner.acquire()
        w = time.perf_counter() - t0
        self._t.hold = time.perf_counter()
        with self._m:
            self.wait += w
            self.n += 1
        return self

    def __exit__(self, *exc):
        d = time.perf_counter() - self._t.hold
        with self._m:
            self.hold += d
        self.inner.release()
        return False


def collect_paths(src: str, limit: int) -> list[str]:
    if src.endswith(".json"):
        paths = json.load(open(src, encoding="utf-8"))
    else:
        paths = sorted(p for p in glob.glob(os.path.join(src, "**", "*"), recursive=True)
                       if p.lower().endswith(EXTS))
    paths = [p for p in paths if os.path.isfile(p)]
    if limit and len(paths) > limit:
        step = max(1, len(paths) // limit)
        paths = paths[::step][:limit]
    return paths


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("src", help="图片目录或 paths.json")
    ap.add_argument("--limit", type=int, default=24)
    ap.add_argument("--photos-parallel", type=int, default=None)
    ap.add_argument("--gpu", choices=("on", "off", "keep"), default="keep")
    ap.add_argument("--rounds", type=int, default=3)
    args = ap.parse_args()

    if "_tmp_vcrdata" not in os.environ.get("VCR_DATA_DIR", "") and \
            not os.environ.get("VCR_PROBE_ALLOW_WRITE"):
        print("拒绝运行：请把 VCR_DATA_DIR 指向人物库副本（本探针会写人脸库）")
        return 2

    paths = collect_paths(args.src, args.limit)
    if not paths:
        print("没有可用图片")
        return 2
    level = args.photos_parallel or config.PHOTO_PARALLEL

    reg = get_registry()
    _ = reg.det, reg.face_det, reg.face_rec, reg.ocr
    if args.gpu != "keep":
        reg.set_gpu_enabled(args.gpu == "on")
    print(f"provider={reg.gpu_info()['provider']} threads={config.threads()} "
          f"P={level} decode_min={config.DECODE_MIN_LONG} ocr_gate={config.OCR_GATE} "
          f"样本={len(paths)} 张\n")

    best = None
    for r in range(args.rounds):
        lock = TimedLock(mr._GPU_RUN_LOCK)
        mr._GPU_RUN_LOCK = lock
        try:
            take_channel_stats()
            t0 = time.perf_counter()
            with ThreadPoolExecutor(level) as ex:
                list(ex.map(lambda p: classify_one(p, reg, use_face=True), paths))
            wall = (time.perf_counter() - t0) * 1000.0
        finally:
            mr._GPU_RUN_LOCK = lock.inner
        stats, n = take_channel_stats()
        row = {k: v / max(n, 1) for k, v in stats.items()}
        row.update(wall=wall, hold=lock.hold * 1000, wait=lock.wait * 1000,
                   locks=lock.n, wall_per=wall / len(paths))
        print(f"第{r + 1}轮 wall {row['wall_per']:6.1f} ms/张 | decode {row.get('decode', 0):6.1f}"
              f" · det {row.get('det', 0):6.1f} · ocr {row.get('ocr', 0):6.1f}"
              f" · face {row.get('face', 0):6.1f} · 合计 {row.get('total', 0):7.1f}"
              f" | 持锁 {row['hold']:6.0f}ms ({row['hold'] / wall * 100:5.1f}%)"
              f" · 加锁 {row['locks']} 次 · 单次 {row['hold'] / max(row['locks'], 1):6.1f}ms")
        if best is None or row["wall_per"] < best["wall_per"]:
            best = row

    print(f"\n最好一轮：wall {best['wall_per']:.1f} ms/张 · 持锁占比 "
          f"{best['hold'] / best['wall'] * 100:.1f}%")
    if best["hold"] / best["wall"] > 0.7:
        print("⇒ 墙钟主要落在 GPU 串行锁内：优先砍「每张图的 GPU 调用次数」，"
              "而不是优化 CPU 侧。")
    else:
        print("⇒ 墙钟主要不在锁内：CPU（解码/后处理）才是地板。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
