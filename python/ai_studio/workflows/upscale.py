"""超分工作流（FEAT-152）：每张图 → 超分 provider → 落盘。

超分走**独立配置的 upscale provider**（baseUrl + model 单独指定）；未配置时回落 edit，
并用「放大 {scale}x」提示词表达意图（服务商若不支持真超分，至少做增强）。
"""
from __future__ import annotations

from .. import presets
from ..providers import openai_compat as oc
from .base import check_cancel, edit_input_bytes, read_image, save_image, stem_of, to_image


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")
    scale = int((req.options or {}).get("scale", 2) or 2)
    prov = req.providers.upscale or req.providers.edit
    if not prov or not prov.baseUrl or not prov.model:
        raise ValueError("未配置超分 provider（设置里填写 baseUrl / model）")

    outputs: list[str] = []
    n = len(req.paths)
    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "超分处理中", f"{i + 1}/{n}")
        img = read_image(path)
        data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img, max_long_edge=3072, quality=95)], presets.upscale_prompt(scale))
        out = save_image(to_image(data[0]), out_dir, f"{stem_of(path)}_超分", ".jpg", quality=95)
        outputs.append(out)
    progress(100, "完成", f"{len(outputs)} 张")
    return {"outputs": outputs, "warnings": [], "meta": {"scale": scale}}
