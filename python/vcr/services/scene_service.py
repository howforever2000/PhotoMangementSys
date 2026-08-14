"""服务层：场景通道（Places365, 可选）

ImageNet 是物体库，只有 ~11 个真场景类；Places365 有 365 个场景类
（自然/城市/街道/室内），能把「自然风景 vs 建筑城市 vs 街道」做准。

模型缺失时静默降级（scene=None），主流程不受影响。
类名 → 目标大类的映射规则见 _map_scene()。
"""
import os
import re
from dataclasses import dataclass

import numpy as np
from PIL import Image

from .. import config, preprocess


@dataclass
class SceneOutcome:
    category: str          # landscape_nature / architecture / street / indoor / other
    label: str
    confidence: float
    ready: bool = False
    error: str = ""


NATURE_PAT = re.compile(
    r"\b(mountain|valley|volcano|cliff|canyon|glacier|iceberg|waterfall|river|lake|ocean|"
    r"sea|beach|shore|coast|forest|woods|jungle|desert|dune|field|meadow|prairie|"
    r"hill|snow|ice|sky|sunset|sunrise|rainbow|aurora|night|star|cave|canyon|"
    r"waterfall|rapids|garden|park|zoo|pond|stream|island|lagoon|wetland|swamp|marsh|"
    r"orchard|vineyard|farm|field|pasture|rice|wheat|corn)\\b",
    re.I,
)
# Phase 4 拆分：城市风光 vs 建筑。harbor 归 cityscape（城市港口），从 NATURE_PAT 移除。
CITYSCAPE_PAT = re.compile(
    r"\b(skyscraper|downtown|city|urban|street|alley|plaza|boulevard|square|skyline|"
    r"metropolis|harbor|harbour|bridge|traffic|sidewalk|crosswalk|intersection|"
    r"highway|road|avenue|pavement|shopfront|storefront|arcade|promenade|esplanade|"
    r"market|bazaar|railway|station|airport|terminal|parking|construction)\b",
    re.I,
)
BUILDING_PAT = re.compile(
    r"\b(church|cathedral|castle|tower|mosque|temple|house|building|facade|mansion|"
    r"courtyard|apartment)\b",
    re.I,
)
INDOOR_PAT = re.compile(
    r"\b(room|kitchen|bedroom|living|bathroom|office|library|classroom|hall|"
    r"corridor|staircase|lobby|auditorium|theater|cinema|restaurant|cafe|bar|"
    r"gym|hospital|church|mosque|temple|house|home|indoor|basement|garage|"
    r"attic|dining|studio|workshop|factory|warehouse)\b",
    re.I,
)


class SceneService:
    def __init__(self, registry):
        self.registry = registry
        self._classes: list[str] = []
        self._load_classes()

    def _load_classes(self):
        path = os.path.join(config.MODEL_DIR, config.SCENE_CATEGORIES)
        if not os.path.isfile(path):
            return
        with open(path, encoding="utf-8") as f:
            self._classes = [ln.strip() for ln in f if ln.strip()]

    def ready(self) -> bool:
        return self.registry.is_ready("scene") and len(self._classes) == 365

    def _map_scene(self, name: str) -> str:
        """场景类名 → 内部类别。Phase 4：拆 cityscape / architecture。

        返回：landscape_nature / cityscape / architecture / indoor / other
        """
        if INDOOR_PAT.search(name) and not CITYSCAPE_PAT.search(name):
            return "indoor"
        if CITYSCAPE_PAT.search(name):
            return "cityscape"
        if BUILDING_PAT.search(name):
            return "architecture"
        if NATURE_PAT.search(name):
            return "landscape_nature"
        return "other"

    def run(self, img: Image.Image) -> SceneOutcome:
        if not self.ready():
            return SceneOutcome("other", "", 0.0)
        self.registry.scene  # 触发加载
        tensor = preprocess.scene_tensor(img)
        out = self.registry.run("scene", tensor)[0][0]
        probs = np.clip(out, 0.0, None)
        if probs.sum() > 0:
            probs = probs / probs.sum()
        top = int(np.argmax(probs))
        label = self._classes[top] if top < len(self._classes) else ""
        return SceneOutcome(
            category=self._map_scene(label),
            label=label,
            confidence=float(probs[top]),
            ready=True,
        )


_scene_service: SceneService | None = None


def get_scene_service(registry) -> SceneService:
    global _scene_service
    if _scene_service is None:
        _scene_service = SceneService(registry)
    return _scene_service
