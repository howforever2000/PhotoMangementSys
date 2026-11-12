"""智能修图 · 单图算子库（FEAT-152 扩展）

分层职责（CS1）：本模块是**原语层**——每个算子只负责「把一张 PIL 图片按参数交给
图像编辑原语处理」。不含规划 / 反思 / 版式 / 进度 / 作业逻辑（那些在
`pipeline.py` 与 `workflows/smart_edit.py`）。

四个算子对应既有的四项基准能力 + 反思微调，统一由 `pipeline` 的白名单解释器调用：
  beauty         人物美颜（presets.beauty_prompt）
  style_transfer 滤镜迁移 / 自主调色（presets.STYLE_TRANSFER_PROMPT / AUTOGRADE_BASE_PROMPT）
  upscale        超分（presets.upscale_prompt；独立 upscale provider，未配置回落 edit）
  refine         反思轮微调（presets.refine_prompt，按复核问题点重绘）
"""
from __future__ import annotations

from typing import Any, Callable, Dict

from PIL import Image

from . import presets
from .providers import openai_compat as oc
from .workflows.base import edit_input_bytes, to_image


def _edit_provider(providers: Any) -> Any:
    """取图像编辑 provider；未配置抛错（与既有 beauty/upscale 口径一致）。"""
    p = getattr(providers, "edit", None)
    if not p or not getattr(p, "baseUrl", "") or not getattr(p, "model", ""):
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")
    return p


def _run_edit(prov: Any, img: Image.Image, prompt: str) -> Image.Image:
    data = oc.images_edit(prov.baseUrl, prov.apiKey, prov.model, [edit_input_bytes(img)], prompt)
    return to_image(data[0])


def op_beauty(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    prov = _edit_provider(providers)
    prompt = presets.beauty_prompt(str(params.get("style", "natural")), str(params.get("level", "medium")))
    return _run_edit(prov, img, prompt)


def op_style_transfer(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    prov = _edit_provider(providers)
    mode = str(params.get("mode", "autograde"))
    extra = str(params.get("extra", "") or "").strip()
    if mode == "transfer":
        prompt = presets.STYLE_TRANSFER_PROMPT
        desc = str(params.get("referenceDesc", "") or "").strip()
        if desc:
            prompt += f" Reference style: {desc}."
        if extra:
            prompt += f" {extra}"
    else:
        prompt = presets.AUTOGRADE_BASE_PROMPT
        if extra:
            prompt += f" {extra}"
    return _run_edit(prov, img, prompt)


def op_upscale(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    prov = getattr(providers, "upscale", None) or getattr(providers, "edit", None)
    if not prov or not getattr(prov, "baseUrl", "") or not getattr(prov, "model", ""):
        raise ValueError("未配置超分 provider（设置里填写 baseUrl / model）")
    try:
        scale = max(1, min(4, int(params.get("scale", 2) or 2)))
    except Exception:
        scale = 2
    data = oc.images_edit(
        prov.baseUrl,
        prov.apiKey,
        prov.model,
        [edit_input_bytes(img, max_long_edge=3072, quality=95)],
        presets.upscale_prompt(scale),
    )
    return to_image(data[0])


def op_refine(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    """反思轮微调：按复核给出的问题点重绘（保持身份/构图）。"""
    prov = _edit_provider(providers)
    prompt = presets.refine_prompt(params.get("issues") or [], str(params.get("extra", "") or ""))
    return _run_edit(prov, img, prompt)


# 白名单算子注册表：pipeline 只认这里的键，plan 里的其它 op 一律被 validate 拦截
OPS: Dict[str, Callable[..., Image.Image]] = {
    "beauty": op_beauty,
    "style_transfer": op_style_transfer,
    "upscale": op_upscale,
    "refine": op_refine,
}

__all__ = ["OPS", "op_beauty", "op_style_transfer", "op_upscale", "op_refine"]
