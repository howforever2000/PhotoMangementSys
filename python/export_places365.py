"""导出 Places365 场景模型为 ONNX（仅需执行一次）

依赖：torch/torchvision（导出后推理只需 onnxruntime）
模型：ResNet18 trained on Places365（官方 MIT 权重）

用法: python export_places365.py
产出: models/resnet18_places365.onnx + models/categories_places365.txt
"""
import os

import torch
import torchvision.models as tv_models
from torchvision.models import ResNet

HERE = os.path.dirname(os.path.abspath(__file__))
MODEL_DIR = os.path.join(HERE, "models")
WEIGHTS_URL = "http://places2.csail.mit.edu/models_places365/resnet18_places365.pth.tar"
CATEGORIES_URL = (
    "https://raw.githubusercontent.com/csailvision/places365/master/categories_places365.txt"
)
WEIGHTS_PATH = os.path.join(MODEL_DIR, "resnet18_places365.pth.tar")
ONNX_PATH = os.path.join(MODEL_DIR, "resnet18_places365.onnx")
CATEGORIES_PATH = os.path.join(MODEL_DIR, "categories_places365.txt")


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
    os.makedirs(MODEL_DIR, exist_ok=True)

    # 1. 权重
    if not os.path.isfile(WEIGHTS_PATH):
        print("[places] 下载权重 …")
        import urllib.request

        urllib.request.urlretrieve(WEIGHTS_URL, WEIGHTS_PATH)
    # 2. 类目表
    if not os.path.isfile(CATEGORIES_PATH):
        import urllib.request

        urllib.request.urlretrieve(CATEGORIES_URL, CATEGORIES_PATH)
        with open(CATEGORIES_PATH, encoding="utf-8") as f:
            n = sum(1 for _ in f if _.strip())
        assert n == 365, f"categories 应 365 行，实际 {n}"

    # 3. 导出 ONNX
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


if __name__ == "__main__":
    main()
