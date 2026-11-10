"""主题手账工作流（FEAT-152）

这是唯一的多步编排（agent 式流水线）：
  1) 视觉模型：一次读全部照片 → 提炼主题 + 每张配文（JSON）
  2) 图像编辑模型：逐张风格化（素描/水彩/水墨/胶片）
  3) 确定性排版：Pillow 按模板把「风格化图 + 主题 + 配文」排成 A4
  4) 导出：1 张 → JPG；多张 → 多页 PDF（并附首图预览）

失败降级：视觉模型不可用 → 主题/配文留空仍可出图；风格化失败 → 该张用原图占位。
"""
from __future__ import annotations

from .. import compose, presets
from ..providers import openai_compat as oc
from .base import (
    check_cancel,
    parse_json_loose,
    png_bytes,
    read_image,
    stem_of,
    to_image,
)


def _vision_prompt(n: int, extra: str) -> str:
    extra = (extra or "").strip() or "无"
    return (
        f"你是手账编辑。请阅读我提供的 {n} 张照片，完成两件事：\n"
        f"1) 提炼一个不超过 12 个字的中文主题；\n"
        f"2) 为每张照片写一句不超过 24 个字的中文配文，语气温和、有画面感。\n"
        f"用户补充要求：{extra}\n"
        f"只输出 JSON，不要任何解释："
        f'{{"theme":"主题","captions":["配文1","配文2"]}}，'
        f"captions 必须正好 {n} 条，顺序与照片一致。"
    )


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    template = str(opts.get("template", "blank"))
    style = str(opts.get("style", "sketch"))
    n = len(req.paths)
    warnings: list[str] = []

    # ---- 1) 理解：主题 + 配文 ----
    theme, captions = "", []
    vp = req.providers.vision
    images = [read_image(p) for p in req.paths]
    if vp and vp.baseUrl and vp.model:
        progress(6, "理解照片主题", f"{n} 张")
        try:
            text = oc.chat_vision(
                vp.baseUrl, vp.apiKey, vp.model,
                [png_bytes(img) for img in images],
                _vision_prompt(n, req.prompt),
            )
            data = parse_json_loose(text)
            theme = str(data.get("theme") or "").strip()
            caps = data.get("captions") or []
            captions = [str(c).strip() for c in caps if str(c).strip()]
        except Exception as e:
            warnings.append(f"主题/配文生成失败，留空继续出图：{e}")
    else:
        warnings.append("未配置图像理解 provider，主题与配文留空")

    while len(captions) < n:
        captions.append("")
    captions = captions[:n]

    # ---- 2) 风格化 ----
    ep = req.providers.edit
    if not ep or not ep.baseUrl or not ep.model:
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")
    styled = []
    for i, img in enumerate(images):
        check_cancel(cancel)
        progress(10 + int(i / n * 72), "风格化", f"{i + 1}/{n}")
        try:
            data = oc.images_edit(
                ep.baseUrl, ep.apiKey, ep.model,
                [png_bytes(img)],
                presets.journal_style_prompt(style, theme),
            )
            styled.append(to_image(data[0]))
        except Exception as e:
            warnings.append(f"{stem_of(req.paths[i])}: 风格化失败，改用原图（{e}）")
            styled.append(img)

    # ---- 3) 排版 + 导出 ----
    check_cancel(cancel)
    progress(86, "排版合成", f"{n} 页")
    stem = "主题" if n > 1 else stem_of(req.paths[0])
    outputs = compose.export_journal(list(zip(styled, captions)), template, theme, out_dir, stem)
    progress(100, "完成", "已导出")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {"theme": theme, "template": template, "style": style, "pages": n},
    }
