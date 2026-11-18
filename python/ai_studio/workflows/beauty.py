"""人物美颜工作流（FEAT-152 → FEAT-169）：预设风格 × 级别 → 提示词 → 图生图 → 落盘。

与超分/滤镜共用 image edit 通道，差异只在 presets.beauty_prompt。

FEAT-169 形状纪律：用户规则是「美颜保持当前人体形状不变，只做分区磨皮和细节」。
落实为三层：
  1. 提示词层：`presets.beauty_prompt` 已改为只描述皮肤质感，并强制追加结构硬段；
  2. 验收层：出图后用 `critic.structure_compare` 比对脸框比例与结构相关度；
  3. 兜底层：检出改形状就以原图作为该张产物并明确告警 —— 用户拿到的绝不是
     「脸被改过但没人说」的图。
"""
from __future__ import annotations

import os

from .. import critic, presets
from ..providers import openai_compat as oc
from .base import (
    check_cancel,
    edit_input_bytes,
    filter_inputs,
    read_image,
    save_image,
    stem_of,
    to_image,
)


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
    warnings: list[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    outputs: list[str] = []
    reverted = 0
    n = len(paths)
    for i, path in enumerate(paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "美颜处理中", f"{i + 1}/{n}")
        img = read_image(path)
        data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img)], prompt)
        result = to_image(data[0])

        # 改形状就整张回退原图：美颜的价值全在「看不出修过」，形状被改即负价值。
        check = critic.structure_compare(img, result)
        if not check["ok"]:
            reverted += 1
            warnings.append(
                f"{os.path.basename(path)}：模型改动了脸型或构图，已改用原图（"
                + "；".join(check["issues"])
                + "）。想要稳定的「只磨皮」效果请用「本地美颜」。"
            )
            result = img

        out = save_image(result, out_dir, f"{stem_of(path)}_美颜", ".jpg", quality=95)
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "style": style,
            "level": level,
            "structureReverted": reverted,
            "geometryPreserved": reverted == 0,
        },
    }
