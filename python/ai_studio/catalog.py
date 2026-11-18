"""算子 / 工具清单（唯一真相源 `op_catalog.json`）的载入、校验与查询。

本模块只做「读一份声明并给出纯函数」，不含任何图像处理、不含作业编排：
  - `ops()` / `tools()`        → 声明对象（带缓存，不重复读盘）
  - `validate()`               → 断链 / 重复 id / 缺省越界 / 依赖成环 全部拦在启动时
  - `op_spec()` / `tool_spec()`→ 按 id 取一条
  - `norm_params()`            → 按 params schema 夹取（取代 pipeline 里逐 op 手写夹取）
  - `allowed_ops()`            → 可由 planner / 配方编排的算子 id（排除 internal）
  - `requires_before()`        → 显式前置依赖，取代硬编码 `_ORDER` 排序
  - `python_modules()`         → 打包 hiddenimports 由清单生成，不再手抄
  - `op_catalog_prompt()`      → 规划提示词里的算子表由清单生成

Python 与前端、Rust、spec 共用同一份 JSON：新增工具只改那一处。
"""
from __future__ import annotations

import json
import os
import sys
from functools import lru_cache
from typing import Any, Dict, List

_CATALOG_FILE = "op_catalog.json"


def _catalog_path() -> str:
    """开发态 = 包目录；PyInstaller 单文件模式 = _MEIPASS（与 config._bundled_dir 同口径）。"""
    here = os.path.dirname(os.path.abspath(__file__))
    candidates = [
        os.path.join(here, _CATALOG_FILE),
        os.path.join(getattr(sys, "_MEIPASS", ""), "ai_studio", _CATALOG_FILE),
    ]
    for p in candidates:
        if os.path.isfile(p):
            return p
    return candidates[0]


@lru_cache(maxsize=1)
def _load() -> Dict[str, Any]:
    with open(_catalog_path(), "r", encoding="utf-8") as fh:
        data = json.load(fh)
    validate(data)
    return data


def ops() -> List[Dict[str, Any]]:
    return _load()["ops"]


def tools() -> List[Dict[str, Any]]:
    return _load()["tools"]


def op_spec(op_id: str) -> Dict[str, Any] | None:
    return next((o for o in ops() if o["id"] == op_id), None)


def tool_spec(tool_id: str) -> Dict[str, Any] | None:
    return next((t for t in tools() if t["id"] == tool_id), None)


def allowed_ops() -> tuple[str, ...]:
    """可编排算子：排除「仅反思循环内部调用」的 internal 算子。"""
    return tuple(o["id"] for o in ops() if not o.get("internal"))


def local_ops() -> frozenset[str]:
    return frozenset(o["id"] for o in ops() if o.get("backend") == "local")


def groups() -> List[tuple[str, str]]:
    """单步工具的展示分组（id, 中文名），按 `groups` 声明顺序。

    分组是**展示口径**，不改变工具集合、不改变后端：加组只改清单。
    """
    return [(g["id"], g["label"]) for g in (_load().get("groups") or [])]


def group_ids() -> tuple[str, ...]:
    return tuple(gid for gid, _ in groups())


def requires_before(op_id: str) -> List[str]:
    spec = op_spec(op_id)
    return list(spec.get("requiresBefore") or []) if spec else []


def stage_of(op_id: str) -> int:
    spec = op_spec(op_id)
    return int(spec.get("stage", 0)) if spec else 0


def python_modules() -> List[str]:
    """打包 hiddenimports：ops 与 tools 的 module 去重（保持声明顺序）。"""
    out: List[str] = []
    for item in [*ops(), *tools()]:
        m = item.get("module")
        if m and m.startswith("ai_studio.") and m not in out:
            out.append(m)
    return out


def op_catalog_prompt() -> str:
    """规划提示词里的算子表：由清单生成，避免与可用算子漂移。"""
    lines = [
        "可用算子（线性执行，后一步吃前一步输出；按顺序依赖，只有确实需要组合时才多步）："
    ]
    for o in ops():
        if o.get("internal"):
            continue
        parts = []
        for key, p in (o.get("params") or {}).items():
            if p["type"] == "enum":
                parts.append(f"{key}: {'|'.join(str(v) for v in p['values'])}")
            elif p["type"] == "range":
                parts.append(f"{key}: {p['min']}..{p['max']}")
            else:
                parts.append(f"{key}?: string")
        dep = o.get("requiresBefore") or []
        tail = f"；须在 {'/'.join(dep)} 之后" if dep else ""
        lines.append(f"- {o['id']}：{o['label']}（{o.get('backend')}）。params: {{{', '.join(parts)}}}{tail}")
    return "\n".join(lines)


# ---------------- 参数归一（schema 驱动，取代手写 if/elif 夹取） ----------------


def _norm_one(raw: Any, spec: Dict[str, Any]) -> Any:
    kind = spec["type"]
    if kind == "enum":
        values = spec["values"]
        return raw if raw in values else spec["default"]
    if kind == "range":
        try:
            v = float(raw)
        except (TypeError, ValueError):
            return spec["default"]
        return max(float(spec["min"]), min(float(spec["max"]), v))
    if kind == "string":
        return str(raw or "")[:500]
    if kind == "bool":
        return bool(raw)
    return spec.get("default")


def norm_params(op_id: str, raw: Any) -> Dict[str, Any]:
    """按清单把任意来源（LLM / 配方 / 前端）的参数夹到合法区间并补默认值。"""
    spec = op_spec(op_id)
    if spec is None:
        return {}
    raw = raw if isinstance(raw, dict) else {}
    schemas = spec.get("params") or {}
    return {key: _norm_one(raw.get(key), sch) for key, sch in schemas.items()}


# ---------------- 启动期校验（断链 / 重复 / 越界 / 成环一律不许上线） ----------------


def validate(data: Dict[str, Any]) -> None:
    """清单自检。抛 ValueError；由 `_load()` 在首次访问时调用。"""
    if not isinstance(data, dict) or not isinstance(data.get("ops"), list):
        raise ValueError("op_catalog.json 缺少 ops 数组")
    if not isinstance(data.get("tools"), list):
        raise ValueError("op_catalog.json 缺少 tools 数组")

    op_ids = [o.get("id") for o in data["ops"]]
    tool_ids = [t.get("id") for t in data["tools"]]
    for label, ids in (("ops", op_ids), ("tools", tool_ids)):
        if any(not isinstance(x, str) or not x for x in ids):
            raise ValueError(f"{label} 存在空 id")
        if len(ids) != len(set(ids)):
            dup = next(x for x in ids if ids.count(x) > 1)
            raise ValueError(f"{label} 存在重复 id: {dup}")

    gids = [g.get("id") for g in (data.get("groups") or [])]
    if any(not isinstance(x, str) or not x for x in gids):
        raise ValueError("groups 存在空 id")
    if len(gids) != len(set(gids)):
        raise ValueError("groups 存在重复 id")

    op_set = set(op_ids)
    for o in data["ops"]:
        for dep in o.get("requiresBefore") or []:
            if dep not in op_set:
                raise ValueError(f"算子 {o['id']} 的 requiresBefore 指向不存在的 {dep}")
            if dep == o["id"]:
                raise ValueError(f"算子 {o['id']} 依赖自身")
        for key, p in (o.get("params") or {}).items():
            if p.get("type") not in ("enum", "range", "string", "bool"):
                raise ValueError(f"算子 {o['id']} 的参数 {key} 类型未知")
            if "default" not in p:
                raise ValueError(f"算子 {o['id']} 的参数 {key} 缺 default")
            if p["type"] == "enum" and p["default"] not in p["values"]:
                raise ValueError(f"算子 {o['id']} 的参数 {key} 默认值不在枚举内")
            if p["type"] == "range" and not (p["min"] <= p["default"] <= p["max"]):
                raise ValueError(f"算子 {o['id']} 的参数 {key} 默认值越界")
        if not str(o.get("module", "")).startswith("ai_studio."):
            raise ValueError(f"算子 {o['id']} 缺合法 module")

    # 依赖必须指向更早的 stage，否则「先瘦脸后磨皮」这类纪律无法靠数值表达
    for o in data["ops"]:
        for dep in o.get("requiresBefore") or []:
            dep_stage = next(int(x.get("stage", 0)) for x in data["ops"] if x["id"] == dep)
            if dep_stage >= int(o.get("stage", 0)):
                raise ValueError(f"算子 {o['id']} 的前置 {dep} 必须 stage 更小")

    valid_policies = {"none", "vision+edit", "edit", "upscale|edit"}
    # 注意：这里只能读传进来的 `data`，不能调 `group_ids()` —— 那会经 `_load()`
    # 再次进入 validate，形成无限递归（启动期直接 RecursionError）。
    valid_groups = {g.get("id") for g in (data.get("groups") or [])}
    if not valid_groups:
        raise ValueError("op_catalog.json 缺少 groups 声明（单步工具分组口径）")
    # 主入口不是「单步工具分组」的一员，但它同样要声明分组字段（供前端统一读取），
    # 于是约定 primary surface 一律取 group="primary"，不参与左栏分组渲染。
    valid_groups.add("primary")
    for t in data["tools"]:
        if t.get("surface") not in ("primary", "single", "planned"):
            raise ValueError(f"工具 {t['id']} 的 surface 非法")
        if t.get("providerPolicy") not in valid_policies:
            raise ValueError(f"工具 {t['id']} 的 providerPolicy 非法")
        # 分组是纯展示口径，但漏声明会让左栏静默丢条目，所以这里一并拦死
        if t.get("group") not in valid_groups:
            raise ValueError(f"工具 {t['id']} 的 group 非法（应为 {sorted(valid_groups)}）")
        if t.get("runtime") == "python" and not str(t.get("module", "")).startswith(
            "ai_studio.workflows."
        ):
            raise ValueError(f"工具 {t['id']} 缺合法 workflows module")
        op = t.get("op")
        if op and op not in op_set:
            raise ValueError(f"工具 {t['id']} 引用了不存在的算子 {op}")


__all__ = [
    "ops",
    "tools",
    "op_spec",
    "tool_spec",
    "allowed_ops",
    "local_ops",
    "groups",
    "group_ids",
    "requires_before",
    "stage_of",
    "python_modules",
    "op_catalog_prompt",
    "norm_params",
    "validate",
]
