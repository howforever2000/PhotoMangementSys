"""工作流注册表（FEAT-152）：tool 名 → run 函数。

四个工具共用同一套 provider 原语，只在这里做「工具名 → 预设工作流」的分发。
"""
from . import beauty, journal, style_transfer, upscale

REGISTRY = {
    "journal": journal.run,
    "beauty": beauty.run,
    "style_transfer": style_transfer.run,
    "upscale": upscale.run,
}

__all__ = ["REGISTRY"]
