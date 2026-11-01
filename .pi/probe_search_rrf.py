"""复现旧/新两套配置下 RRF 融合结果的成分差异（纯离线，只读库 + 一次文本编码）。"""
import json
import sqlite3
import urllib.request
from collections import Counter
from pathlib import Path

import numpy as np

DB = Path.home() / "AppData/Roaming/com.photomanagementsys.app/photos.db"
BASE = "http://127.0.0.1:18765"
QUERY = "自行车"
K = 60.0


def embed_text(text):
    req = urllib.request.Request(
        BASE + "/embed_text",
        data=json.dumps({"text": text}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        v = np.array(json.load(r)["embedding"], dtype=np.float32)
    return v / np.linalg.norm(v)


def load(conn, sql, args=()):
    out = []
    for h, blob in conn.execute(sql, args):
        out.append((h, np.frombuffer(blob, dtype=np.float32)))
    return out


def lane(items, q, min_sim, cap):
    cos = np.stack([v for _, v in items]) @ q
    idx = [i for i in range(len(items)) if cos[i] >= min_sim]
    idx.sort(key=lambda i: (-cos[i], i))  # 与 Rust 侧 (分数降序, 行号升序) 同口径
    return [(items[i][0], float(cos[i])) for i in idx[:cap]]


def rrf(img, desc):
    score, sem, order = {}, {}, []
    for lane_items in (img, desc):
        for r, (h, c) in enumerate(lane_items):
            if h not in score:
                score[h] = 0.0
                order.append(h)
            score[h] += 1.0 / (K + r + 1.0)
            sem[h] = max(sem.get(h, c), c)
    ranked = sorted(order, key=lambda h: -score[h])  # 稳定：并列保持登记顺序
    return [(h, score[h], sem[h]) for h in ranked]


def main():
    q = embed_text(QUERY)
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    img = load(conn, "SELECT photo_hash, embedding FROM photo_embeddings WHERE model='chinese-clip-vit-b16' AND user_id=1")
    desc = load(conn, "SELECT photo_hash, embedding FROM photo_text_embeddings WHERE kind='desc'")

    paths = {}
    for h, p in conn.execute("SELECT photo_hash, path FROM photo_content_scan WHERE user_id=1"):
        paths.setdefault(h, p)
    desc_text = {}
    for h, t in conn.execute("SELECT photo_hash, text FROM photo_text_embeddings WHERE kind='desc'"):
        desc_text[h] = t
    imgset = {h for h, _ in img}

    for label, (m_img, c_img, m_desc, c_desc, page) in {
        "新（FEAT-154：阈值0.45 / 池600 / 页300）": (0.45, 600, 0.45, 600, 300),
        "旧（阈值0.30 / 池2000 / 全量3826）": (0.30, 2000, 0.30, 2000, 300),
    }.items():
        li = lane(img, q, m_img, c_img)
        ld = lane(desc, q, m_desc, c_desc)
        fused = rrf(li, ld)
        top = fused[:page]
        only_desc = sum(1 for h, _, _ in top if h not in imgset)
        print(f"\n===== {label} =====")
        print(f"图像路命中 {len(li)} · 描述路命中 {len(ld)} · 融合 {len(fused)} · 页面 {len(top)}")
        print(f"页面内「纯描述路」占比 {only_desc}/{len(top)} = {only_desc/len(top)*100:.0f}%")
        cos_top = Counter(round(s, 4) for _, _, s in top)
        print("页面内 AI 匹配度分布（top5）:", cos_top.most_common(5))
        print("前 12 条：")
        for h, sc, s in top[:12]:
            src = "图像+描述" if h in imgset else "仅描述"
            print(f"  rrf={sc:.5f} cos={s:.4f} {src}  {Path(paths.get(h,'?')).name}  desc={desc_text.get(h,'?')}")


if __name__ == "__main__":
    main()
