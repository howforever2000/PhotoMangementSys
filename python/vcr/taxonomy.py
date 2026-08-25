"""taxonomy 收敛层：album_groups.json 加载 + 类别折叠

server.py 输出前调用 fold(category)，保证前端拿到的 category 一定是 9 组之一：
    portrait / street / animal / food / flower / landscape / cityscape / night_scene / document

折叠映射见 python/models/album_groups.json（缺文件时用 _DEFAULT_FOLD 兜底）。
"""
import json
import os

from . import config

GROUPS = [
    "portrait", "street", "animal", "food", "flower",
    "landscape", "cityscape", "night_scene", "document",
]

# 默认折叠（album_groups.json 缺失时兜底，与 json 文件保持一致）
_DEFAULT_FOLD = {
    "architecture": "cityscape",     # 建筑 → 城市风光（Phase 4 收敛前兜底）
    "plant_flower": "flower",        # 植物花卉 → 花朵
    "plant": "other",                # 专家拆分后的纯植物
    "text": "document",              # 文本截图 → 文档
    "sports": "other",
    "vehicle": "other",
    "indoor": "other",               # 场景通道内部类别
    "landscape_nature": "landscape", # 自然风景 → landscape
}


class Taxonomy:
    def __init__(self):
        self._fold: dict[str, str] = dict(_DEFAULT_FOLD)
        self._groups: list[str] = list(GROUPS)
        self._conflicts: list[str] = []
        self._load()

    def _load(self):
        path = config.ALBUM_GROUPS
        if not os.path.isfile(path):
            return
        try:
            with open(path, encoding="utf-8") as f:
                data = json.load(f)
        except Exception:
            return
        if isinstance(data.get("groups"), list) and data["groups"]:
            self._groups = list(data["groups"])
        fold = data.get("fold", {})
        self._fold.update({k: v for k, v in fold.items() if k and v})
        self._conflicts = list(data.get("conflicts", []))

    # ------------------------------------------------------------------
    def groups(self) -> list[str]:
        return list(self._groups)

    def conflicts(self) -> list[str]:
        return list(self._conflicts)

    def fold(self, category: str) -> str:
        """把内部类别折叠到 9 组之一；未列出且不在组的归 other。"""
        if category in self._groups:
            return category
        return self._fold.get(category, "other")

    def fold_top3(self, top3: list[dict]) -> list[dict]:
        out = []
        for t in top3:
            out.append({**t, "category": self.fold(t.get("category", "other"))})
        return out


_tax: Taxonomy | None = None


def get_taxonomy() -> Taxonomy:
    global _tax
    if _tax is None:
        _tax = Taxonomy()
    return _tax
