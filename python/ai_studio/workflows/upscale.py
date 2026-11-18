"""超分工作流（FEAT-152 → FEAT-169）：每张图 → 超分 provider → 落盘。

超分走**独立配置的 upscale provider**（baseUrl + model 单独指定）；未配置时回落 edit，
并用「放大 {scale}x」提示词表达意图（服务商若不支持真超分，至少做增强）。

FEAT-169：提示词经 `presets.upscale_prompt` 拼装并带结构保全硬段；出图后做结构验收
（超分是唯一**允许放大**的保真工具，所以这里只核对宽高比，不参与「回退」策略）。
"""
from __future__ import annotations

import os

from .. import critic, presets
from ..providers import openai_compat as oc
from .base import check_cancel, edit_input_bytes, filter_inputs, read_image, save_image, stem_of, to_image


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

    warnings: list[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    outputs: list[str] = []
    n = len(paths)
    for i, path in enumerate(paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "超分处理中", f"{i + 1}/{n}")
        img = read_image(path)
        data = oc.images_edit(
            prov.baseUrl,
            prov.apiKey,
            prov.model,
            [edit_input_bytes(img, max_long_edge=3072, quality=95)],
            presets.upscale_prompt(scale),
        )
        result = to_image(data[0])
        check = critic.structure_compare(img, result)
        # 只报画幅比问题：像素尺寸变化是超分的本职，宽高比变了才是构图被改
        aspect = [x for x in check["issues"] if "画幅比例" in x]
        if aspect:
            warnings.append(f"{os.path.basename(path)}：画幅比例被改动 —— " + "；".join(aspect))
        out = save_image(result, out_dir, f"{stem_of(path)}_超分", ".jpg", quality=95)
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {"outputs": outputs, "warnings": warnings, "meta": {"scale": scale}}
