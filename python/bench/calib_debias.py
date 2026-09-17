"""P0d：prompt 去偏（background debiasing）—— 用中性提示词基线校正概念尺度差

动机（P0/P0b 实测）：
  raw 余弦的"背景值"随概念变化（动物 0.334 / 美食 0.363 / 文档 0.330 同量级但真实可分数不同），
  单一绝对阈值在 0.38 时「一只猫」边界处精度已崩（人工抽查 5 张仅 1 张有猫）。
  均值中心化（P0b）会让低信息量图片变成 hub，反而更糟。

本方案：rel = cos(x, t) - mean_b cos(x, b)   （b = 一组中性提示词）
  概念尺度差被基线吸收，rel 应可用单一阈值判定。

产出：rel 分位分布 + 固定阈值命中数 + top/边界处缩略图路径（交给视觉模型抽查）。

用法：python python/bench/calib_debias.py [rel阈值]
"""
import json
import os
import sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
from vcr.services.embed_service import get_embed_service  # noqa: E402

BACKGROUND = [
    "一张照片", "一张图片", "一张普通的照片", "随手拍的照片",
    "日常生活照片", "一张图片素材", "相册里的一张图",
]

KWS = [
    "一只猫", "一只狗", "动物", "美食", "花朵", "自然风景", "天空", "建筑物",
    "汽车", "日落", "雪景", "大海", "夜景", "文字", "文档", "屏幕截图",
    "人物", "合影", "婴儿", "跑步", "蛋糕", "手机",
]


def main() -> None:
    thr = float(sys.argv[1]) if len(sys.argv) > 1 else 0.10
    X = np.load(os.path.join(HERE, "out", "embeddings.npy")).astype(np.float32)
    X = X / (np.linalg.norm(X, axis=1, keepdims=True) + 1e-9)
    PATHS = json.load(open(os.path.join(HERE, "out", "paths.json"), encoding="utf-8"))
    svc = get_embed_service()

    cache_path = os.path.join(HERE, "out", "kw_text_vecs.json")
    cache = json.load(open(cache_path, encoding="utf-8")) if os.path.isfile(cache_path) else {}
    T = {}
    for k in KWS + BACKGROUND:
        if k in cache:
            T[k] = np.asarray(cache[k], dtype=np.float32)
            continue
        T[k] = svc.embed_text(k).astype(np.float32)
    json.dump({k: v.tolist() for k, v in T.items()}, open(cache_path, "w", encoding="utf-8"))

    def unit(v):
        return v / (np.linalg.norm(v) + 1e-9)

    B = np.vstack([unit(T[b]) for b in BACKGROUND])
    base = (X @ B.T).mean(axis=1)  # (N,) 每张图相对中性提示的平均相似度
    print(f"中性基线 base：p5={np.percentile(base, 5):.3f} p50={np.percentile(base, 50):.3f} "
          f"p95={np.percentile(base, 95):.3f}")

    print(f"\n{'关键词':<8}{'p50':>8}{'p90':>8}{'p95':>8}{'p99':>8}{'max':>8}   {'命中@%.2f' % thr:>10}")
    rels: dict[str, np.ndarray] = {}
    for k in KWS:
        rel = X @ unit(T[k]) - base
        rels[k] = rel
        p = np.percentile(rel, [50, 90, 95, 99])
        print(f"{k:<8}{p[0]:>8.3f}{p[1]:>8.3f}{p[2]:>8.3f}{p[3]:>8.3f}{rel.max():>8.3f}"
              f"{int((rel >= thr).sum()):>12}")

    print(f"\n[命中边界抽查 @ rel>={thr}]")
    for k in ["一只猫", "美食", "汽车", "文字", "人物"]:
        rel = rels[k]
        idx = np.where(rel >= thr)[0]
        if len(idx) == 0:
            print(f"  {k}: 无命中")
            continue
        print(f"  --- {k} 共 {len(idx)} 张，取前 5 与末 5 ---")
        for i in list(idx[:5]) + list(idx[-5:]):
            print(f"    {rel[i]:.3f}  {PATHS[i]}")


if __name__ == "__main__":
    main()
