"""智能搜索检索质量评测台（BUG-2026-1101-007 配套）。

三组配置离线复算，全部走真实库向量、真实文本塔（HTTP 调本地 VCR 服务）：

  legacy   阈值 0.30 / 单路候选 2000 / 不折叠 / 权重 1.0（FEAT-154 之前）
  current  阈值 0.45 / 单路候选 600  / 不折叠 / 权重 1.0（FEAT-154，线上「改前」）
  rework   分路相对相关度 / 图像 top300 / 描述 top200 且同文本折叠 / 按 query 类型调权

指标：Precision@10 · Recall@10 · nDCG@10 · 第一屏命中数(top18) · 同分最大簇(top30)

ground truth：
  - 结构化 query（日期 / 地点 / 文档）：由库内字段精确判定，客观可复现
  - 物体词 query：人审池打标（`--labels` 指向 .pi/search_eval/labels.json，
    池子 = 三组配置各自 top20 的并集，保证三组在同一判定集上比较）

用法：
    python python/bench/eval_search_quality.py                # 跑全部 query × 三组配置
    python python/bench/eval_search_quality.py --pool 自行车  # 只导人审池（供打标）
"""
from __future__ import annotations

import argparse
import json
import re
import sqlite3
import sys
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".pi" / "search_eval"
DB = Path.home() / "AppData/Roaming/com.photomanagementsys.app/photos.db"
VCR = "http://127.0.0.1:18765"
MODEL = "chinese-clip-vit-b16"
USER_ID = 1
RRF_K = 60.0

# ---------------------------------------------------------------- 配置 ----------


@dataclass
class LaneCfg:
    """一路语义召回的参数"""

    abs_min_sim: float | None = None  # 绝对余弦闸（legacy / current 用）
    rel_floor: float = 0.0            # 相对提升闸（rework 用）
    candidates: int = 2000            # 该路候选池上限
    max_per_group: int = 0            # 同一描述文本最多几张（0 = 不折叠）
    lane_min_lift: float = 0.0        # 本路 rel 峰值低于此值 → 整路不参与


@dataclass
class Cfg:
    name: str
    img: LaneCfg
    desc: LaneCfg
    weight_structured: tuple[float, float] | None = None  # (图像路, 描述路)
    weight_object: tuple[float, float] | None = None
    relative_badge: bool = False
    limit: int | None = None
    tie_break: bool = False
    fold_structured: bool = False


CFGS = [
    Cfg(
        name="legacy",
        img=LaneCfg(abs_min_sim=0.30, candidates=2000),
        desc=LaneCfg(abs_min_sim=0.30, candidates=2000),
        limit=None,
    ),
    Cfg(
        name="current",
        img=LaneCfg(abs_min_sim=0.45, candidates=600),
        desc=LaneCfg(abs_min_sim=0.45, candidates=600),
        limit=300,
    ),
    Cfg(
        name="rework",
        img=LaneCfg(rel_floor=0.005, candidates=300, lane_min_lift=0.02),
        desc=LaneCfg(rel_floor=0.005, candidates=200, max_per_group=2, lane_min_lift=0.02),
        # 物体词：像不像由像素回答 → 图像 1.0，描述降为兜底 0.2
        # 结构化（日期/地点/文档）：日期地点文档都不在像素里 → 图像降为 0.2，描述 0.5
        weight_object=(1.0, 0.2),
        weight_structured=(0.2, 0.5),
        relative_badge=True,
        limit=300,
        tie_break=True,
        fold_structured=False,
    ),
]

# ------------------------------------------------------------ query 分类 ----------

_STRUCT_PATTERNS = [
    (re.compile(r"\d{4}\s*年|\d{1,2}\s*月|春|夏|秋|冬|今天|昨天|今年|去年"), "date"),
    (re.compile(r"省|市|区|县|镇|村|路|街|大道|景区|公园|广[场州]|旅游"), "place"),
    (re.compile(r"文档|文件|截图|发票|合同|证件|票据|扫描|paper|document"), "document"),
]


def query_kind(q: str) -> str:
    """query → 类型（date / place / document / object）。与 Rust `content::query_kind` 同口径。"""
    for pat, name in _STRUCT_PATTERNS:
        if pat.search(q):
            return name
    return "object"


def lane_weights(cfg: Cfg, q: str) -> tuple[float, float]:
    """query → (图像路权重, 描述路权重)。legacy/current 不分类型，一律 1.0。"""
    if cfg.weight_object is None:
        return (1.0, 1.0)
    return cfg.weight_object if query_kind(q) == "object" else cfg.weight_structured


# ---------------------------------------------------------------- 数据 ----------


def embed_text(text: str) -> np.ndarray:
    req = urllib.request.Request(
        VCR + "/embed_text",
        data=json.dumps({"text": text}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=120) as r:
        v = np.array(json.load(r)["embedding"], dtype=np.float32)
    return v / np.linalg.norm(v)


@dataclass
class Corpus:
    hashes: list[str]
    img: np.ndarray            # (N, 512) 已 L2 归一化
    desc_hash: list[str]
    desc: np.ndarray
    desc_group: np.ndarray     # 同文本 → 同 group id
    meta: dict[str, dict]      # hash → {path, shoot_time, location, content, label, id}
    kw_rows: list[tuple]       # 关键字路扫描的原始行（hash, shoot_time, id, path）
    desc_text: dict[str, str] = field(default_factory=dict)


def load_corpus() -> Corpus:
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = conn.execute(
        "SELECT photo_hash, path, id, shoot_time, location, content, label FROM photo_content_scan "
        "WHERE user_id=?",
        (USER_ID,),
    ).fetchall()
    meta = {}
    kw_rows = []
    for h, path, pid, st, loc, content, label in rows:
        meta[h] = {
            "path": path,
            "id": pid,
            "shoot_time": st,
            "location": loc,
            "content": content or "",
            "label": label or "",
        }
        kw_rows.append((h, st or "", pid, path or "", loc or "", content or "", label or ""))

    def vecs(sql, args=()):
        out = []
        for h, blob in conn.execute(sql, args):
            out.append((h, np.frombuffer(blob, dtype=np.float32)))
        hashes = [h for h, _ in out]
        mat = np.stack([v for _, v in out]) if out else np.zeros((0, 512), np.float32)
        norms = np.linalg.norm(mat, axis=1, keepdims=True)
        mat = mat / np.maximum(norms, 1e-9)
        return hashes, mat

    img_hash, img = vecs(
        "SELECT photo_hash, embedding FROM photo_embeddings WHERE user_id=? AND model=?", (USER_ID, MODEL)
    )
    desc_rows = conn.execute(
        "SELECT photo_hash, text, embedding FROM photo_text_embeddings WHERE kind='desc' AND model=?",
        (MODEL,),
    ).fetchall()
    keep = [(h, t, b) for h, t, b in desc_rows if h in meta]
    desc_hash = [h for h, _, _ in keep]
    desc = np.stack([np.frombuffer(b, dtype=np.float32) for _, _, b in keep]) if keep else np.zeros((0, 512), np.float32)
    desc = desc / np.maximum(np.linalg.norm(desc, axis=1, keepdims=True), 1e-9)
    texts = [t for _, t, _ in keep]
    gmap: dict[str, int] = {}
    desc_group = np.array([gmap.setdefault(t, len(gmap)) for t in texts], dtype=np.int32)
    desc_text = {h: t for h, t, _ in keep}
    return Corpus(img_hash, img, desc_hash, desc, desc_group, meta, kw_rows, desc_text)


# ------------------------------------------------------------ 三路召回 ----------


def keyword_lane(corpus: Corpus, kw: str) -> list[str]:
    """复刻 `db::content::smart_search` 的关键字路：LIKE 宽匹配 + shoot_time DESC。"""
    like = kw.strip()
    if not like:
        return []
    hit = [
        r
        for r in corpus.kw_rows
        if like in r[1] or like in r[4] or like in r[5] or like in r[6] or like in r[3]
    ]
    hit.sort(key=lambda r: (r[1] == "", [-ord(c) for c in r[1]], -r[2]))
    return [r[0] for r in hit]


def lane_rank(
    cos: np.ndarray,
    hashes: list[str],
    cfg: LaneCfg,
    groups: np.ndarray | None = None,
) -> tuple[list[tuple[str, float]], dict]:
    """一路排名 → ([(hash, 展示分)], 统计)。展示分是余弦（legacy/current）或 rel（rework）。"""
    if cos.size == 0:
        return [], {"median": 0.0, "rel_max": 0.0, "n": 0, "raw": 0}
    if cfg.abs_min_sim is not None:  # 绝对阈值范式
        idx = np.nonzero(cos >= cfg.abs_min_sim)[0]
        idx = idx[np.argsort(-cos[idx], kind="stable")]
        idx = idx[: cfg.candidates]
        return [(hashes[i], float(cos[i])) for i in idx], {
            "median": float(np.median(cos)),
            "rel_max": 0.0,
            "n": len(idx),
            "raw": int((cos >= cfg.abs_min_sim).sum()),
        }

    # 相对范式：rel = cos − 本路中位数
    median = float(np.median(cos))
    rel = cos - median
    rel_max = float(rel.max())
    stats = {"median": median, "rel_max": rel_max, "n": 0, "raw": int((rel >= cfg.rel_floor).sum())}
    if rel_max < cfg.lane_min_lift:
        return [], stats
    idx = np.nonzero(rel >= cfg.rel_floor)[0]
    idx = idx[np.argsort(-rel[idx], kind="stable")]
    if cfg.max_per_group > 0 and groups is not None:
        per: dict[int, int] = {}
        kept = []
        for i in idx:
            g = int(groups[i])
            if per.get(g, 0) >= cfg.max_per_group:
                continue
            per[g] = per.get(g, 0) + 1
            kept.append(i)
            if len(kept) >= cfg.candidates:
                break
        idx = np.array(kept, dtype=np.int64)
    else:
        idx = idx[: cfg.candidates]
    stats["n"] = len(idx)
    return [(hashes[i], float(rel[i])) for i in idx], stats


def fuse(
    cfg: Cfg,
    kw: list[str],
    img: list[tuple[str, float]],
    desc: list[tuple[str, float]],
    w_img: float,
    w_desc: float,
) -> list[dict]:
    """三路加权 RRF。返回 [{hash, score, badge, lane}]，已按最终顺序排好。"""
    score: dict[str, float] = {}
    order: dict[str, int] = {}
    badge: dict[str, float] = {}
    lane: dict[str, str] = {}

    def bump(h: str, s: float, b: float | None, ln: str | None) -> None:
        if h not in score:
            score[h] = 0.0
            order[h] = len(order)
        score[h] += s
        if b is not None and (h not in badge or b > badge[h]):
            badge[h] = b
            lane[h] = ln or ""

    img_max = max((v for _, v in img), default=1.0) or 1.0
    desc_max = max((v for _, v in desc), default=1.0) or 1.0
    for r, h in enumerate(kw):
        bump(h, 1.0 / (RRF_K + r + 1.0), None, None)
    for r, (h, v) in enumerate(img):
        bump(h, w_img / (RRF_K + r + 1.0), (v / img_max) if cfg.relative_badge else v, "img")
    for r, (h, v) in enumerate(desc):
        bump(h, w_desc / (RRF_K + r + 1.0), (v / desc_max) if cfg.relative_badge else v, "desc")

    def key(h: str):
        # 分数降序 → 图像路命中优先（图像优先·描述兜兜）→ 相关度降序 → 登记顺序
        primary = -score[h]
        img_first = -1 if (cfg.tie_break and lane.get(h) == "img") else 0
        rel = -(badge.get(h) or 0.0)
        return (primary, img_first, rel, order[h]) if cfg.tie_break else (primary, order[h])

    keys = sorted(score, key=key)
    out = [{"hash": h, "score": score[h], "badge": badge.get(h), "lane": lane.get(h)} for h in keys]
    return out[: cfg.limit] if cfg.limit else out


# -------------------------------------------------------------- 判定 ----------


def gt_structured(corpus: Corpus, q: str) -> dict[str, int] | None:
    """结构化 query → {hash: 1}（库内字段精确判定）；非结构化返回 None。"""
    kind = query_kind(q)
    m = re.fullmatch(r"(\d{4})\s*年\s*(\d{1,2})\s*月", q.strip())
    if m:
        y, mo = m.group(1), int(m.group(2))
        pre = f"{y}-{mo:02d}"
        alt = f"{y}/{mo:02d}"
        return {
            h: 1
            for h, v in corpus.meta.items()
            if v["shoot_time"] and (v["shoot_time"].startswith(pre) or v["shoot_time"].startswith(alt))
        }
    if kind == "place":
        core = re.sub(r"省|市|区|县", "", q.strip())
        if len(core) >= 2:
            return {h: 1 for h, v in corpus.meta.items() if core in (v["location"] or "")}
    if kind == "document":
        return {h: 1 for h, v in corpus.meta.items() if "文档" in (v["label"] or "") or "document" in (v["content"] or "")}
    return None


def ndcg(ranked: list[str], gt: dict[str, int], k: int) -> float:
    dcg = sum((gt.get(h, 0)) / np.log2(i + 2) for i, h in enumerate(ranked[:k]))
    ideal = sum(1 / np.log2(i + 2) for i in range(min(k, len(gt))))
    return float(dcg / ideal) if ideal > 0 else 0.0


def metrics(ranked: list[str], gt: dict[str, int], badges: list[float | None]) -> dict:
    top10 = ranked[:10]
    hits10 = sum(gt.get(h, 0) for h in top10)
    gt10 = min(10, len(gt))
    first_screen = sum(gt.get(h, 0) for h in ranked[:18])
    shown = [round(b * 100) if b is not None else None for b in badges[:30]]
    cluster = max((shown.count(x) for x in set(shown) if x is not None), default=0)
    return {
        "P@10": hits10 / 10.0,
        "R@10": hits10 / gt10 if gt10 else 0.0,
        "nDCG@10": ndcg(ranked, gt, 10),
        "第一屏命中": first_screen,
        "同分最大簇": cluster,
    }


# ---------------------------------------------------------------- 主流程 ----------


def judged_pool(corpus: Corpus, q: str, qv: np.ndarray, top: int = 20) -> list[str]:
    """三组配置各自 topN 的并集 = 人审池（保证同一判定集比较）"""
    kw = keyword_lane(corpus, q)
    img_cos = corpus.img @ qv if corpus.img.size else np.array([])
    desc_cos = corpus.desc @ qv if corpus.desc.size else np.array([])
    pool: dict[str, int] = {}
    for cfg in CFGS:
        img, _ = lane_rank(img_cos, corpus.hashes, cfg.img)
        desc, _ = lane_rank(desc_cos, corpus.desc_hash, cfg.desc, corpus.desc_group)
        w_img, w_desc = lane_weights(cfg, q)
        merged = fuse(cfg, kw, img, desc, w_img, w_desc)
        for r in merged[:top]:
            pool.setdefault(r["hash"], len(pool))
    return list(pool)


def thumb_of(conn, corpus: Corpus, h: str) -> str | None:
    row = conn.execute(
        "SELECT thumb_path FROM photo_thumb_cache WHERE photo_hash=? LIMIT 1", (h,)
    ).fetchone()
    if row and row[0] and Path(row[0]).is_file():
        return row[0]
    p = corpus.meta[h]["path"]
    return p if p and Path(p).is_file() else None


def dump_pool(corpus: Corpus, queries: list[str]) -> None:
    conn = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    OUT.mkdir(parents=True, exist_ok=True)
    for q in queries:
        if gt_structured(corpus, q) is not None:
            continue
        qv = embed_text(q)
        pool = judged_pool(corpus, q, qv)
        items = []
        for h in pool:
            t = thumb_of(conn, corpus, h)
            items.append({"hash": h, "thumb": t, "path": corpus.meta[h]["path"]})
        dst = OUT / f"pool_{q}.json"
        dst.write_text(json.dumps({"query": q, "items": items}, ensure_ascii=False, indent=1), "utf-8")
        missing = sum(1 for i in items if not i["thumb"])
        print(f"[pool] {q}: {len(items)} 张（缺图 {missing}）→ {dst.relative_to(ROOT)}")


def run(corpus: Corpus, queries: list[str], labels: dict[str, dict[str, int]]) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    header = f"{'query':<16}{'config':<10}{'P@10':>7}{'R@10':>7}{'nDCG@10':>9}{'第一屏':>8}{'簇':>5}{'条数':>7}"
    print(header)
    print("-" * len(header))
    table: dict[tuple[str, str], dict] = {}
    for q in queries:
        qv = embed_text(q)
        gt = gt_structured(corpus, q)
        if gt is None:
            gt = {h: 1 for h, v in labels.get(q, {}).items() if v}
        kind = query_kind(q)
        wk_img, wk_desc = lane_weights(CFGS[-1], q)
        kw = keyword_lane(corpus, q)
        img_cos = corpus.img @ qv if corpus.img.size else np.array([])
        desc_cos = corpus.desc @ qv if corpus.desc.size else np.array([])
        for cfg in CFGS:
            desc_cfg = cfg.desc
            if kind != "object" and not cfg.fold_structured:
                desc_cfg = LaneCfg(**{**desc_cfg.__dict__, "max_per_group": 0})
            img, _ = lane_rank(img_cos, corpus.hashes, cfg.img)
            desc, _ = lane_rank(desc_cos, corpus.desc_hash, desc_cfg, corpus.desc_group)
            wi, wd = lane_weights(cfg, q)
            merged = fuse(cfg, kw, img, desc, wi, wd)
            ranked = [r["hash"] for r in merged]
            m = metrics(ranked, gt, [r["badge"] for r in merged])
            table[(q, cfg.name)] = m
            print(
                f"{q:<16}{cfg.name:<10}{m['P@10']:>5.0%} {m['R@10']:>6.0%}{m['nDCG@10']:>9.3f}"
                f"{m['第一屏命中']:>8}{m['同分最大簇']:>5}{len(ranked):>7}"
            )
        print(
            f"  └ {q}: 类型={kind} 权重=图{wk_img}/描{wk_desc} | 关键字路={len(kw)} | GT={len(gt)}"
            f" | 图像中位={float(np.median(img_cos)):.3f} 描述中位={float(np.median(desc_cos)):.3f}"
        )

    print("\n=== 汇总（各配置在全部 query 上的均值）===")
    for cfg in CFGS:
        ms = [table[(q, cfg.name)] for q in queries]
        print(
            f"{cfg.name:<10} P@10={np.mean([m['P@10'] for m in ms]):.3f} "
            f"R@10={np.mean([m['R@10'] for m in ms]):.3f} "
            f"nDCG@10={np.mean([m['nDCG@10'] for m in ms]):.3f} "
            f"第一屏={np.mean([m['第一屏命中'] for m in ms]):.2f} "
            f"最大簇={max(m['同分最大簇'] for m in ms)}"
        )
    (OUT / "last_metrics.json").write_text(
        json.dumps({f"{q}|{c}": v for (q, c), v in table.items()}, ensure_ascii=False, indent=1), "utf-8"
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pool", nargs="*", help="导出人审池（物体词 query）")
    ap.add_argument("--queries", nargs="*")
    args = ap.parse_args()

    corpus = load_corpus()
    print(f"[corpus] 图像向量 {len(corpus.hashes)} · 描述向量 {len(corpus.desc_hash)} · 元数据 {len(corpus.meta)}")
    default_q = ["自行车", "火锅", "雪山", "2019年6月", "成都市", "文档"]
    queries = args.queries or default_q

    if args.pool is not None:
        dump_pool(corpus, args.pool or queries)
        return

    labels: dict[str, dict[str, int]] = {}
    lp = OUT / "labels.json"
    if lp.is_file():
        labels = json.loads(lp.read_text("utf-8"))
    else:
        print(f"[warn] 未找到 {lp.relative_to(ROOT)}：物体词 query 的 GT 为空，指标不可解释")
    run(corpus, queries, labels)


if __name__ == "__main__":
    sys.exit(main())
