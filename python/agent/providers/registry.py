"""档位 → 尺寸/质量 的映射。

**不再映射型号**：用户决定「不预设 provider」，model 名由用户填。
档位在"不预设"世界里的含义只剩两件事：
  1. 影响出图尺寸（草稿便宜、成品大）；
  2. 让 Agent 的工具调用参数保持稳定 —— 工具只传 `tier`，不传具体型号。

这条设计的价值：**换模型、加模型都不用改 Agent 循环**（docs/v5.md §18）。
"""
from __future__ import annotations

from .base import Endpoint

TIER_MAP: dict[str, dict[str, str]] = {
    "draft": {"size": "1024x1024", "label": "草稿 1K"},
    "standard": {"size": "2048x2048", "label": "成品 2K"},
    "hq": {"size": "2048x2048", "label": "高清 2K"},
    "local": {"size": "1024x1024", "label": "本地"},
}

DEFAULT_TIER = "draft"


def resolve_size(tier: str | None) -> str:
    return TIER_MAP.get((tier or DEFAULT_TIER).strip(), TIER_MAP[DEFAULT_TIER])["size"]


def tier_label(tier: str | None) -> str:
    return TIER_MAP.get((tier or DEFAULT_TIER).strip(), TIER_MAP[DEFAULT_TIER])["label"]


def resolve(tier: str | None, endpoint: Endpoint) -> tuple[str, str]:
    """返回 (model, size)。model 一律来自用户配置。"""
    return endpoint.model, resolve_size(tier)


def tiers_for_ui() -> list[dict]:
    """前端套餐菜单（2~4 项足够，多了只是让用户困惑）。"""
    return [
        {"id": "draft", "label": "草稿", "hint": "便宜先看效果", "size": TIER_MAP["draft"]["size"]},
        {"id": "standard", "label": "成品", "hint": "默认", "size": TIER_MAP["standard"]["size"]},
        {"id": "hq", "label": "高清", "hint": "最贵，只用于最终出图", "size": TIER_MAP["hq"]["size"]},
    ]
