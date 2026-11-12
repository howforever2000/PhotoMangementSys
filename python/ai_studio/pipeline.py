"""智能修图 · 计划校验与执行（FEAT-152 扩展）

分层职责（CS1）：
  - `validate_plan`  把 planner 返回的宽松 JSON 归一成**受约束计划**（白名单算子、
                     参数夹取、顺序规范化、步数上限）；非法抛 `PlanError`。
  - `execute_plan`   按计划线性串联 `ops` 白名单算子（后一步吃前一步输出）。
  - `verify_result`  用多模态模型复核结果是否满足意图，返回 `{pass, issues}`。

本模块不含 provider 协议细节（在 `providers/openai_compat.py`）、不含提示词文本
（在 `presets.py`）、不含作业/进度框架（在 `workflows/smart_edit.py`）。
"""
from __future__ import annotations

from typing import Any, Dict, List

from PIL import Image

from . import ops, presets
from .providers import openai_compat as oc
from .workflows.base import check_cancel, parse_json_loose, png_bytes

# 只有这三个算子能进「计划」——refine 只由反思循环内部调用，不允许 planner 直接编排。
ALLOWED_OPS = ("beauty", "style_transfer", "upscale")
_ORDER = {"beauty": 0, "style_transfer": 1, "upscale": 2}
MAX_STEPS = 4
_MAX_SCALE = 4


class PlanError(ValueError):
    """计划非法（planner 输出不满足白名单/约束）。planner 可据此错误重试一次。"""


def _norm_step(raw: Any) -> Dict[str, Any]:
    if not isinstance(raw, dict):
        raise PlanError("步骤必须是对象")
    op = str(raw.get("op") or "").strip()
    if op not in ALLOWED_OPS:
        raise PlanError(f"不支持的算子: {op or '(空)'}，仅允许 {', '.join(ALLOWED_OPS)}")
    params = raw.get("params")
    if not isinstance(params, dict):
        params = {}
    # 参数归一 / 夹取，避免模型给出越界值
    if op == "upscale":
        try:
            params["scale"] = max(1, min(_MAX_SCALE, int(params.get("scale", 2) or 2)))
        except Exception:
            params["scale"] = 2
    elif op == "beauty":
        if str(params.get("style", "")) not in ("natural", "crisp", "film"):
            params["style"] = "natural"
        if str(params.get("level", "")) not in ("light", "medium", "strong"):
            params["level"] = "medium"
    elif op == "style_transfer":
        if str(params.get("mode", "")) not in ("transfer", "autograde"):
            params["mode"] = "autograde"
        if params.get("mode") == "transfer" and not str(params.get("referenceDesc", "")).strip():
            params["mode"] = "autograde"  # 没有参考描述就退回自主调色，避免空迁移
    return {"op": op, "params": params}


def validate_plan(raw: Dict[str, Any]) -> Dict[str, Any]:
    """宽松 JSON → 受约束计划。非法抛 PlanError。"""
    if not isinstance(raw, dict):
        raise PlanError("计划必须是 JSON 对象")
    steps_raw = raw.get("steps")
    if not isinstance(steps_raw, list) or not steps_raw:
        raise PlanError("计划缺少 steps 或为空")
    steps = [_norm_step(s) for s in steps_raw]
    # 规范化顺序：美颜 → 调色 → 超分；同 op 保持原相对次序
    steps = [s for _, s in sorted(enumerate(steps), key=lambda kv: (_ORDER[kv[1]["op"]], kv[0]))]
    if len(steps) > MAX_STEPS:
        steps = steps[:MAX_STEPS]
    intent = str(raw.get("intent") or "").strip()
    return {"intent": intent, "steps": steps}


def execute_plan(img: Image.Image, plan: Dict[str, Any], providers: Any, cancel=None) -> Image.Image:
    """按计划逐步执行；每步前检查取消。返回终图。"""
    cur = img
    for st in plan.get("steps") or []:
        check_cancel(cancel)
        fn = ops.OPS.get(st.get("op"))
        if fn is None:  # 理论到不了（validate 已拦），防御性跳过
            continue
        cur = fn(cur, st.get("params") or {}, providers)
    return cur


def verify_result(img: Image.Image, intent: str, providers: Any, extra: str = "") -> Dict[str, Any]:
    """多模态复核；返回 `{pass: bool, issues: list[str]}`。

    复核不可用（未配置 vision / 调用异常 / 输出无法解析）时一律**视为通过**，
    以免复核环节把「能出图」变成「出不了图」。
    """
    vp = getattr(providers, "vision", None)
    if not vp or not getattr(vp, "baseUrl", "") or not getattr(vp, "model", ""):
        return {"pass": True, "issues": []}
    prompt = presets.verifier_prompt(intent, extra)
    try:
        text = oc.chat_vision(vp.baseUrl, vp.apiKey, vp.model, [png_bytes(img)], prompt)
    except Exception:
        return {"pass": True, "issues": []}
    data = parse_json_loose(text)
    if not isinstance(data, dict) or "pass" not in data:
        return {"pass": True, "issues": []}
    passed = bool(data.get("pass"))
    issues = [str(x).strip() for x in (data.get("issues") or []) if str(x).strip()]
    return {"pass": passed, "issues": issues}


__all__ = ["PlanError", "validate_plan", "execute_plan", "verify_result", "ALLOWED_OPS", "MAX_STEPS"]
