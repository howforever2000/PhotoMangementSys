"""配方清单载入与校验（`recipes.json`）。

与 `catalog.py` 分开：算子清单管「有哪些原子能力」，配方清单管「怎么串」，
两者都只是数据，运行时不落盘、不进任何注册表。
"""
from __future__ import annotations

import json
import os
import sys
from functools import lru_cache
from typing import Any, Dict, List

from . import catalog, pipeline

_FILE = "recipes.json"


def _path() -> str:
    here = os.path.dirname(os.path.abspath(__file__))
    candidates = [
        os.path.join(here, _FILE),
        os.path.join(getattr(sys, "_MEIPASS", ""), "ai_studio", _FILE),
    ]
    for p in candidates:
        if os.path.isfile(p):
            return p
    return candidates[0]


@lru_cache(maxsize=1)
def _load() -> Dict[str, Any]:
    with open(_path(), "r", encoding="utf-8") as fh:
        data = json.load(fh)
    recipes = data.get("recipes")
    if not isinstance(recipes, list) or not recipes:
        raise ValueError("recipes.json 缺少 recipes 数组")
    ids = [r.get("id") for r in recipes]
    if any(not isinstance(x, str) or not x for x in ids):
        raise ValueError("配方存在空 id")
    if len(ids) != len(set(ids)):
        raise ValueError("配方 id 重复")
    for r in recipes:
        # 借用同一个校验器：配方与 LLM 计划受完全相同的约束
        pipeline.validate_plan({"steps": r.get("steps")})
    return data


def recipes() -> List[Dict[str, Any]]:
    """给前端的配方列表（不含执行细节）。"""
    return [
        {"id": r["id"], "label": r["label"], "desc": r["desc"],
         "stepCount": len(r["steps"])}
        for r in _load()["recipes"]
    ]


def recipe(recipe_id: str) -> Dict[str, Any] | None:
    return next((r for r in _load()["recipes"] if r["id"] == recipe_id), None)


def plan_of(recipe_id: str) -> Dict[str, Any]:
    """配方 → 受约束计划。找不到时回落到第一条（懒人场景绝不空手而归）。"""
    r = recipe(recipe_id) or _load()["recipes"][0]
    return pipeline.validate_plan({"intent": r["label"], "steps": r["steps"]})


def plan_from_llm(raw: Dict[str, Any]) -> Dict[str, Any]:
    """大模型产出的同形状 JSON → 受约束计划；非法抛 PlanError，由调用方回落配方。"""
    return pipeline.validate_plan(raw)


# 样片微调（workshop-positioning.md 修订记录）：允许被前端覆盖的配方级参数白名单。
# 只有这些键能进计划——曲线/蒙版/画笔等精修参数根本不在这张表里，想传也传不进。
TUNABLE_PARAMS: Dict[str, frozenset] = {
    "auto_grade": frozenset({"style", "strength"}),
    "local_film": frozenset(
        {"lut", "engine", "strength", "look", "grade", "grade_strength",
         "anchor", "grain", "vignette"}
    ),
    "color_transfer": frozenset(
        {"method", "strength", "l_weight", "tone_weight", "chroma_weight",
         "keep_exposure", "protect_skin", "reference_path"}
    ),
    "face_reshape": frozenset({"slim", "eyes", "eye_bright"}),
    "skin_smooth": frozenset({"strength", "texture"}),
    # 交互简化（2026-11-26）：界面只暴露「黝黑↔白皙 + 红嫩」两个滑条；hue_shift/chroma
    # 不再进 catalog/白名单，由 SkinParams.from_raw 在单滑条契约下按 brighten 联动推导
    # （白皙侧自然白、黝黑侧晒黑暖调）。显式四键的旧存档由 from_raw 双契约兼容。
    "skin_tone": frozenset({"brighten", "blush"}),
    "local_vibrance": frozenset({"vibrance"}),
}


def apply_overrides(recipe_id: str, overrides: Dict[str, Any] | None) -> Dict[str, Any]:
    """配方计划 + 样片微调参数 → 覆盖后的计划。

    形状：`{ op: { param: value } }`，如 `{"face_reshape": {"slim": 0.5}}`。
    只覆盖白名单内的键；未知算子/参数静默忽略（前端与后端的清单可能版本错位，
    忽略比报错更符合「懒人绝不空手而归」）。覆盖后仍走 validate_plan 全量校验，
    参数夹取、依赖顺序与配方直跑完全同一套约束。
    """
    plan = plan_of(recipe_id)
    if not isinstance(overrides, dict) or not overrides:
        return plan
    steps = []
    for st in plan.get("steps") or []:
        op = st["op"]
        allow = TUNABLE_PARAMS.get(op)
        patch = overrides.get(op)
        if allow and isinstance(patch, dict):
            params = {**st["params"], **{k: v for k, v in patch.items() if k in allow}}
            steps.append({**st, "params": params})
        else:
            steps.append(st)
    return pipeline.validate_plan({"intent": plan.get("intent"), "steps": steps})


def drop_unready(plan: Dict[str, Any]) -> tuple[Dict[str, Any], List[str], List[str]]:
    """摘掉资源未就绪的本地步，返回 `(计划, warnings, 被摘的 op 列表)`。

    纪律：缺资源只摘该步 + warning，绝不整单失败（`knowledge/local-enhance.md` §6）。
    返回被摘 op 列表是为了让入口层补 `skipped` 步骤事件，前端步骤卡不缺行。
    """
    from . import ops

    kept, warnings, skipped = [], [], []
    for st in plan.get("steps") or []:
        op = st["op"]
        if op in catalog.local_ops() and not ops.local_op_ready(op):
            label = (catalog.op_spec(op) or {}).get("label", op)
            warnings.append(f"本地资源未就绪，已跳过「{label}」；可在「模型设置 › 本地模型」里下载")
            skipped.append(op)
            continue
        kept.append(st)
    return {**plan, "steps": kept}, warnings, skipped


__all__ = [
    "recipes",
    "recipe",
    "plan_of",
    "plan_from_llm",
    "apply_overrides",
    "TUNABLE_PARAMS",
    "drop_unready",
]
