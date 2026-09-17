"""端到端语义搜索质量验证：中文查询 → top-10 → 对照 photo_content_scan 分类标签

链路：embeddings.npy(全库11372) + paths.json → 查询文本编码 → 余弦 top-10
      → 缩略图文件名 join photo_thumb_cache.thumb_path → photo_hash → photo_content_scan.category
输出：每条查询 top-10 的 AI 分类分布（语义合理性人工判读 + 附前几张供抽查）
"""
import json
import os
import sqlite3
import sys

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
MD = os.path.join(HERE, "..", "models", "chinese-clip-vit-b-16")
DB = os.path.join(os.environ["APPDATA"], "com.haoyuan.photo-management-sys", "photos.db")

QUERIES = ["海边日落", "一只猫", "城市夜景", "朋友聚餐", "人物自拍特写",
           "樱花盛开", "文件文档截图", "小狗", "街道行人", "雪山风景"]

import onnxruntime as ort  # noqa: E402
from minitok import MiniBertTok  # noqa: E402


def main():
    emb = np.load(os.path.join(HERE, "out", "embeddings.npy"))
    paths = json.load(open(os.path.join(HERE, "out", "paths.json")))
    name2row = {os.path.basename(p): i for i, p in enumerate(paths)}
    print(f"向量库: {emb.shape}")

    con = sqlite3.connect(DB)
    # 缩略图文件名 → photo_hash → category/label
    rows = con.execute("select thumb_path, photo_hash from photo_thumb_cache").fetchall()
    thumb2hash = {os.path.basename(tp): h for tp, h in rows}
    meta = {}
    for h, cat, sub, lab in con.execute(
            "select photo_hash, category, sub_category, label from photo_content_scan"):
        meta[h] = (cat, sub, lab)

    sess = ort.InferenceSession(os.path.join(MD, "clip_text_b16.onnx"),
                                providers=["DmlExecutionProvider"])
    tok = MiniBertTok(os.path.join(MD, "vocab.txt"))
    for q in QUERIES:
        ids, mask = tok.encode([q])
        feeds = {sess.get_inputs()[0].name: ids,
                 sess.get_inputs()[1].name: mask}
        qv = sess.run(None, feeds)[0].astype(np.float32)
        qv /= np.linalg.norm(qv)
        sims = emb @ qv[0]
        top = np.argsort(-sims)[:10]
        cats, hits = [], []
        for r in top:
            h = thumb2hash.get(os.path.basename(paths[r]))
            m = meta.get(h, ("<无扫描>", "", ""))
            cats.append(m[0] or "<空>")
            hits.append((os.path.basename(paths[r]), round(float(sims[r]), 3), m[0], m[1]))
        dist = {}
        for c in cats:
            dist[c] = dist.get(c, 0) + 1
        print(f"\n「{q}」 top10 相似度 {sims[top[0]]:.3f}~{sims[top[-1]]:.3f}  分类分布: {dist}")
        for i, (fn, s, c, sub) in enumerate(hits[:3]):
            print(f"   #{i+1} {s}  [{c}/{sub}]  {fn}")


if __name__ == "__main__":
    main()
