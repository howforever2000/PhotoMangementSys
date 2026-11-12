"""工作流注册表（FEAT-152 / FEAT-163）：tool 名 → run 函数。

云端四工具（journal/beauty/style_transfer/upscale/smart_edit）共用 provider 原语；
本地三工具（film_sim/color_transfer/restore）纯本机、零付费，只在这里做分发。
"""
from . import (
    beauty,
    color_transfer,
    film_sim,
    journal,
    restore,
    smart_edit,
    style_transfer,
    upscale,
)

REGISTRY = {
    "journal": journal.run,
    "beauty": beauty.run,
    "style_transfer": style_transfer.run,
    "upscale": upscale.run,
    "smart_edit": smart_edit.run,
    # 本地增强（FEAT-163）
    "film_sim": film_sim.run,
    "color_transfer": color_transfer.run,
    "restore": restore.run,
}

__all__ = ["REGISTRY"]
