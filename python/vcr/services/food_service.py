"""服务层：食物专家通道（Phase 5，可选，懒加载）

food101-resnet50.onnx（101 类食物）缺失时 run() 返回 None，食物判定依赖
映射补丁（hot pot/蟹/龙虾 → food）+ 现有温和修正。

触发条件在 pipeline 侧判断（cls=other/food 或 scene 命中餐厅词），
避免全量推理开销（模型 ~90MB）。
"""
from dataclasses import dataclass

import numpy as np
from PIL import Image

from .. import config, preprocess


@dataclass
class FoodOutcome:
    food_conf: float | None = None   # 专家模型食物置信度（None = 通道不可用）
    ready: bool = False
    error: str = ""


class FoodService:
    def __init__(self, registry):
        self.registry = registry

    def ready(self) -> bool:
        self.registry.food
        return self.registry.is_ready("food")

    def run(self, img: Image.Image) -> FoodOutcome:
        if not self.ready():
            return FoodOutcome(error="食物专家模型缺失")
        try:
            tensor = preprocess.food_tensor(img)
            out = self.registry.run("food", tensor)[0][0]        # (101,)
            probs = np.clip(out, 0.0, None)
            if probs.sum() > 0:
                probs = probs / probs.sum()
            return FoodOutcome(food_conf=float(probs.max()), ready=True)
        except Exception as e:  # noqa: BLE001
            return FoodOutcome(error=f"食物专家推理失败: {e}")


_food_service: FoodService | None = None


def get_food_service(registry) -> FoodService:
    global _food_service
    if _food_service is None:
        _food_service = FoodService(registry)
    return _food_service
