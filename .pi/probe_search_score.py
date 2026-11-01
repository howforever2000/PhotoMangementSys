"""诊断：为什么「自行车」搜出一堆 71% 的不相干照片。

只读探针：
1) 从正在运行的 VCR 服务取「自行车」的文本向量（与线上同一条编码路径）
2) 读 photos.db 的 photo_embeddings（图像塔）与 photo_text_embeddings（描述塔）
3) 分别算余弦，打印分布 + topN，看两路的「绝对值尺度」是否可比
"""
import json
import sqlite3
import urllib.request
from pathlib import Path

import numpy as np

DB = Path.home() / "AppData/Roaming/com.photomanagementsys.app/photos.db"
BASE = "http://127.0.0.1:18765"
QUERY = "自行车"


def embed_text(text: str) -> np.ndarray:
    req = urllib.request.Request(
        BASE + "/embed_text",
        data=json.dumps({"text": text}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        v = np.array(json.load(r)["embedding"], dtype=np.float32)
    return v / np.linalg.norm(v)


def rows(conn, sql, args=()):
    cur = conn.execute(sql, args)
    out = []
    for r in cur:
        blob = r[1]
        v = np.frombuffer(blob, dtype=np.float32)
        out.append((r[0], v))
    return out


def report(name, items, q):
    if not items:
        print(f"[{name}] 空")
        return
    n = len(items)
    mats = np.stack([v for _, v in items])
    norms = np.linalg.norm(mats, axis=1)
    cos = mats @ q
    order = np.argsort(-cos)
    print(f"\n=== {name} · {n} 条 ===")
    print(f"向量模长 min/median/max = {norms.min():.4f} / {np.median(norms):.4f} / {norms.max():.4f}")
    pct = np.percentile(cos, [1, 10, 25, 50, 75, 90, 99])
    print("余弦分位 1/10/25/50/75/90/99 = " + " / ".join(f"{p:.3f}" for p in pct))
    for thr in (0.30, 0.35, 0.40, 0.45, 0.50):
        print(f"  >= {thr}: {(cos >= thr).sum()} 条 ({(cos >= thr).mean()*100:.1f}%)")
    print("top15:")
    for i in order[:15]:
        h, _ = items[i]
        print(f"  {cos[i]:.4f}  {h}")


def main():
    q = embed_text(QUERY)
    print(f"查询「{QUERY}」文本向量 dim={q.shape[0]} |q|={np.linalg.norm(q):.6f}")
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    model = conn.execute(
        "SELECT model, COUNT(*) FROM photo_embeddings GROUP BY model"
    ).fetchall()
    print("photo_embeddings 档位分布:", model)
    model_id = model[0][0] if model else "chinese-clip-vit-b16"

    img = rows(
        conn,
        "SELECT photo_hash, embedding FROM photo_embeddings WHERE model=? AND user_id=1",
        (model_id,),
    )
    report("图像塔 photo_embeddings", img, q)

    desc = rows(
        conn,
        "SELECT photo_hash, embedding FROM photo_text_embeddings WHERE kind='desc'",
    )
    report("描述塔 photo_text_embeddings(desc)", desc, q)

    # 描述塔里分高的是些什么文本？
    if desc:
        mats = np.stack([v for _, v in desc])
        cos = mats @ q
        order = np.argsort(-cos)
        hs = [desc[i][0] for i in order[:15]]
        ph = ",".join("?" * len(hs))
        txt = {}
        for h, t in conn.execute(
            f"SELECT photo_hash, text FROM photo_text_embeddings WHERE kind='desc' AND photo_hash IN ({ph})",
            hs,
        ):
            txt[h] = t
        print("\n=== 描述塔 top15 实际描述文本 ===")
        for i in order[:15]:
            print(f"  {cos[i]:.4f}  {txt.get(desc[i][0], '?')}")

    # 给 top 命中的图像塔结果配上路径/描述，看它到底是不是自行车
    if img:
        mats = np.stack([v for _, v in img])
        cos = mats @ q
        order = np.argsort(-cos)
        hs = [img[i][0] for i in order[:25]]
        ph = ",".join("?" * len(hs))
        info = {}
        for h, path, content in conn.execute(
            f"SELECT photo_hash, path, content FROM photo_content_scan WHERE user_id=1 AND photo_hash IN ({ph})",
            hs,
        ):
            info[h] = (path, content)
        print("\n=== 图像塔 top25 实际内容 ===")
        for i in order[:25]:
            h = img[i][0]
            p, c = info.get(h, ("<无 scan 行>", ""))
            print(f"  {cos[i]:.4f}  {Path(p).name if p else p}  | content={str(c)[:40]}")


if __name__ == "__main__":
    main()
