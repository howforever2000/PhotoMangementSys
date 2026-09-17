"""语义档位一致性 / 速度对照（Q2 验证工具）

用途：
  1. 换语义档位（b16 / b16-fp32）前，确认「图塔 + 文塔」输出是否一致
     —— 余弦 >0.999 才可认为两档在同一语义空间（可作为索引迁移依据）；
  2. 验证某档位在指定 provider（CPU / DmlExecutionProvider）上的数值正确性与速度
     —— fp16 在 DirectML 上有算子级数值 bug（BUG-2026-0910-006），
        本工具是「fp32 档能否安全启用 GPU」的判定依据。

用法：
  python python/bench/verify_clip_tiers.py b16 b16-fp32            # CPU 对照
  python python/bench/verify_clip_tiers.py b16-fp32 b16-fp32 --provider-a DmlExecutionProvider --provider-b CPUExecutionProvider
  python python/bench/verify_clip_tiers.py b16 b16-fp32 --n 24

判定门：vision_cos / text_cos 均 > 0.999 视为一致（Phase 0 拆图校验同门限）。
"""
import argparse
import json
import os
import sys
import time

import numpy as np

sys.stdout.reconfigure(encoding="utf-8")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

from vcr import config  # noqa: E402
from vcr.preprocess import clip_tensor, open_image  # noqa: E402

KW = ["一只猫", "美食", "雪山", "夜景", "一张照片", "屏幕截图"]


def _sess(path: str, provider: str, basic_opt: bool = True):
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = config.threads()
    so.inter_op_num_threads = 1
    so.graph_optimization_level = (
        ort.GraphOptimizationLevel.ORT_ENABLE_BASIC if basic_opt else ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    )
    return ort.InferenceSession(path, sess_options=so, providers=[provider])


def _tokenizer(tier: str):
    p = config.clip_paths(tier)
    try:
        from tokenizers import Tokenizer

        tok = Tokenizer.from_file(p["tokenizer"])
        tok.enable_truncation(max_length=p["max_len"])
        tok.enable_padding(length=p["max_len"], pad_id=tok.token_to_id("[PAD]"), pad_token="[PAD]")

        def enc(texts):
            es = tok.encode_batch(texts)
            return (
                np.asarray([e.ids for e in es], dtype=np.int64),
                np.asarray([e.attention_mask for e in es], dtype=np.int64),
            )

        return enc
    except Exception:  # noqa: BLE001
        from vcr.services.embed_service import _MiniBertTok

        mini = _MiniBertTok(p["vocab"], p["max_len"])
        return mini.encode_batch


def _l2(v: np.ndarray) -> np.ndarray:
    v = v.astype(np.float32)
    return v / (np.linalg.norm(v, axis=-1, keepdims=True) + 1e-9)


def _cos(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    return (_l2(a) * _l2(b)).sum(axis=-1)


def _sample_thumbs(n: int) -> list[str]:
    """从 bench 产物（真实库缩略图路径）抽样；缺失则回落 test_fixture_photos。"""
    paths_json = os.path.join(HERE, "out", "paths.json")
    if os.path.isfile(paths_json):
        allp = json.load(open(paths_json, encoding="utf-8"))
        step = max(1, len(allp) // n)
        return allp[::step][:n]
    fx = os.path.join(ROOT, "test_fixture_photos")
    out = []
    for name in sorted(os.listdir(fx))[:n]:
        out.append(os.path.join(fx, name))
    return out


def run_tier(tier: str, provider: str, thumbs: list[str]):
    p = config.clip_paths(tier)
    for f in ("vision", "text"):
        if not os.path.isfile(p[f]):
            raise SystemExit(f"档位 {tier} 的 {f} 拆分件缺失：{p[f]}（先跑 extract_clip_subgraphs.py {tier}）")
    vsess = _sess(p["vision"], provider)
    tsess = _sess(p["text"], provider)
    enc = _tokenizer(tier)

    # 图塔
    imgs = [im for im in (open_image(t) for t in thumbs) if im is not None]
    pixels = np.vstack([clip_tensor(im) for im in imgs])
    t0 = time.perf_counter()
    v = vsess.run(None, {vsess.get_inputs()[0].name: pixels})[0]
    img_ms = (time.perf_counter() - t0) * 1000.0 / max(1, len(imgs))
    # 文塔
    ids, mask = enc(KW)
    t0 = time.perf_counter()
    tv = tsess.run(None, {tsess.get_inputs()[0].name: ids, tsess.get_inputs()[1].name: mask})[0]
    txt_ms = (time.perf_counter() - t0) * 1000.0 / max(1, len(KW))
    return {
        "tier": tier,
        "provider": provider,
        "bound": list(vsess.get_providers()),
        "dim": int(v.shape[1]),
        "size": int(p["size"]),
        "vision": v.astype(np.float32),
        "text": tv.astype(np.float32),
        "img_ms": img_ms,
        "txt_ms": txt_ms,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("tier_a")
    ap.add_argument("tier_b")
    ap.add_argument("--provider-a", default="CPUExecutionProvider")
    ap.add_argument("--provider-b", default="CPUExecutionProvider")
    ap.add_argument("--n", type=int, default=24)
    args = ap.parse_args()

    thumbs = _sample_thumbs(args.n)
    a = run_tier(args.tier_a, args.provider_a, thumbs)
    b = run_tier(args.tier_b, args.provider_b, thumbs)

    print(f"A = {a['tier']} @{a['provider']}  实际绑定 {a['bound']}  dim={a['dim']} size={a['size']}")
    print(f"B = {b['tier']} @{b['provider']}  实际绑定 {b['bound']}  dim={b['dim']} size={b['size']}")
    print(f"样本 {len(thumbs)} 张 · 图塔 {a['img_ms']:.1f}ms/张 vs {b['img_ms']:.1f}ms/张 · 文塔 {a['txt_ms']:.1f}ms/条 vs {b['txt_ms']:.1f}ms/条")

    if a["dim"] != b["dim"]:
        print(f"\n维度不同（{a['dim']} vs {b['dim']}）→ 两个档位不是同一空间，无需比对（换档必须重建索引）")
        return

    cv = _cos(a["vision"], b["vision"])
    ct = _cos(a["text"], b["text"])
    same_size = a["size"] == b["size"]
    print(f"\n图塔余弦：min={cv.min():.6f} mean={cv.mean():.6f}  {'(输入尺寸一致)' if same_size else '(输入尺寸不同，仅作参考)'}")
    print(f"文塔余弦：min={ct.min():.6f} mean={ct.mean():.6f}")
    for i, kw in enumerate(KW):
        print(f"    {kw:<6} {ct[i]:.6f}")
    ok = cv.min() > 0.999 and ct.min() > 0.999
    print(f"\n判定（门限 0.999）：{' 一致 ✓' if ok else ' 不一致 ✗ —— 不可视为同一语义空间'}")
    sys.exit(0 if ok else 2)


if __name__ == "__main__":
    main()
