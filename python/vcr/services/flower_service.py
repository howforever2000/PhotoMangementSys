"""服务层：花朵专家通道（Phase 3，可选，懒加载）

efficientnet-b0-flowers.onnx（oxford-102 花朵）缺失时 run() 返回 None，
仲裁器退化为 taxonomy 折叠：plant_flower → flower 组（现状行为）。

模型就绪时：cls_cat == plant_flower 才触发，置信度 ≥ FLOWER_CONF → flower，
否则 plant（纯植物归 other 组）。
"""
import os
from dataclasses import dataclass

import numpy as np
from PIL import Image

from .. import config, preprocess

FLOWER_CLASSES = "flower_classes.txt"   # 102 行类名（模型配套）


@dataclass
class FlowerOutcome:
    flower_conf: float | None = None   # 专家模型花朵置信度（None = 通道不可用）
    ready: bool = False
    error: str = ""


class FlowerService:
    def __init__(self, registry):
        self.registry = registry

    def ready(self) -> bool:
        self.registry.flower
        return self.registry.is_ready("flower")

    def run(self, img: Image.Image) -> FlowerOutcome:
        if not self.ready():
            return FlowerOutcome(error="花朵专家模型缺失")
        try:
            tensor = preprocess.flower_tensor(img)
            out = self.registry.run("flower", tensor)[0][0]      # (102,)
            probs = np.clip(out, 0.0, None)
            if probs.sum() > 0:
                probs = probs / probs.sum()
            conf = float(probs.max())
            return FlowerOutcome(flower_conf=conf, ready=True)
        except Exception as e:  # noqa: BLE001
            return FlowerOutcome(error=f"花朵专家推理失败: {e}")


_flower_service: FlowerService | None = None


def get_flower_service(registry) -> FlowerService:
    global _flower_service
    if _flower_service is None:
        _flower_service = FlowerService(registry)
    return _flower_service
