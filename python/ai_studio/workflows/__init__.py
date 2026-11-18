"""工作流注册表（FEAT-152 / FEAT-163 / FEAT-169）：tool 名 → run 函数。

云端四工具（journal/beauty/style_transfer/upscale/smart_edit）共用 provider 原语；
本地工具（film_sim/color_transfer/restore/auto_grade/local_beauty/face_reshape/ocr）
纯本机、零付费，只在这里做分发。
"""
from . import (
    auto_grade,
    beauty,
    color_transfer,
    film_sim,
    journal,
    local_beauty,
    ocr,
    reshape,
    recipe,
    restore,
    smart_edit,
    style_transfer,
    upscale,
)

REGISTRY = {
    # 一键出片（FEAT-168）：人工配方与大模型计划共用的执行入口
    "recipe": recipe.run,
    "journal": journal.run,
    "beauty": beauty.run,
    "style_transfer": style_transfer.run,
    "upscale": upscale.run,
    "smart_edit": smart_edit.run,
    # 本地增强（FEAT-163）
    "film_sim": film_sim.run,
    "color_transfer": color_transfer.run,
    "restore": restore.run,
    # 本地增强二期（FEAT-165）：零模型自适应调色 + 分区磨皮
    "auto_grade": auto_grade.run,
    "local_beauty": local_beauty.run,
    # 本地增强三期（FEAT-167）：68 点关键点驱动的瘦脸 / 大眼液化
    "face_reshape": reshape.run,
    # 文字提取（FEAT-171）：PaddleOCR det+rec 本机离线识别
    "ocr": ocr.run,
}

__all__ = ["REGISTRY"]
