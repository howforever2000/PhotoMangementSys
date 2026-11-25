"""智能修图 · 单图算子库（FEAT-152 扩展）

分层职责（CS1）：本模块是**原语层**——每个算子只负责「把一张 PIL 图片按参数交给
图像编辑原语处理」。不含规划 / 反思 / 版式 / 进度 / 作业逻辑（那些在
`pipeline.py` 与 `workflows/smart_edit.py`）。

云端四算子（需要外部 provider）：
  beauty         人物美颜（presets.beauty_prompt）
  style_transfer 滤镜迁移 / 自主调色（presets.STYLE_TRANSFER_PROMPT / AUTOGRADE_BASE_PROMPT）
  upscale        超分（presets.upscale_prompt；独立 upscale provider，未配置回落 edit）
  refine         反思轮微调（presets.refine_prompt，按复核问题点重绘）

本地两算子（FEAT-164，纯本机、免费、离线，`providers` 参数不用）：
  local_film     内置 .cube 胶片模拟（lutcube，可加颗粒/暗角）
  local_upscale  Real-ESRGAN ×4 超分（localmodels，需本机模型就绪）

两类算子签名统一为 `fn(img, params, providers) -> Image`，由 `pipeline.execute_plan`
按白名单线性调用 —— 这样「AI 规划器可调用本地组件」不需要另起一条执行链路。
"""
from __future__ import annotations

import os
from typing import Any, Callable, Dict

import numpy as np
from PIL import Image

from . import (catalog, config, faceparse, film_render, film_styles, landmark, localmodels,
               ocr_engine, presets)
from .providers import openai_compat as oc
from .renderopts import EMPTY as EMPTY_CTX, RunContext
from .workflows.base import edit_input_bytes, to_image

# 本地超分输入的长边上限：与 workflows/restore.py 同一口径，避免几百 MP 的内存峰值
_LOCAL_UP_MAX_LONG = 2000


def _edit_provider(providers: Any) -> Any:
    """取图像编辑 provider；未配置抛错（与既有 beauty/upscale 口径一致）。"""
    p = getattr(providers, "edit", None)
    if not p or not getattr(p, "baseUrl", "") or not getattr(p, "model", ""):
        raise ValueError("未配置图像编辑 provider（设置里填写 baseUrl / model）")
    return p


def _run_edit(
    prov: Any,
    img: Image.Image,
    prompt: str,
    ctx: RunContext = EMPTY_CTX,
    styled: bool = True,
) -> Image.Image:
    """云端编辑一次：输入走 1K 压缩（`edit_input_bytes` 默认长边 1024），
    采样步数由作业级 `ctx` 下发（默认档不写进请求体，见 `renderopts`）。

    `styled=True` 时把作业级风格段落（`ctx.style_prompt`）追加到提示词末尾 ——
    这是风格提示词**唯一真正生效**的位置（塞进 `params.extra` 会被 500 字截断）。
    超分步（`op_upscale`）显式传 `styled=False`：那一步只负责分辨率，把影调段落
    再叠一遍等于把已经调好的颜色重刷一次，而且会让「4× = 2× 跑两遍」各自带一次
    风格重绘，颜色逐遍漂移。
    """
    data = oc.images_edit(
        prov.baseUrl,
        prov.apiKey,
        prov.model,
        [edit_input_bytes(img)],
        ctx.apply_style(prompt) if styled else prompt,
        steps=ctx.gateway_steps(),
    )
    return to_image(data[0])


def op_beauty(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    prov = _edit_provider(providers)
    prompt = presets.beauty_prompt(str(params.get("style", "natural")), str(params.get("level", "medium")))
    return _run_edit(prov, img, prompt, ctx)


def op_style_transfer(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """滤镜迁移 / 自主调色（仅参考图来源）：提示词一律带结构保全硬段。"""
    prov = _edit_provider(providers)
    mode = str(params.get("mode", "autograde"))
    extra = str(params.get("extra", "") or "").strip()
    desc = str(params.get("referenceDesc", "") or "").strip()
    if mode == "transfer":
        prompt = presets.transfer_prompt(desc, extra)
    else:
        prompt = presets.auto_grade_prompt(extra)
    return _run_edit(prov, img, prompt, ctx)


def op_upscale(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """云端超分一次。`scale` 只进提示词（网关没有倍率参数），所以「4×」由**两次 2×**
    组合实现（见 `ai_studio/upsample.py`），比一次写 4× 更接近真实细节增益。"""
    ctx.mark_geometry_changed()  # 整幅重绘/缩放：磨皮留下的掩码与新图不再对齐（FEAT-208）
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
        steps=ctx.gateway_steps(),
    )
    return to_image(data[0])


def op_refine(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """反思轮微调：按复核给出的问题点重绘（保持身份/构图）。"""
    prov = _edit_provider(providers)
    prompt = presets.refine_prompt(params.get("issues") or [], str(params.get("extra", "") or ""))
    return _run_edit(prov, img, prompt, ctx)


# ---------------- 本地组件（FEAT-164）：纯本机、免费、离线 ----------------
#
# 与上面四个算子的区别只有「不看 providers、不发网络请求」；签名保持一致
# （含 `ctx`，本机算子收下不使用），这样 pipeline 的白名单解释器不需要区分云端 / 本地。


def _clamp01(v: Any, default: float) -> float:
    try:
        return max(0.0, min(1.0, float(v)))
    except Exception:
        return default


def _limit_long_edge(img: Image.Image, max_long: int) -> Image.Image:
    w, h = img.size
    m = max(w, h)
    if m <= max_long:
        return img
    s = max_long / float(m)
    return img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)


def op_local_film(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """本机胶片模拟：内置 .cube 查表（默认）或参数化引擎，可选颗粒/暗角。不读 providers / ctx。

    引擎分支统一在 `film_render.apply`，与批量工作流 `workflows/film_sim` 同一份像素实现，
    避免「预览一个样、出片另一个样」。
    """
    style_id = str(params.get("style") or params.get("lut") or "provia")
    engine, _recipe = film_styles.resolve(style_id, str(params.get("engine") or "auto"))

    lut_path = None
    if engine == "lut":
        entry = film_styles.lut_entry(style_id)
        if entry is None:
            raise ValueError(f"未知的胶片风格: {style_id}")
        lut_path = config.lut_path(entry["file"])
        if not os.path.isfile(lut_path):
            raise FileNotFoundError(f"内置 LUT 缺失: {entry['file']}")

    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    out, _meta = film_render.apply(
        arr,
        {
            "style": style_id,
            "engine": engine,
            "strength": _clamp01(params.get("strength", 1.0), 1.0),
            "look": float(params.get("look", 1.0) or 1.0),
            "grade": str(params.get("grade") or ""),
            "grade_strength": float(params.get("grade_strength", 1.0) or 1.0),
            "anchor": _clamp01(params.get("anchor", 0.0), 0.0),
            "grain": _clamp01(params.get("grain", 0.0), 0.0),
            "vignette": _clamp01(params.get("vignette", 0.0), 0.0),
        },
        lut_file_path=lut_path,
        seed=0,
    )
    return Image.fromarray(out)


def op_local_upscale(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """本机 Real-ESRGAN ×4 超分（2× 时再降采样）。模型未就绪抛 FileNotFoundError。"""
    ctx.mark_geometry_changed()  # 整幅重绘/缩放：磨皮留下的掩码与新图不再对齐（FEAT-208）
    if not localmodels.esrgan_ready():
        raise FileNotFoundError(
            "本地超分模型未就绪（real_esrgan_x4.onnx），请在「模型设置 › 本地模型」下载后重试"
        )
    try:
        scale = int(params.get("scale", 4) or 4)
    except Exception:
        scale = 4
    if scale not in (2, 4):
        scale = 4

    src = _limit_long_edge(img.convert("RGB"), _LOCAL_UP_MAX_LONG)
    up = localmodels.run_esrgan(src, tile=256)
    if scale == 2:
        up = up.resize((src.width * 2, src.height * 2), Image.LANCZOS)
    # 守卫：超分绝不允许输出比输入小。`_limit_long_edge` 会把长边压到 2000，
    # 2× 后只有 4000，对 6000px 原图就是净缩水 33%（BUG-2026-1117-003）。
    # 真发生缩水时退回原图——「没放大」远好于「悄悄变糊」。
    if up.width * up.height < img.width * img.height:
        return img.convert("RGB")
    return up


# 白名单算子注册表：以 `op_catalog.json` 为唯一真相源，按声明的 module 惰性解析。
# 既有云端 / 本地算子保留在本模块，新增的原子算子在 ops_local.py；漏了模块会在
# 首次解析时明确抛错，而不是静默跳过。
OPS: Dict[str, Callable[..., Image.Image]] = {
    "beauty": op_beauty,
    "style_transfer": op_style_transfer,
    "upscale": op_upscale,
    "refine": op_refine,
    "local_film": op_local_film,
    "local_upscale": op_local_upscale,
}

# 本地算子集合：由清单 backend 直接推导，不再手写（编排层据此判降级与计费）
LOCAL_OPS = catalog.local_ops()


def _resolve(op_id: str) -> Callable[..., Image.Image] | None:
    """按清单的 module 找算子实现。OPS 里的既有实现优先，其余按 module 导入。"""
    fn = OPS.get(op_id)
    if fn is not None:
        return fn
    spec = catalog.op_spec(op_id)
    if spec is None:
        return None
    mod = spec.get("module", "")
    if mod == "ai_studio.ops_local":
        from . import ops_local  # 惰性导入：避免 server import 期拉起 onnxruntime
        return ops_local.OPS_LOCAL.get(op_id)
    return None


def resolve(op_id: str) -> Callable[..., Image.Image] | None:
    """执行器唯一入口：按 id 取算子函数（含清单里新增的原子算子）。"""
    return _resolve(op_id)


def _paddleocr_rec_ready() -> bool:
    """文字提取识别模型就绪（rec onnx 在工坊区；字典随包内置不检查）。"""
    return ocr_engine.rec_ready()


# 清单用模型 id 声明依赖，这里把 id 映射到就绪判定（onnxruntime 在 localmodels 里惰性导入）
_MODEL_READY = {
    "det_10g": localmodels.face_det_ready,
    "real_esrgan_x4": localmodels.esrgan_ready,
    "face_parsing_r34": faceparse.ready,
    "face_landmark_68": landmark.ready,
    "paddleocr_rec": _paddleocr_rec_ready,
}


def local_op_ready(op: str) -> bool:
    """本地算子当前是否可跑。**完全由清单的 requires 驱动**，不再逐 op 写 if 分支。

    编排层用它在规划后摘掉不可用的本地步骤：本机组件是可选增强，
    缺资源时应降级并给提示，而不是让整单作业失败。
    """
    spec = catalog.op_spec(op)
    if spec is None or spec.get("backend") != "local":
        return True
    req = spec.get("requires") or {}
    if req.get("luts"):
        if not any(os.path.isfile(config.lut_path(lut["file"])) for lut in config.LUTS):
            return False
    for model_id in req.get("models") or []:
        ready = _MODEL_READY.get(model_id)
        if ready is not None and not ready():
            return False
    return True


__all__ = [
    "OPS",
    "LOCAL_OPS",
    "resolve",
    "local_op_ready",
    "op_beauty",
    "op_style_transfer",
    "op_upscale",
    "op_refine",
    "op_local_film",
    "op_local_upscale",
]
