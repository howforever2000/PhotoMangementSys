"""滤镜迁移 / 自主调色工作流（FEAT-152 → FEAT-169）

两种模式：
  - transfer：把**参考图**的影调迁移到内容图（风格预设来源已按用户裁定下线）
  - autograde：只凭补充要求对内容图做专业调色

多图编辑实测（本网关 qwen-image-3.0）：送两张图会被当作「风格参考」重绘、
丢原图构图；因此 transfer 固定采用「视觉模型描述参考图风格 → 单图编辑 + 风格描述」，
以保证内容图的构图/主体不变、只迁移色调影调。

所有提示词都经 `presets.transfer_prompt` / `auto_grade_prompt` 拼装，
两者都会追加 `presets.STRUCTURE_LOCK` 结构保全硬段；出图后由调用方的
`critic.structure_compare` 做硬验收（见 `pipeline.execute_plan` 与工作流收尾）。
"""
from __future__ import annotations

import os

from .. import critic, presets
from ..providers import openai_compat as oc
from .base import (
    check_cancel,
    edit_input_bytes,
    filter_inputs,
    png_bytes,
    read_image,
    save_image,
    stem_of,
    to_image,
)


def _describe_reference(vision, ref_img) -> str:
    try:
        return oc.chat_vision(
            vision.baseUrl,
            vision.apiKey,
            vision.model,
            [png_bytes(ref_img)],
            "用不超过 60 字的中文描述这张参考图的色调、光线与整体影调风格。只输出描述本身。",
        ).strip()
    except Exception:
        return ""


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")
    opts = req.options or {}
    mode = str(opts.get("mode", "autograde"))
    if mode not in ("transfer", "autograde"):
        mode = "autograde"
    prov = req.providers.edit
    if not prov or not prov.baseUrl or not prov.model:
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")

    warnings: list[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    ref_path = str(opts.get("referencePath", "") or "")
    ref_img = None
    # transfer 必须有参考图：没有来源就只是空跑一次云端调用
    if mode == "transfer" and not ref_path:
        raise ValueError("滤镜迁移需要指定一张参考图")
    if ref_path:
        ref_img = read_image(ref_path)

    extra = (req.prompt or "").strip()
    ref_desc = ""
    if ref_img is not None:
        ref_desc = _describe_reference(req.providers.vision, ref_img) if req.providers.vision else ""
        if not ref_desc:
            warnings.append("未能从参考图提取风格描述，仅按补充要求迁移影调")

    outputs: list[str] = []
    drift = 0
    n = len(paths)
    for i, path in enumerate(paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "滤镜处理中", f"{i + 1}/{n}")
        img = read_image(path)
        if mode == "transfer":
            prompt = presets.transfer_prompt(ref_desc, extra)
            suffix = "_滤镜"
        else:
            prompt = presets.auto_grade_prompt(extra)
            suffix = "_调色"
        data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img)], prompt)
        result = to_image(data[0])

        # 硬验收：结构漂移就告警（本工具的策略是「报告而非回退」——迁移本身就是要改影调，
        # 直接回退原图等于什么都没做；执行器里的回退只用于链路中的中间步）
        check = critic.structure_compare(img, result)
        if check["issues"]:
            drift += 1
            warnings.append(f"{os.path.basename(path)}：结构可能被改动 —— " + "；".join(check["issues"]))

        outputs.append(save_image(result, out_dir, f"{stem_of(path)}{suffix}", ".jpg", quality=95))

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "mode": mode,
            "structureDrift": drift,
            "geometryPreserved": drift == 0,
        },
    }
