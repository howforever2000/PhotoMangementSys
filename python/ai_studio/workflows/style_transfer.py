"""滤镜迁移 / 自主调色工作流（FEAT-152）

两种模式：
  - transfer：输入「内容图 + 参考图」，把参考图的色调/影调迁移到内容图
  - autograde：只凭提示词对内容图做专业调色

多图编辑实测（本网关 qwen-image-3.0）：送两张图会被当作「风格参考」重绘、
丢原图构图；因此 transfer 固定采用「视觉模型描述参考图风格 → 单图编辑 + 风格描述」，
以保证内容图的构图/主体不变、只迁移色调影调。
"""
from __future__ import annotations

from .. import presets
from ..providers import openai_compat as oc
from .base import check_cancel, edit_input_bytes, png_bytes, read_image, save_image, stem_of, to_image


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
    mode = str(opts.get("mode", "transfer"))
    prov = req.providers.edit
    if not prov or not prov.baseUrl or not prov.model:
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")

    warnings: list[str] = []
    ref_path = str(opts.get("referencePath", "") or "")
    ref_img = None
    if mode == "transfer":
        if not ref_path:
            raise ValueError("滤镜迁移需要一张参考图")
        ref_img = read_image(ref_path)

    extra = (req.prompt or "").strip()
    ref_desc = ""
    if mode == "transfer":
        ref_desc = _describe_reference(req.providers.vision, ref_img) if req.providers.vision else ""
        if not ref_desc:
            warnings.append("未能从参考图提取风格描述，仅按提示词迁移色调")
    outputs: list[str] = []
    n = len(req.paths)
    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "滤镜处理中", f"{i + 1}/{n}")
        img = read_image(path)
        if mode == "transfer":
            prompt = presets.STYLE_TRANSFER_PROMPT
            if ref_desc:
                prompt += f" Reference style: {ref_desc}."
            if extra:
                prompt += f" {extra}"
            data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img)], prompt)
            suffix = "_滤镜"
        else:
            prompt = presets.AUTOGRADE_BASE_PROMPT + (f" {extra}" if extra else "")
            data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img)], prompt)
            suffix = "_调色"
        out = save_image(to_image(data[0]), out_dir, f"{stem_of(path)}{suffix}", ".jpg", quality=95)
        outputs.append(out)
    progress(100, "完成", f"{len(outputs)} 张")
    return {"outputs": outputs, "warnings": warnings, "meta": {"mode": mode}}
