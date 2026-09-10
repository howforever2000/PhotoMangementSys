"""一次性拆分 CLIP 双塔（开发/部署 CLI）：python extract_clip_subgraphs.py

产出 python/models/chinese-clip/clip_vision.onnx + clip_text.onnx（服务端首次使用
也会自动拆，见 vcr/services/embed_service.ensure_subgraphs）。
"""
import sys

from vcr.services.clip_subgraph import ensure_subgraphs, verify_subgraphs

if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    err = ensure_subgraphs()
    if err:
        print("拆分失败:", err, file=sys.stderr)
        sys.exit(1)
    v = verify_subgraphs()
    print("verify:", v)
    sys.exit(0 if v["pass"] else 1)
