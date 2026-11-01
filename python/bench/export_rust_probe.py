"""导出 Rust 侧交叉校验用的查询向量（真实文本塔编码结果，落盘后由 Rust 测试读取）。"""
import json
from pathlib import Path

from eval_search_quality import OUT, embed_text

QUERIES = ["自行车", "2019年6月", "成都市", "文档"]


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    data = {"queries": [{"q": q, "vec": [float(x) for x in embed_text(q)]} for q in QUERIES]}
    dst = OUT / "rust_probe.json"
    dst.write_text(json.dumps(data, ensure_ascii=False), "utf-8")
    print(f"→ {dst} ({len(QUERIES)} 条查询向量, dim={len(data['queries'][0]['vec'])})")


if __name__ == "__main__":
    main()
