"""ImageNet → 相册大类 映射加载与查询"""
import json
import os

from . import config


class CategoryMapping:
    def __init__(self):
        self._index_to_cat: dict[int, str] = {}
        self._index_to_sub: dict[int, str] = {}
        self._classes: list[str] = []
        self._desc: dict[str, str] = {}
        self._load()

    def _load(self):
        with open(config.CLASSES_PATH, encoding="utf-8") as f:
            self._classes = [line.strip().split(" ", 1)[1] for line in f if line.strip()]
        with open(config.MAPPING_PATH, encoding="utf-8") as f:
            m = json.load(f)
        cat_map = m.get("mapping", {})
        rev: dict[str, int] = {name: i for i, name in enumerate(self._classes)}
        for cat, names in cat_map.items():
            for n in names:
                if n in rev:
                    self._index_to_cat[rev[n]] = cat
        # 动物子类（狗/猫/鸟）：名字反查索引
        sub_rev: dict[str, int] = {}
        for sub, names in m.get("animal_sub", {}).items():
            for n in names:
                if n in rev:
                    sub_rev[rev[n]] = sub
        for i, cat in self._index_to_cat.items():
            if cat == "animal" and i in sub_rev:
                self._index_to_sub[i] = sub_rev[i]
        # 植物花卉子类（花/植物）：daisy/rapeseed → flower，其余（真菌/果）→ plant
        pf_sub_rev: dict[int, str] = {}
        for n, s in m.get("plant_flower_sub", {}).items():
            if n in rev:
                pf_sub_rev[rev[n]] = s
        for i, cat in self._index_to_cat.items():
            if cat == "plant_flower" and i in pf_sub_rev:
                self._index_to_sub[i] = pf_sub_rev[i]
        self._desc = m.get("meta", {}).get("category_desc", {})

    @property
    def classes(self) -> list[str]:
        return self._classes

    def category_of(self, idx: int) -> str:
        return self._index_to_cat.get(idx, "other")

    def sub_of(self, idx: int) -> str:
        return self._index_to_sub.get(idx, "")

    def desc(self, cat: str) -> str:
        return self._desc.get(cat, cat)


_mapping: CategoryMapping | None = None


def get_mapping() -> CategoryMapping:
    global _mapping
    if _mapping is None:
        _mapping = CategoryMapping()
    return _mapping
