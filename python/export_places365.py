"""导出 Places365 场景模型为 ONNX（仅需执行一次）

依赖：torch/torchvision（导出后推理只需 onnxruntime）
模型：ResNet18 trained on Places365（官方 MIT 权重）

用法: python export_places365.py
      python export_places365.py --mirror https://ghfast.top    # 镜像加速
产出: models/resnet18_places365.onnx (~44MB) + models/categories_places365.txt (365 行)

为什么这是必启用通道：
  ImageNet 1k 是物体库（animal/vehicle/food/...），「风景/城市/室内」只覆盖 11 个粗类，
  top-1 几乎不会落在自然风景上。Places365 是场景库，有 365 个场景类
  （mountain/forest/coast/valley/abbey/crosswalk/bedroom/...），
  ImageNet 物体通道 + Places365 场景通道 互补，才能让「自然风景 vs 城市风光 vs 街道」准确。
  7840HS CPU 单张推理 ResNet18 ~50-80ms，可接受。

下载源：
  - 权重（~44MB）：http://places2.csail.mit.edu/models_places365/resnet18_places365.pth.tar
  - 类目（~6KB）：https://raw.githubusercontent.com/csailvision/places365/master/categories_places365.txt

  mit.edu 偶有拦截（413/503），脚本支持 --mirror 走 ghfast 等 CDN 镜像加速。
"""
import argparse
import os
import urllib.request

import torch
import torchvision.models as tv_models
from torchvision.models import ResNet

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "models")
WEIGHTS_URL_BASE = "http://places2.csail.mit.edu/models_places365/resnet18_places365.pth.tar"
CATEGORIES_URL = (
    "https://raw.githubusercontent.com/csailvision/places365/master/categories_places365.txt"
)
WEIGHTS_PATH = os.path.join(MODEL_DIR, "resnet18_places365.pth.tar")
ONNX_PATH = os.path.join(MODEL_DIR, "resnet18_places365.onnx")
CATEGORIES_PATH = os.path.join(MODEL_DIR, "categories_places365.txt")


def _download(url: str, dest: str, mirror: str | None):
    final = url
    if mirror and url.startswith("http://places2.csail.mit.edu/"):
        # mit 资源不走 ghfast；通过镜像代理仅当可达
        final = mirror.rstrip("/") + "/" + url
    print(f"[places] 下载 {final} → {dest}")
    req = urllib.request.Request(final, headers={"User-Agent": "Mozilla/5.0 VCR-Export/1.0"})
    with urllib.request.urlopen(req, timeout=60) as resp, open(dest, "wb") as f:
        while True:
            chunk = resp.read(1 << 16)
            if not chunk:
                break
            f.write(chunk)
    print(f"[places] OK {os.path.getsize(dest)/1e6:.1f} MB")


def build_model() -> ResNet:
    model = tv_models.resnet18(num_classes=365)
    state_dict = torch.load(WEIGHTS_PATH, map_location="cpu")
    if "state_dict" in state_dict:
        state_dict = state_dict["state_dict"]
    # 兼容官方权重键名（classifier.1.weight → fc.weight）
    keys = list(state_dict.keys())
    if keys and keys[0].startswith("module."):
        state_dict = {k[7:]: v for k, v in state_dict.items()}
    if "classifier.1.weight" in state_dict:
        model.fc.weight.data = state_dict.pop("classifier.1.weight")
        model.fc.bias.data = state_dict.pop("classifier.1.bias")
    model.load_state_dict(state_dict)
    return model


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mirror", default="", help="CDN 镜像前缀，如 https://ghfast.top")
    args = parser.parse_args()
    mirror = args.mirror.strip() or None
    os.makedirs(MODEL_DIR, exist_ok=True)

    # 1. 权重
    if not os.path.isfile(WEIGHTS_PATH):
        try:
            _download(WEIGHTS_URL_BASE, WEIGHTS_PATH, mirror)
        except Exception as e:  # noqa: BLE001
            print(f"[places] 下载失败: {e}\n请手动从官方地址下载后放到 {WEIGHTS_PATH}", file=__import__("sys").stderr)
            raise

    # 2. 类目表
    if not os.path.isfile(CATEGORIES_PATH):
        _download(CATEGORIES_URL, CATEGORIES_PATH, mirror)
        with open(CATEGORIES_PATH, encoding="utf-8") as f:
            n = sum(1 for _ in f if _.strip())
        assert n == 365, f"categories 应 365 行，实际 {n}"

    # 3. 导出 ONNX（已存在则跳过，避免重复导出耗时）
    if not os.path.isfile(ONNX_PATH):
        model = build_model().eval()
        dummy = torch.randn(1, 3, 224, 224)
        with torch.no_grad():
            torch.onnx.export(
                model,
                dummy,
                ONNX_PATH,
                input_names=["images"],
                output_names=["output0"],
                opset_version=12,
                dynamic_axes={"images": {0: "batch"}},
            )
        print(f"[places] OK → {ONNX_PATH} ({os.path.getsize(ONNX_PATH)/1e6:.1f} MB)")
    else:
        print(f"[skip] ONNX 已存在 {ONNX_PATH}")


if __name__ == "__main__":
    main()
