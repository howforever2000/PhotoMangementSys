"""下载 Chinese-CLIP ViT-B/16（HF 官方权重，走 hf-mirror 镜像）

产出 python/models/chinese-clip-vit-b-16/：
  config.json / vocab.txt / model.safetensors（或 pytorch_model.bin）
"""
import os
import sys

os.environ["HF_ENDPOINT"] = "https://hf-mirror.com"

from huggingface_hub import hf_hub_download

REPO = "OFA-Sys/chinese-clip-vit-base-patch16"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "models", "chinese-clip-vit-b-16")

def main():
    os.makedirs(OUT, exist_ok=True)
    for fname in ["config.json", "vocab.txt"]:
        p = hf_hub_download(REPO, fname, local_dir=OUT)
        print("ok:", p)
    # 优先 safetensors（约 750MB），失败回落 pytorch_model.bin
    try:
        p = hf_hub_download(REPO, "model.safetensors", local_dir=OUT)
    except Exception as e:
        print("safetensors 失败，回落 pytorch_model.bin:", e)
        p = hf_hub_download(REPO, "pytorch_model.bin", local_dir=OUT)
    print("ok:", p)
    print("size(MB):", round(os.path.getsize(p) / 1e6, 1))

if __name__ == "__main__":
    sys.exit(main())
