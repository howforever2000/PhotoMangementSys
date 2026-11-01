"""把人工看图打标结果写进 labels.json（按 pool_<query>.json 的条目顺序对齐哈希）。

打标口径（物体词 query）：图里确实出现该物体 = 1，否则 = 0。
自行车：两轮人力车（含自行车 / 共享单车 / 自行车局部特写）；摩托车、电动车不算。
"""
import json
from pathlib import Path

OUT = Path(".pi/search_eval")

LABELS = {
    "自行车": [
        0, 1, 1, 0, 1, 0, 1, 0, 1, 0,
        1, 0, 1, 0, 1, 0, 1, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 0,
        0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    ],
}


def main() -> None:
    path = OUT / "labels.json"
    labels = json.loads(path.read_text("utf-8")) if path.is_file() else {}
    for q, verdicts in LABELS.items():
        items = json.loads((OUT / f"pool_{q}.json").read_text("utf-8"))["items"]
        assert len(verdicts) == len(items), f"{q}: 标注 {len(verdicts)} 条 ≠ 池子 {len(items)} 条"
        labels[q] = {it["hash"]: v for it, v in zip(items, verdicts)}
        pos = sum(verdicts)
        print(f"{q}: 池 {len(items)} 张 · 正例 {pos} 张 ({pos/len(items)*100:.0f}%)")
    path.write_text(json.dumps(labels, ensure_ascii=False, indent=1), "utf-8")
    print(f"→ {path}")


if __name__ == "__main__":
    main()
