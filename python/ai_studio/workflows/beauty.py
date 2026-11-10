"""人物美颜工作流（FEAT-152）：预设风格 × 级别 → 提示词 → 图生图 → 落盘。

与超分/滤镜共用 image edit 通道，差异只在 presets.beauty_prompt。
"""
from __future__ import annotations

from .. import presets
from ..providers import openai_compat as oc
from .base import check_cancel, png_bytes, read_image, save_image, stem_of, to_image


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")
    opts = req.options or {}
    style = str(opts.get("style", "natural"))
    level = str(opts.get("level", "medium"))
    prov = req.providers.edit
    if not prov or not prov.baseUrl or not prov.model:
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")

    prompt = presets.beauty_prompt(style, level, req.prompt)
    outputs: list[str] = []
    n = len(req.paths)
    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "美颜处理中", f"{i + 1}/{n}")
        img = read_image(path)
        data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [png_bytes(img)], prompt)
        out = save_image(to_image(data[0]), out_dir, f"{stem_of(path)}_美颜", ".jpg", quality=95)
        outputs.append(out)
    progress(100, "完成", f"{len(outputs)} 张")
    return {"outputs": outputs, "warnings": [], "meta": {"style": style, "level": level}}
