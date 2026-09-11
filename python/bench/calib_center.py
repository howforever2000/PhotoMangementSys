"""P0b：验证「均值中心化」能否把 CLIP 余弦从"全库 0.33 背景值"变成可判定的分数

背景（P0 实测）：B/16 上不同概念的余弦尺度差异极大，且无关对也在 0.30~0.37，
单一全局绝对阈值不可用（阈值 0.30 命中 99%，0.40 又漏掉大量真阳性）。

本脚本对比三种打分：
  raw       s = cos(x, t)
  centered  s = cos(x-μ, t-μ)            （μ = 全库图像向量均值）
  txtcenter s = cos(x-μt, t-μt)          （μt = 关键词集文本均值）

产出各关键词分位分布 + top-8 命中缩略图（人工抽查精度），并把文本向量存盘复用。
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

KWS = [
    "一只猫", "一只狗", "动物", "美食", "花朵", "自然风景", "天空", "建筑物",
    "汽车", "日落", "雪景", "大海", "夜景", "文字", "文档", "屏幕截图",
    "人物", "合影", "婴儿", "跑步", "蛋糕", "咖啡", "手机", "宠物",
]
TVEC = os.path.join(HERE, "out", "kw_text_vecs.json")


def main() -> None:
    X = np.load(os.path.join(HERE, "out", "embeddings.npy")).astype(np.float32)
    Xn = X / (np.linalg.norm(X, axis=1, keepdims=True) + 1e-9)
    paths = json.load(open(os.path.join(HERE, "out", "paths.json"), encoding="utf-8"))
    mu = Xn.mean(axis=0)
    mu = mu / (np.linalg.norm(mu) + 1e-9)
    print(f"库 {Xn.shape[0]} 张；μ 模长 {np.linalg.norm(Xn.mean(axis=0)):.4f}")

    svc = get_embed_service()
    cache = {}
    if os.path.isfile(TVEC):
        cache = json.load(open(TVEC, encoding="utf-8"))
    T = {}
    for k in KWS:
        if k in cache:
            T[k] = np.asarray(cache[k], dtype=np.float32)
            continue
        T[k] = svc.embed_text(k).astype(np.float32)
    json.dump({k: v.tolist() for k, v in T.items()}, open(TVEC, "w", encoding="utf-8"))

    def centered(v, m):
        z = v - m
        return z / (np.linalg.norm(z) + 1e-9)

    Xc = Xn - mu
    Xc = Xc / (np.linalg.norm(Xc, axis=1, keepdims=True) + 1e-9)

    print("\n{:<8}{:>22}{:>22}{:>22}".format("关键词", "raw p50/p95/max", "center p50/p95/max", "center p99/命中@0.35/0.45"))
    for k in KWS:
        t = T[k] / (np.linalg.norm(T[k]) + 1e-9)
        s_raw = Xn @ t
        s_c = Xc @ centered(t, mu)
        pr = np.percentile(s_raw, [50, 95])
        pc = np.percentile(s_c, [50, 95, 99])
        n35 = int((s_c >= 0.35).sum())
        n45 = int((s_c >= 0.45).sum())
        print(
            f"{k:<8}{pr[0]:>7.3f}{pr[1]:>7.3f}{s_raw.max():>8.3f}"
            f"{pc[0]:>8.3f}{pc[1]:>7.3f}{s_c.max():>7.3f}"
            f"{pc[2]:>8.3f}{n35:>9}{n45:>7}"
        )

    print("\n[抽查 top-5 @ 中心化 0.35]")
    for k in ["一只猫", "美食", "自然风景", "汽车", "文字", "人物", "蛋糕", "跑步"]:
        t = centered(T[k], mu)
        s = Xc @ t
        idx = np.argsort(-s)[:5]
        print(f"  {k}: " + " | ".join(f"{float(s[i]):.2f}:{os.path.basename(paths[i])[:30]}" for i in idx))


if __name__ == "__main__":
    main()
