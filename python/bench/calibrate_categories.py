"""P0 校准：真实缩略图库上标定「语义分类」默认阈值与关键词策略

输入：python/bench/out/embeddings.npy（11372 张真实缩略图的 Chinese-CLIP B/16 512 维向量）
      + 生产链路 embed_service（fp16 拆分件，CPU，与 App 运行时一致）

产出（打印到 stdout，供决策）：
  1. 后端一致性：生产 fp16 图像塔重编码 vs bench fp32 向量，余弦应 > 0.99
  2. 每个关键词的余弦分布分位（p50/p90/p95/p99/max）—— 判定「背景值」量级
  3. 每个候选分类（关键词取 max）在阈值 0.22~0.40 上的命中数
  4. ≥1 预设命中的覆盖率 + 多分类重叠（一张图命中多个分类的比例）
  5. 每个候选分类的 top-6 命中缩略图文件名（人工抽查是否能认）

用法：python python/bench/calibrate_categories.py
"""
import json
import os
import sys
import time

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from vcr.services.embed_service import get_embed_service  # noqa: E402

EMB = os.path.join(HERE, "out", "embeddings.npy")
PATHS = os.path.join(HERE, "out", "paths.json")

# 预设分类候选（关键词即用户在 UI 里会写的那种自然语言短语）
PRESETS: dict[str, list[str]] = {
    "动物": ["动物", "宠物", "野生动物"],
    "猫": ["一只猫", "小猫", "猫咪"],
    "狗": ["一只狗", "小狗", "狗狗"],
    "美食": ["美食", "食物", "菜肴", "甜点"],
    "花卉": ["花朵", "鲜花", "盛开的花"],
    "风景": ["自然风景", "山水风景", "湖泊"],
    "天空云彩": ["天空", "蓝天白云", "云彩"],
    "建筑": ["建筑物", "楼房", "城市建筑"],
    "车辆": ["汽车", "车辆", "摩托车"],
    "日落晚霞": ["日落", "晚霞", "夕阳"],
    "雪景": ["雪景", "雪山", "积雪"],
    "大海": ["大海", "海边", "海滩"],
    "夜景": ["夜景", "夜晚的城市", "灯光"],
    "文字截图": ["文字", "文档", "屏幕截图"],
    "人物": ["人物", "人像", "一个人"],
    "聚会合影": ["聚会", "合影", "一群人"],
    "儿童": ["婴儿", "小孩", "儿童"],
    "运动": ["运动", "体育比赛", "跑步"],
}

THRESHOLDS = [0.22, 0.25, 0.28, 0.30, 0.32, 0.35, 0.40]


def main() -> None:
    vecs = np.load(EMB).astype(np.float32)
    paths = json.load(open(PATHS, encoding="utf-8"))
    n = vecs.shape[0]
    print(f"库：{n} 张 · dim={vecs.shape[1]}")

    svc = get_embed_service()

    # ---- 1. 后端一致性：生产 fp16 图像塔 vs bench fp32 向量 ----
    sample_idx = [0, n // 4, n // 2, (3 * n) // 4, n - 1]
    sample_paths = [paths[i] for i in sample_idx]
    t0 = time.perf_counter()
    res = svc.embed_images(sample_paths)
    ms = (time.perf_counter() - t0) * 1000.0 / max(1, len(sample_paths))
    print(f"\n[1] 生产链路 fp16 图像塔重编码 {len(sample_paths)} 张 · {ms:.1f}ms/张")
    for i, r in zip(sample_idx, res):
        if r.get("embedding") is None:
            print(f"    行{i}: 失败 {r.get('error')}")
            continue
        v = np.asarray(r["embedding"], dtype=np.float32)
        cos = float(np.dot(v, vecs[i]))
        print(f"    行{i}: cos(fp16, fp32) = {cos:.4f}  {os.path.basename(paths[i])}")

    # ---- 2. 关键词余弦分布 ----
    print("\n[2] 关键词余弦分布（全库 11372 张）")
    print(f"{'关键词':<12}{'p50':>8}{'p90':>8}{'p95':>8}{'p99':>8}{'max':>8}  {'文本编码ms':>10}")
    kw_vecs: dict[str, np.ndarray] = {}
    for kws in PRESETS.values():
        for kw in kws:
            if kw in kw_vecs:
                continue
            t0 = time.perf_counter()
            q = svc.embed_text(kw).astype(np.float32)
            dt = (time.perf_counter() - t0) * 1000.0
            kw_vecs[kw] = q
            s = vecs @ q
            p = np.percentile(s, [50, 90, 95, 99])
            print(
                f"{kw:<12}{p[0]:>8.3f}{p[1]:>8.3f}{p[2]:>8.3f}{p[3]:>8.3f}{s.max():>8.3f}  {dt:>10.1f}"
            )

    # ---- 3/4. 分类命中数与覆盖/重叠 ----
    print("\n[3] 各分类命中数（关键词取 max）")
    header = f"{'分类':<10}" + "".join(f"{t:>8.2f}" for t in THRESHOLDS)
    print(header)
    cat_scores: dict[str, np.ndarray] = {}
    for cat, kws in PRESETS.items():
        q = np.vstack([kw_vecs[k] for k in kws])
        cat_scores[cat] = q @ vecs.T  # (K, N)
    cat_best = {c: v.max(axis=0) for c, v in cat_scores.items()}
    for cat, best in cat_best.items():
        counts = "".join(f"{int((best >= t).sum()):>8}" for t in THRESHOLDS)
        print(f"{cat:<10}{counts}")

    print("\n[4] 覆盖与重叠（阈值 0.28 / 0.30）")
    best_mat = np.vstack([cat_best[c] for c in PRESETS])  # (C, N)
    for t in (0.28, 0.30):
        hit = best_mat >= t
        cov = int(hit.any(axis=0).sum())
        per_img = hit.sum(axis=0)
        multi = int((per_img >= 2).sum())
        print(
            f"    t={t:.2f}: 至少命中 1 类 {cov}/{n} = {cov / n:.1%}；"
            f"命中≥2 类 {multi}/{n} = {multi / n:.1%}"
        )

    # ---- 5. 抽查：每个分类 top-6 命中文件名 ----
    print("\n[5] 抽查（阈值 0.28 以上，按分数降序取前 6）")
    for cat, best in cat_best.items():
        idx = np.argsort(-best)[:6]
        items = [f"{float(best[i]):.2f}:{os.path.basename(paths[i])[:38]}" for i in idx]
        print(f"  {cat}: " + " | ".join(items))


if __name__ == "__main__":
    main()
