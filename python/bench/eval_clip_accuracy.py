"""CLIP 变体精度对比：同一 53 张标注集上比零样本分类准确率 + 数值一致性 + 速度

背景问题（2026-09-20 用户提出）：`python/models/chinese-clip-vit-b-16/`（PyTorch 导出的 fp32）
与 `python/models/chinese-clip-fp32/`（Xenova 转换的 fp32）到底差多少？谁是"精度更准"？

被比对的变体（每个 = 图塔 + 文塔 + 预处理 + 分词器）：
  b16-fp16       models/chinese-clip/          线上默认档（377MB，CPU 固定）
  b16-fp32       models/chinese-clip-fp32/     Xenova fp32（719MB，可走 DML）
  b16-torch      models/chinese-clip-vit-b-16/ PyTorch weights 直接 torch.onnx.export 的 fp32
  l14-fp16       models/chinese-clip-l14/      L/14-336（814MB/768 维）

方法（与 python/bench/eval_clip_zero.py 完全一致，便于与 FEAT-055 记录的 81.1% 基线对齐）：
  11 类中文 prompt × 4 模板 → 类心（各自 L2 归一后取均值再归一）
  原图 224/336 预处理 → 图塔 → L2 归一 → 与类心点积 argmax
  准确率 = top-1 / 53；并输出每类明细、图/文塔耗时、变体间余弦一致性

用法：
  python python/bench/eval_clip_accuracy.py                      # 全部可用变体，CPU
  python python/bench/eval_clip_accuracy.py --provider DmlExecutionProvider
  python python/bench/eval_clip_accuracy.py --only b16-fp32,b16-torch --provider DmlExecutionProvider
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
from vcr.services.embed_service import _MiniBertTok  # noqa: E402

GT_PATH = os.path.join(ROOT, "ground_truth.json")
OUT_JSON = os.path.join(HERE, "out", "clip_accuracy.json")

# 与 eval_clip_zero.py 严格一致的类名与模板（保证与 81.1% 基线可比）
CLASS_ZH = {
    "portrait": "人物特写人像", "street": "街道街景", "night_scene": "城市夜景",
    "plant_flower": "植物花朵", "food": "美食", "architecture": "建筑物",
    "vehicle": "车辆", "landscape_nature": "自然山水风景", "text": "文字文档屏幕",
    "other": "其他杂项", "animal": "动物",
}
TEMPLATES = ["一张{}的照片", "一张{}的图片", "这张照片展示的是{}", "{}"]
CLIP_MEAN = np.array(config.CLIP_MEAN, dtype=np.float32)
CLIP_STD = np.array(config.CLIP_STD, dtype=np.float32)

# 变体定义：目录 + 图塔/文塔文件名 + 输入尺寸 + 分词器来源（fast=tokenizer.json / mini=vocab.txt）
VARIANTS = [
    {"key": "b16-fp16", "dir": "chinese-clip", "vision": "clip_vision.onnx",
     "text": "clip_text.onnx", "size": 224, "max_len": 52, "tok": "fast", "note": "线上默认（fp16）"},
    {"key": "b16-fp32", "dir": "chinese-clip-fp32", "vision": "clip_vision.onnx",
     "text": "clip_text.onnx", "size": 224, "max_len": 52, "tok": "fast", "note": "Xenova fp32"},
    {"key": "b16-torch", "dir": "chinese-clip-vit-b-16", "vision": "clip_vision_b16.onnx",
     "text": "clip_text_b16.onnx", "size": 224, "max_len": 52, "tok": "mini", "note": "PyTorch 导出 fp32"},
    {"key": "l14-fp16", "dir": "chinese-clip-l14", "vision": "clip_vision.onnx",
     "text": "clip_text.onnx", "size": 336, "max_len": 64, "tok": "fast", "note": "L/14-336（Xenova fp16）"},
    {"key": "l14-224", "dir": "chinese-clip-l14-224", "vision": "clip_vision.onnx",
     "text": "clip_text.onnx", "size": 224, "max_len": 52, "tok": "auto",
     "note": "L/14@224（官方权重自行导出 fp32）"},
]


def paths_of(v: dict) -> tuple[str, str, str]:
    d = os.path.join(config.MODEL_DIR, v["dir"])
    return (os.path.join(d, v["vision"]), os.path.join(d, v["text"]), d)


def available(v: dict) -> bool:
    vis, txt, _ = paths_of(v)
    return os.path.isfile(vis) and os.path.isfile(txt)


def load_sessions(v: dict, provider: str):
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = config.threads()
    so.inter_op_num_threads = 1
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
    vis, txt, _ = paths_of(v)
    return (
        ort.InferenceSession(vis, sess_options=so, providers=[provider]),
        ort.InferenceSession(txt, sess_options=so, providers=[provider]),
    )


def make_tokenizer(v: dict, model_dir: str):
    mode = v["tok"]
    if mode == "auto":
        mode = "fast" if os.path.isfile(os.path.join(model_dir, "tokenizer.json")) else "mini"
    if mode == "fast":
        from tokenizers import Tokenizer

        tok = Tokenizer.from_file(os.path.join(model_dir, "tokenizer.json"))
        tok.enable_truncation(max_length=v["max_len"])
        tok.enable_padding(length=v["max_len"], pad_id=tok.token_to_id("[PAD]"), pad_token="[PAD]")

        def enc(texts):
            es = tok.encode_batch(texts)
            return (np.asarray([e.ids for e in es], dtype=np.int64),
                    np.asarray([e.attention_mask for e in es], dtype=np.int64))

        return enc, "fast"
    mini = _MiniBertTok(os.path.join(model_dir, "vocab.txt"), v["max_len"])
    return mini.encode_batch, "mini"


def preprocess(fp: str, size: int, mode: str = "crop", max_side: int = 0) -> np.ndarray | None:
    """两种 recipe：
      crop   = 短边缩放到 size + 中心裁剪（线上 B/16 现用；CLIP 通用做法）
      squash = 直接缩放到 size×size（不保持宽高比、不裁剪）—— **官方 ChineseCLIPFeatureExtractor
               的配置**（两个仓库的 preprocessor_config.json 都是 size=模型尺寸 + do_center_crop=false）
    实测两者准确率差异见 design/clip-accuracy-comparison.md。
    """
    from PIL import Image

    try:
        im = Image.open(fp).convert("RGB")
    except Exception:
        return None
    if max_side:
        # 模拟线上「缩略图直编码」：忠实还原 src-tauri thumbnail.rs 的
        # `DynamicImage::thumbnail(256,256)` —— **保持宽高比、长边=256**（不是短边）
        w, h = im.size
        if max(w, h) > max_side:
            r = max_side / max(w, h)
            im = im.resize((max(1, round(w * r)), max(1, round(h * r))), Image.BICUBIC)
    if mode == "squash":
        im = im.resize((size, size), Image.BICUBIC)
    else:
        w, h = im.size
        r = size / min(w, h)
        im = im.resize((max(size, round(w * r)), max(size, round(h * r))), Image.BICUBIC)
        w, h = im.size
        l, t = (w - size) // 2, (h - size) // 2
        im = im.crop((l, t, l + size, t + size))
    a = np.asarray(im, dtype=np.float32) / 255.0
    a = (a - CLIP_MEAN) / CLIP_STD
    return np.ascontiguousarray(a.transpose(2, 0, 1))


def l2(m: np.ndarray) -> np.ndarray:
    m = m.astype(np.float32)
    return m / (np.linalg.norm(m, axis=-1, keepdims=True) + 1e-9)


def run_variant(v: dict, provider: str, gt: dict, mode: str = "crop", max_side: int = 0) -> dict | None:
    if not available(v):
        return None
    t0 = time.perf_counter()
    vsess, tsess = load_sessions(v, provider)
    load_s = time.perf_counter() - t0
    enc, tok_mode = make_tokenizer(v, paths_of(v)[2])

    classes = list(CLASS_ZH)
    prompts = [t.format(CLASS_ZH[c]) for c in classes for t in TEMPLATES]
    ids, mask = enc(prompts)
    t0 = time.perf_counter()
    temb = l2(tsess.run(None, {tsess.get_inputs()[0].name: ids,
                               tsess.get_inputs()[1].name: mask})[0])
    text_ms = (time.perf_counter() - t0) * 1000.0 / len(prompts)
    cent = np.vstack([temb[i * len(TEMPLATES):(i + 1) * len(TEMPLATES)].mean(0)
                      for i in range(len(classes))])
    cent = l2(cent)

    files, gts, vecs, times = [], [], [], []
    per_image = []          # 逐图 {file, gt, pred, top1, margin}：供"连续量"配对检验
    for lab in gt["labels"]:
        fp = os.path.join(gt["test_dir"], lab["file"])
        px = preprocess(fp, v["size"], mode, max_side)
        if px is None:
            continue
        t0 = time.perf_counter()
        out = vsess.run(None, {vsess.get_inputs()[0].name: px[None]})[0]
        times.append((time.perf_counter() - t0) * 1000.0)
        vecs.append(l2(out)[0])
        files.append(lab["file"])
        gts.append(lab["gt"])
        # 连续量：top1 原始分 与 top1-top2 间隔（越分离越好）
        sims = cent @ vecs[-1]
        order = np.argsort(-sims)
        margin = float(sims[order[0]] - sims[order[1]]) if len(order) > 1 else 0.0
        per_image.append({
            "file": lab["file"], "gt": lab["gt"],
            "pred": classes[int(order[0])],
            "top1": float(sims[order[0]]), "margin": margin,
            "gt_sim": float(sims[classes.index(lab["gt"])]) if lab["gt"] in classes else None,
        })
    # warm 后重测速度（去掉首张的会话预热影响）
    warm = []
    for lab in gt["labels"][: min(12, len(files))]:
        px = preprocess(os.path.join(gt["test_dir"], lab["file"]), v["size"], mode, max_side)
        if px is None:
            continue
        t0 = time.perf_counter()
        vsess.run(None, {vsess.get_inputs()[0].name: px[None]})
        warm.append((time.perf_counter() - t0) * 1000.0)

    X = np.vstack(vecs)
    pred = [classes[int(np.argmax(cent @ x))] for x in X]
    hit = [p == g for p, g in zip(pred, gts)]
    per = {}
    for g, ok in zip(gts, hit):
        a = per.setdefault(g, [0, 0])
        a[0] += int(ok)
        a[1] += 1
    return {
        "key": v["key"], "note": v["note"], "provider": provider, "resize": mode, "max_side": max_side,
        "bound": list(vsess.get_providers()), "tok": tok_mode,
        "dim": int(X.shape[1]), "size": v["size"],
        "acc": round(sum(hit) / len(hit), 4), "n": len(hit),
        "per_class": {k: f"{v2[0]}/{v2[1]}" for k, v2 in sorted(per.items())},
        "wrong": [f for f, ok in zip(files, hit) if not ok],
        "per_image": per_image,
        "text_ms": round(text_ms, 2), "img_ms": round(float(np.mean(times)), 1),
        "img_ms_warm": round(float(np.mean(warm)), 1) if warm else None,
        "load_s": round(load_s, 1),
        "_vecs": X, "_gts": gts,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--provider", default="CPUExecutionProvider")
    ap.add_argument("--only", default="", help="逗号分隔变体 key，默认全部可用")
    ap.add_argument("--max-side", type=int, default=0,
                    help="复现线上缩略图：长边缩到该值（thumbnail(256,256) 语义），0=用原图")
    ap.add_argument("--resize", default="crop", choices=["crop", "squash"],
                    help="预处理 recipe：crop=短边+中心裁剪（B/16 现用）/ squash=直接缩放（官方配置）")
    args = ap.parse_args()

    gt = json.load(open(GT_PATH, encoding="utf-8"))
    todo = [v for v in VARIANTS if available(v)]
    if args.only:
        want = {s.strip() for s in args.only.split(",") if s.strip()}
        todo = [v for v in todo if v["key"] in want]
    if not todo:
        print("没有可用的模型变体（检查 python/models 下的拆分件）")
        sys.exit(2)

    print(f"标注集：{gt['total']} 张 · {len(CLASS_ZH)} 类 · provider={args.provider}")
    print(f"参与对比：{', '.join(v['key'] for v in todo)}\n")
    results = []
    for v in todo:
        r = run_variant(v, args.provider, gt, args.resize, args.max_side)
        if r is None:
            continue
        results.append(r)
        print(f"[{r['key']:<10}] resize={r['resize']:<6} maxside={r['max_side']:<4} 准确率 {r['acc'] * 100:5.1f}% ({r['n']} 张) · "
              f"dim={r['dim']} size={r['size']} tok={r['tok']} · "
              f"图 {r['img_ms_warm']}ms/张(热) 文 {r['text_ms']}ms/条 · "
              f"绑定 {r['bound'][0]} · 加载 {r['load_s']}s")
        if r["wrong"]:
            print(f"             错判 {len(r['wrong'])} 张：{', '.join(r['wrong'][:6])}"
                  + ("…" if len(r["wrong"]) > 6 else ""))

    # 数值一致性（同 dim/size 才能比）
    print("\n=== 变体间数值一致性（同图塔/文塔输出余弦，仅同 dim+size 可比）===")
    for i in range(len(results)):
        for j in range(i + 1, len(results)):
            a, b = results[i], results[j]
            if a["dim"] != b["dim"] or a["size"] != b["size"]:
                print(f"  {a['key']} ↔ {b['key']}: 维度/尺寸不同（{a['dim']}/{a['size']} vs "
                      f"{b['dim']}/{b['size']}）不可比")
                continue
            same_order = a["_gts"] == b["_gts"]
            if not same_order:
                continue
            cos = (l2(a["_vecs"]) * l2(b["_vecs"])).sum(axis=1)
            agree = sum(1 for x, y in zip(a["wrong"], b["wrong"]))
            pred_diff = len(set(a["wrong"]) ^ set(b["wrong"]))
            print(f"  {a['key']} ↔ {b['key']}: 图塔余弦 min={cos.min():.6f} mean={cos.mean():.6f} · "
                  f"错判集差异 {pred_diff} 张")

    print("\n=== 每类准确率 ===")
    keys = [r["key"] for r in results]
    print(f"{'类别':<20}" + "".join(f"{k:>12}" for k in keys))
    for c in CLASS_ZH:
        row = "".join(f"{results[i]['per_class'].get(c, '-'):>12}" for i in range(len(results)))
        print(f"{c:<20}{row}")

    os.makedirs(os.path.dirname(OUT_JSON), exist_ok=True)
    dump = [{k: v for k, v in r.items() if not k.startswith("_")} for r in results]
    json.dump({"provider": args.provider, "resize": args.resize, "max_side": args.max_side, "results": dump}, open(OUT_JSON, "w", encoding="utf-8"),
              ensure_ascii=False, indent=2)
    print(f"\n结果已写入 {os.path.relpath(OUT_JSON, ROOT)}")


if __name__ == "__main__":
    main()
