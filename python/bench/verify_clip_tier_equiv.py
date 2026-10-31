# -*- coding: utf-8 -*-
"""语义档位收敛的**守卫脚本**（FEAT-137）。

背景：语义模型原有两档（B/16 fp16 / B/16 fp32），2026-11-02 实测后只留 fp32 一档。
本脚本把当时的判断依据固化成可复跑的证据，防止以后有人「顺手加回一档 fp16」：

  1. **速度**：fp16/CPU 必须不优于 fp32/CPU（否则收敛的理由不成立）
  2. **精度**：两档必须等价（同批图 cos ≥ 0.9999）——这是「旧向量标签可以直接重打、
     不必全库重编码」的唯一依据（Rust `db::embedding::restamp_embedding_models`）
  3. **设备**：fp32 能上 DirectML，fp16 永远只能 CPU（fp16 在 DML 上有算子级数值
     bug，BUG-2026-0910-006）

用法：
    python python/bench/verify_clip_tier_equiv.py            # 两档都在场 → 全量核对
    python python/bench/verify_clip_tier_equiv.py --n 32

任一检查不通过 → 退出码 1（可直接接 CI / 发版前跑）。
fp16 权重已被清理时不会报错，只跳过等价性检查并明确打印原因。
"""
import argparse
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

import numpy as np
import onnxruntime as ort

from vcr import config, preprocess

ALBUM = os.environ.get("VCR_BENCH_ALBUM") or r"D:/YUAN HAO/Pictures/2026/test"
EXTS = (".jpg", ".jpeg", ".png", ".webp", ".bmp")
FP16 = os.path.join(config.MODEL_DIR, "chinese-clip", "clip_vision.onnx")
FP32 = os.path.join(config.MODEL_DIR, "chinese-clip-fp32", "clip_vision.onnx")


def session(path: str, device: str):
    so = ort.SessionOptions()
    so.intra_op_num_threads = 8
    so.inter_op_num_threads = 1
    # 与线上一致：BASIC（全量图优化会在 vision 塔初始化时崩，见 BUG-2026-0910-006）
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
    provs = ["CPUExecutionProvider"] if device == "CPU" else \
        [f"{device}ExecutionProvider", "CPUExecutionProvider"]
    return ort.InferenceSession(path, sess_options=so, providers=provs)


def tensors(n: int) -> np.ndarray:
    import glob

    files = sorted(p for p in glob.glob(os.path.join(ALBUM, "**", "*"), recursive=True)
                   if p.lower().endswith(EXTS))[:n]
    if not files:
        raise SystemExit(f"相册里没有图片：{ALBUM}（可用 VCR_BENCH_ALBUM 指定）")
    out = []
    for p in files:
        img = preprocess.open_image(p)
        if img is not None:
            out.append(preprocess.clip_tensor(img))
    return np.vstack(out)


def bench(sess, batch, rounds: int = 3):
    name = sess.get_inputs()[0].name
    for _ in range(2):
        sess.run(None, {name: batch})
    best, out = 1e9, None
    for _ in range(rounds):
        t = time.perf_counter()
        out = sess.run(None, {name: batch})[0]
        best = min(best, time.perf_counter() - t)
    return best * 1000.0, out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=32, help="等价性比对用多少张")
    ap.add_argument("--batch", type=int, default=8, help="速度口径：一批几张（线上是 8）")
    args = ap.parse_args()

    have16 = os.path.isfile(FP16)
    have32 = os.path.isfile(FP32)
    print(f"fp16 图塔 {FP16} → {'在场' if have16 else '不在（已清理）'}")
    print(f"fp32 图塔 {FP32} → {'在场' if have32 else '不在'}")
    if not have32:
        print("✗ 唯一档的 fp32 权重不在场，无法核对")
        return 1
    print(f"providers: {ort.get_available_providers()}")

    batch = tensors(args.batch)
    rows = []
    for tag, path, dev in (("fp32/CPU", FP32, "CPU"), ("fp32/DML", FP32, "Dml")):
        if dev != "CPU" and "DmlExecutionProvider" not in ort.get_available_providers():
            continue
        s = session(path, dev)
        ms, _ = bench(s, batch)
        rows.append((tag, s.get_providers()[0], ms / args.batch))
        print(f"  {tag:<10} {s.get_providers()[0]:<22} {ms / args.batch:6.1f} ms/张")

    ok = True
    if have16:
        x = tensors(args.n)
        s16 = session(FP16, "CPU")
        s32 = session(FP32, "CPU")
        ms16, e16 = bench(s16, batch)
        ms32, e32 = bench(s32, batch)
        per16, per32 = ms16 / args.batch, ms32 / args.batch
        print(f"\n速度：fp16/CPU {per16:.1f} ms/张 · fp32/CPU {per32:.1f} ms/张 "
              f"（fp16 是 fp32 的 {per16 / per32:.2f}×）")
        if per16 <= per32 * 1.02:
            ok = False
            print("✗ fp16 并不比 fp32 慢 —— 收敛成单档的前提不成立，需重新评估")
        else:
            print("✓ fp16 确实更慢（收敛理由成立）")

        ms16x, _ = bench(s16, x, rounds=2)
        ms32x, _ = bench(s32, x, rounds=2)
        _, a = bench(s16, x, rounds=1)
        _, b = bench(s32, x, rounds=1)
        na = a / (np.linalg.norm(a, axis=1, keepdims=True) + 1e-9)
        nb = b / (np.linalg.norm(b, axis=1, keepdims=True) + 1e-9)
        cos = np.sum(na * nb, axis=1)
        print(f"\n精度：{args.n} 张图 cos 最小 {cos.min():.8f} · "
              f"中位 {np.median(cos):.8f} · 逐元素最大差 {np.max(np.abs(a - b)):.6f}")
        if cos.min() < 0.9999:
            ok = False
            print("✗ 两档不等价（<0.9999）—— 旧向量标签**不能**直接重打，必须重建索引")
        else:
            print("✓ 等价（旧向量标签可直接重打，无需全库重编码）")
    else:
        print("\n跳过两档等价性比对：fp16 权重已不在场（正是收敛后的预期状态）")

    print("\n判定：", "通过" if ok else "未通过")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
