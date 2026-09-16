"""在真实手机相册上做端到端语义测试（612 张 4000×3000，从未入库的相册）

测三件事：
  1. **速度**：真实 4000×3000 手机照片上，「解码原图」vs「生成缩略图」vs「CLIP 编码」各占多少
     —— 直接回答「顺带生成 CLIP 缩略图值不值」「原图编码贵多少」
  2. **可行性**：忠实复现 app 的缩略图（长边 256 + WebP q85 + Triangle≈BILINEAR），批量生成 + 编码，
     得到单相册（612 张）的完整扫描耗时估算
  3. **质量（label-free 代理）**：用**用户真实分类配置**（photo_categories 的 17 个预设 + 各自阈值）
     算命中数与 top 命中，供人工/视觉模型抽查精度；并给出"命中数是否合理"的分布

用法：python python/bench/e2e_real_album_test.py [照片目录] [--limit N] [--sample-speed 60]
"""
import argparse
import glob
import json
import os
import sqlite3
import sys
import tempfile
import time

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)          # python/
REPO = os.path.dirname(ROOT)
sys.path.insert(0, ROOT)

from PIL import Image  # noqa: E402
from vcr import config  # noqa: E402
from vcr.services.embed_service import get_embed_service  # noqa: E402

DB = os.path.expandvars(r"%APPDATA%\com.haoyuan.photo-management-sys\photos.db")
# 与 src-tauri/src/category.rs::NEUTRAL_PROMPTS 严格一致
NEUTRAL = ["一张照片", "一张图片", "一张普通的照片", "随手拍的照片",
           "日常生活照片", "一张图片素材", "相册里的一张图"]
EXTS = (".jpg", ".jpeg", ".png", ".webp", ".heic", ".bmp")
THUMB_LONG = 256


def list_photos(root: str, limit: int = 0) -> list[str]:
    out: list[str] = []
    for dp, _dn, fn in os.walk(root):
        for f in sorted(fn):
            if f.lower().endswith(EXTS):
                out.append(os.path.join(dp, f))
    out.sort()
    return out[:limit] if limit else out


def make_thumb(src: str, dst: str) -> float:
    """忠实复现 app：长边 256 保持宽高比（Triangle≈BILINEAR）+ WebP q85。返回耗时 ms"""
    t0 = time.perf_counter()
    with Image.open(src) as im0:
        im = im0.convert("RGB")
    w, h = im.size
    r = THUMB_LONG / max(w, h)
    if r < 1:
        im = im.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BILINEAR)
    im.save(dst, "WEBP", quality=85)
    return (time.perf_counter() - t0) * 1000.0


def load_user_categories() -> list[tuple[str, list[str], float]]:
    """用户真实配置：非 builtin 分类的 (名称, 关键词, 阈值 rel)"""
    con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    out = []
    for name, kw, thr in con.execute(
        "SELECT name, keywords, threshold FROM photo_categories "
        "WHERE source != 'builtin' AND enabled = 1 AND keywords IS NOT NULL ORDER BY id"
    ):
        kws = json.loads(kw) if kw else []
        if kws:
            out.append((name, kws, float(thr)))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dir", nargs="?", default=r"D:\YUAN HAO\Pictures\img\手机照片\2019")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--sample-speed", type=int, default=60)
    args = ap.parse_args()

    photos = list_photos(args.dir, args.limit)
    print(f"相册：{args.dir}\n共 {len(photos)} 张\n")

    # ---------- 1. 速度：原图解码 vs 生成缩略图 ----------
    tmp = tempfile.mkdtemp(prefix="e2e_thumbs_")
    sample = photos[:: max(1, len(photos) // args.sample_speed)][: args.sample_speed]
    dec, thumb_ms, sizes = [], [], []
    for i, src in enumerate(sample):
        t0 = time.perf_counter()
        with Image.open(src) as im0:
            im = im0.convert("RGB")
            sizes.append(im.size)
        dec.append((time.perf_counter() - t0) * 1000)
        thumb_ms.append(make_thumb(src, os.path.join(tmp, f"s{i}.webp")))
    dec_ms, mk_ms = float(np.mean(dec)), float(np.mean(thumb_ms))
    kb = float(np.mean([os.path.getsize(os.path.join(tmp, f"s{i}.webp")) for i in range(len(sample))])) / 1024
    print("=== ① 速度（真实 4000×3000 手机照片）===")
    print(f"  原图解码           {dec_ms:7.1f} ms/张   （尺寸示例 {sizes[0][0]}×{sizes[0][1]}）")
    print(f"  生成缩略图(+WebP)  {mk_ms:7.1f} ms/张   （长边{THUMB_LONG}, 平均 {kb:.1f} KB）")
    print(f"  CLIP 编码(缩略图)  待测")

    # ---------- 2. 全量生成缩略图 + 编码 ----------
    print(f"\n=== ② 单相册全量（{len(photos)} 张）===")
    t0 = time.perf_counter()
    thumbs = []
    for i, src in enumerate(photos):
        tp = os.path.join(tmp, f"t{i}.webp")
        make_thumb(src, tp)
        thumbs.append(tp)
    gen_s = time.perf_counter() - t0
    print(f"  缩略图生成 共 {gen_s:6.1f} s（{gen_s / len(photos) * 1000:.1f} ms/张，含 WebP 编码）")

    svc = get_embed_service()
    vecs: list[np.ndarray] = []
    t0 = time.perf_counter()
    B = 8
    for i in range(0, len(thumbs), B):
        res = svc.embed_images(thumbs[i:i + B])
        for r in res:
            if r.get("embedding") is not None:
                vecs.append(np.asarray(r["embedding"], np.float32))
    emb_s = time.perf_counter() - t0
    print(f"  CLIP 编码  共 {emb_s:6.1f} s（{emb_s / len(thumbs) * 1000:.1f} ms/张，batch={B}）")
    print(f"  → 单相册端到端 ≈ {gen_s + emb_s:.0f} s（{len(photos)} 张）；10k 张折算 ≈ "
          f"{(gen_s + emb_s) / len(photos) * 10000 / 60:.0f} min")
    X = np.vstack(vecs)
    print(f"  向量矩阵 {X.shape}")

    # ---------- 3. 用用户真实分类配置算命中 ----------
    cats = load_user_categories()
    print(f"\n=== ③ 语义分类命中（用户真实配置：{len(cats)} 个分类，阈值取自 DB）===")
    texts, index = [], {}
    for _, kws, _ in cats:
        for k in kws:
            if k not in index:
                index[k] = len(texts)
                texts.append(k)
    for t in NEUTRAL:
        if t not in index:
            index[t] = len(texts)
            texts.append(t)
    tv = np.vstack(svc.embed_texts(texts))
    tv /= np.linalg.norm(tv, axis=1, keepdims=True)
    base = np.mean([X @ tv[index[t]] for t in NEUTRAL], axis=0)

    print(f"{'分类':<8}{'阈值':>6}{'命中':>7}   前三名命中分数")
    rows = []
    for name, kws, thr in cats:
        sims = np.vstack([X @ tv[index[k]] for k in kws])
        best = sims.max(axis=0)
        arg = sims.argmax(axis=0)
        rel = best - base
        idx = np.where(rel >= thr)[0]
        order = idx[np.argsort(-rel[idx])]
        top = ", ".join(f"{rel[i]:.3f}" for i in order[:3])
        rows.append((name, thr, len(idx), order, arg))
        print(f"{name:<8}{thr:>6.2f}{len(idx):>7}   {top}")
    total_hits = sum(r[2] for r in rows)
    print(f"\n  命中总数（含跨分类重复）{total_hits}；平均每张命中 "
          f"{total_hits / len(photos):.2f} 个分类")

    # 抽查清单：每类 top3 命中文件名
    print("\n=== 抽查清单（每类 top3，供视觉模型判读）===")
    for name, thr, cnt, order, arg in rows:
        if cnt == 0:
            continue
        picks = [os.path.basename(photos[i]) for i in order[:3]]
        kw = cats[[c[0] for c in cats].index(name)][1][arg[order[0]]]
        print(f"  {name:<8} 关键词「{kw}」← {picks}")

    out = os.path.join(HERE, "out", "e2e_real_album.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump({
        "album": args.dir, "n": len(photos),
        "decode_ms": round(dec_ms, 1), "thumb_ms": round(mk_ms, 1),
        "thumb_gen_s": round(gen_s, 1), "embed_s": round(emb_s, 1),
        "thumb_kb": round(kb, 1),
        "categories": [{"name": r[0], "threshold": r[1], "hits": r[2],
                        "top": [os.path.basename(photos[i]) for i in r[3][:10]]} for r in rows],
    }, open(out, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    print(f"\n结果已写入 {os.path.relpath(out, REPO)}")


if __name__ == "__main__":
    main()
