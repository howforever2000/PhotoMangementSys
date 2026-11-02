"""本地调色参数表：**唯一真相**。

为什么单独一个模块（不是写在 style.py 里）：
「高级区执行参数预览」要显示的就是这几个数字（色温 +8 / 高光 -15 / 饱和 +6），
如果预览从一处算、实际执行从另一处算，用户看到的参数会和结果不符 ——
那比不显示更糟。

强度三档按固定倍率缩放（不插值）：同一个强度档在任何图上都用同一组数字，
批量时才能保持"逐字一致"的一致性卖点。
"""
from __future__ import annotations

# 基准值对应 intensity=moderate（UI 上显示的就是这一组）
PRESETS: dict[str, dict[str, int]] = {
    # 黄昏暖调：色温偏暖、压高光、压阴影、略提饱和
    "warm_dusk": {"temp": 8, "highlight": -15, "shadow": -6, "saturation": 6},
    # 柯达胶片：更暖一点、高光柔和、饱和略高
    "film_kodak": {"temp": 5, "highlight": -9, "shadow": 4, "saturation": 10},
    # 清透空气：偏冷、高光通透、饱和降低
    "clean_air": {"temp": -6, "highlight": 6, "shadow": 8, "saturation": -8},
}

SCALE: dict[str, float] = {"subtle": 0.55, "moderate": 1.0, "strong": 1.5}

DEFAULT_PRESET = "warm_dusk"


def preset_params(preset: str, level: str) -> dict[str, int]:
    """取某个预设在某强度档下的参数（一次执行内不再变化）。"""
    base = PRESETS.get(preset)
    if base is None:
        base = PRESETS[DEFAULT_PRESET]
    k = SCALE.get(level, 1.0)
    return {name: int(round(v * k)) for name, v in base.items()}


def preset_label(preset: str) -> str:
    return {
        "warm_dusk": "黄昏暖调",
        "film_kodak": "柯达胶片",
        "clean_air": "清透空气",
    }.get(preset, preset)
