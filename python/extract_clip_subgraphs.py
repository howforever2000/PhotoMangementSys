"""一次性拆分 CLIP 双塔（开发/部署 CLI）：python extract_clip_subgraphs.py [档位]

档位默认取当前生效档（config.active_clip()），可显式指定：b16 / b16-fp32 / l14。
产出 <模型目录>/clip_vision.onnx + clip_text.onnx（服务端首次使用也会自动拆，
见 vcr/services/embed_service.ensure_subgraphs）。
"""
import sys

from vcr.services.clip_subgraph import ensure_subgraphs, verify_subgraphs

if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    tier = sys.argv[1] if len(sys.argv) > 1 else None
    err = ensure_subgraphs(tier)
    if err:
        print("拆分失败:", err, file=sys.stderr)
        sys.exit(1)
    v = verify_subgraphs(tier)
    print(f"档位 {tier or 'auto'} verify:", v)
    sys.exit(0 if v["pass"] else 1)
