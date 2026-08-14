"""VCR 回归评测：读 ground_truth.json + 对测试集跑全量 classify_one，输出准确率。

用法:
    python eval_vcr.py                     # 全量 53 张
    python eval_vcr.py --limit 10          # 前 10 张（快速冒烟）
    python eval_vcr.py --out test_results_phase1_5.json

对比口径：
  - gt 与预测都用 taxonomy.fold() 折叠到 9 组（portrait/street/animal/food/flower/
    landscape/cityscape/night_scene/document + other），保证新旧类名可比。
  - 输出：整体准确率 / 分项 / 错误明细（预测→gt 对照）。
"""
import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from vcr.model_registry import get_registry  # noqa: E402
from vcr.services.pipeline import classify_one  # noqa: E402
from vcr.taxonomy import get_taxonomy  # noqa: E402

GT_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ground_truth.json")


def load_gt() -> list[dict]:
    with open(GT_PATH, encoding="utf-8") as f:
        d = json.load(f)
    return d["labels"], d.get("test_dir", "")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--out", default="test_results_phase1_5.json")
    args = parser.parse_args()

    labels, test_dir = load_gt()
    if args.limit > 0:
        labels = labels[: args.limit]

    tax = get_taxonomy()
    reg = get_registry()
    reg.status()  # 强制加载模型

    rows = []
    t_start = time.perf_counter()
    for i, item in enumerate(labels):
        path = os.path.join(test_dir, item["file"])
        r = classify_one(path, reg)
        if r is None:
            rows.append({"file": item["file"], "gt": item["gt"], "gt_zh": item["gt_zh"],
                         "pred": "ERR", "pred_folded": "other", "ok": False, "error": "读取失败"})
            continue
        pred_folded = tax.fold(r.category)
        gt_folded = tax.fold(item["gt"])
        rows.append({
            "file": item["file"],
            "gt": item["gt"], "gt_zh": item["gt_zh"], "gt_folded": gt_folded,
            "pred": r.category, "pred_folded": pred_folded,
            "label": r.label, "source": r.source, "conf": r.confidence,
            "ok": pred_folded == gt_folded,
        })
        if (i + 1) % 10 == 0:
            print(f"[eval] {i + 1}/{len(labels)}", file=sys.stderr)
    elapsed = time.perf_counter() - t_start

    ok = sum(1 for x in rows if x.get("ok"))
    total = len(rows)
    acc = ok / total * 100 if total else 0.0

    # 分项统计
    by_gt: dict[str, dict] = {}
    for x in rows:
        g = x.get("gt_folded", "other")
        b = by_gt.setdefault(g, {"n": 0, "ok": 0, "miss": []})
        b["n"] += 1
        if x.get("ok"):
            b["ok"] += 1
        else:
            b["miss"].append({"file": x["file"], "pred": x.get("pred_folded"), "label": x.get("label"), "source": x.get("source")})

    print("=" * 64)
    print(f"[eval] 总数 {total} | 正确 {ok} | 准确率 {acc:.1f}% | 耗时 {elapsed:.1f}s ({elapsed/max(total,1)*1000:.0f}ms/张)")
    print("=" * 64)
    for g in sorted(by_gt):
        b = by_gt[g]
        pct = b["ok"] / b["n"] * 100 if b["n"] else 0
        print(f"  {g:<12} {b['ok']}/{b['n']} ({pct:5.1f}%)")
        for m in b["miss"]:
            print(f"      ✕ {m['file']}  → {m['pred']} ({m['label']}, {m['source']})")
    print("=" * 64)

    summary = {
        "total": total, "ok": ok, "accuracy_pct": round(acc, 1),
        "elapsed_sec": round(elapsed, 1), "avg_ms": round(elapsed / max(total, 1) * 1000, 0),
        "by_gt": {g: {"n": b["n"], "ok": b["ok"], "miss": b["miss"]} for g, b in by_gt.items()},
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump({"summary": summary, "rows": rows}, f, ensure_ascii=False, indent=1)
    print(f"[eval] 明细已写入 {args.out}")


if __name__ == "__main__":
    main()
