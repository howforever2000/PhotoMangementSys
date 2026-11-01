"""交叉校验：Rust 真实现（cargo test probe_real_lanes）vs Python 离线模拟（eval_search_quality）。

两者必须逐条同序 —— 评测台是模拟的，不改用真实现钉住就等于没验。

用法：
    cargo test --lib probe_real_lanes -- --ignored --nocapture > .pi/search_eval/rust_lanes.txt
    python python/bench/verify_rust_vs_sim.py
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python" / "bench"))

from eval_search_quality import (  # noqa: E402
    CFGS,
    OUT,
    embed_text,
    lane_rank,
    lane_weights,
    load_corpus,
    query_kind,
)


def parse_rust(path: Path) -> dict:
    out: dict[tuple[str, str], list[tuple[str, float]]] = {}
    meta: dict[str, dict] = {}
    for line in path.read_text("utf-8").splitlines():
        p = line.split("\t")
        if p[0] == "PROBE" and len(p) >= 7:
            meta[p[1]] = {
                "kind": p[2],
                "img_median": float(p[3].split("=")[1]),
                "img_relmax": float(p[4].split("=")[1]),
                "desc_median": float(p[5].split("=")[1]),
                "desc_relmax": float(p[6].split("=")[1]),
            }
        elif p[0] == "LANE" and len(p) >= 5:
            out.setdefault((p[1], p[2]), []).append((p[3], float(p[4])))
    return {"lanes": out, "meta": meta}


def compare(got: list[tuple[str, float]], sim: list[tuple[str, float]], top: int = 20) -> dict:
    """按「同序 + 容差」对比两路排名。

    允许的差异只有一种：rel 几乎相等（< 1e-5）的**近并列**位置会因 f32 累加顺序互换。
    这不是实现分歧 —— 两边差值的量级就是 1e-7。所以判据分两层：
      1. 前 `top` 名必须逐条同序（注：`top` 内若也有近并列，则按集合比）；
      2. 全长度上报告最大名次漂移与最大 rel 差值。
    """
    n = min(len(got), len(sim))
    got_h = [g[0] for g in got[:n]]
    sim_h = [s[0] for s in sim[:n]]
    exact = got_h == sim_h
    top_exact = got_h[:top] == sim_h[:top]
    pos = {h: i for i, h in enumerate(sim_h)}
    drift = [abs(i - pos[h]) for i, h in enumerate(got_h) if h in pos]
    max_diff = max((abs(g[1] - s[1]) for g, s in zip(got[:n], sim[:n])), default=0.0)
    return {
        "n": n,
        "exact": exact,
        "top_exact": top_exact,
        "max_drift": max(drift, default=0),
        "max_diff": max_diff,
    }


def main() -> int:
    src = OUT / "rust_lanes.txt"
    if not src.is_file():
        print(f"缺少 {src}：先跑 cargo test --lib probe_real_lanes -- --ignored --nocapture")
        return 2
    rust = parse_rust(src)
    cfg = next(c for c in CFGS if c.name == "rework")
    corpus = load_corpus()
    bad = 0
    for q in rust["meta"]:
        qv = embed_text(q)
        kind = query_kind(q)
        fold = 2 if kind == "object" else 0
        desc_cfg = type(cfg.desc)(**{**cfg.desc.__dict__, "max_per_group": fold})
        img, _ = lane_rank(corpus.img @ qv, corpus.hashes, cfg.img)
        desc, _ = lane_rank(corpus.desc @ qv, corpus.desc_hash, desc_cfg, corpus.desc_group)
        w_img, w_desc = lane_weights(cfg, q)
        print(f"\n[{q}] 类型={kind} 权重=图{w_img}/描{w_desc} 折叠={fold}")
        print(f"  Rust  : {rust['meta'][q]}")
        for lane, sim in (("img", img), ("desc", desc)):
            got = rust["lanes"].get((q, lane), [])
            c = compare(got, sim)
            ok = c["top_exact"] and c["max_diff"] < 1e-4
            if not ok:
                bad += 1
                for i, (g, s) in enumerate(zip(got, sim)):
                    if g[0] != s[0]:
                        print(f"  首个分叉 #{i}: rust={g} sim={s}")
                        break
            print(
                f"  {lane:<4} rust={len(got):>3} sim={len(sim):>3} 比较{c['n']:>3} "
                f"前20同序={c['top_exact']} 整体同序={c['exact']} "
                f"最大名次漂移={c['max_drift']:<3} 最大分值差={c['max_diff']:.2e} → {'OK' if ok else 'MISMATCH'}"
            )
    print(
        "\n结论："
        + ("前 20 名逐条同序、分值差 < 1e-4（近并列位次会因 f32 累加顺序互换，量级 1e-7）" if bad == 0 else f"{bad} 条路不一致")
    )
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
