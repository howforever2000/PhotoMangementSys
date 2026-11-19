"""智能修图 · 计划校验与执行（FEAT-152 → FEAT-168 清单驱动）

分层职责（CS1）：
  - `validate_plan`  把 planner / 配方返回的宽松 JSON 归一成**受约束计划**：
                      算子白名单来自 `catalog.allowed_ops()`，参数夹取来自
                      `catalog.norm_params()`，顺序靠 `requiresBefore` 显式校验，
                      不再用 `_ORDER` 排序硬编码。非法抛 `PlanError`。
  - `execute_plan`   线性串联算子（后一步吃前一步输出），每步发 `on_step` 事件。
  - `verify_result`  多模态复核；无 provider / 异常 / 解析失败一律返回 `unknown`，
                      **不再 fail-open 当通过**。

本模块不含 provider 协议细节（在 `providers/openai_compat.py`）、不含提示词文本
（在 `presets.py`）、不含作业/进度框架（在 `workflows/*.py`）。
"""
from __future__ import annotations

import time
from typing import Any, Callable, Dict, List

from PIL import Image

from . import catalog, critic, ops
from .providers import openai_compat as oc
from .workflows.base import check_cancel, parse_json_loose, understand_bytes

# 超分同族：云端/本地各一次即可，同族多步会重复放大、反而不如单步
_SUPERRES_OPS = frozenset({"upscale", "local_upscale"})
MAX_STEPS = 6  # 配方比单步计划多（调色→滤镜→形变→磨皮→超分），放宽到 6

StepEvent = Callable[[Dict[str, Any]], None]


class PlanError(ValueError):
    """计划非法（planner 输出不满足白名单/约束）。planner 可据此错误重试一次。"""


def _norm_step(raw: Any) -> Dict[str, Any]:
    """单步归一：白名单 + schema 夹取；保留 optional 等编排字段。"""
    if not isinstance(raw, dict):
        raise PlanError("步骤必须是对象")
    op = str(raw.get("op") or "").strip()
    if op not in catalog.allowed_ops():
        raise PlanError(
            f"不支持的算子: {op or '(空)'}，仅允许 {', '.join(catalog.allowed_ops())}"
        )
    step: Dict[str, Any] = {
        "op": op,
        "params": catalog.norm_params(op, raw.get("params")),
    }
    if raw.get("optional"):
        step["optional"] = True
    return step


def _one_superres(steps: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """超分同族只留第一步（云端/本地不叠加）：两次 ×4 会变成 ×16，体积与伪影都失控。"""
    out: List[Dict[str, Any]] = []
    seen = False
    for step in steps:
        if step["op"] in _SUPERRES_OPS:
            if seen:
                continue
            seen = True
        out.append(step)
    return out


def validate_plan(raw: Dict[str, Any]) -> Dict[str, Any]:
    """宽松 JSON → 受约束计划。非法抛 PlanError。

    顺序不再靠 `sorted(_ORDER[op])` 悄悄改写：前置依赖必须**显式出现在前面**，
    否则报错让 planner 重试。理由：用排序代替依赖无法表达「磨皮必须在形变之后」。
    """
    if not isinstance(raw, dict):
        raise PlanError("计划必须是 JSON 对象")
    steps_raw = raw.get("steps")
    if not isinstance(steps_raw, list) or not steps_raw:
        raise PlanError("计划缺少 steps 或为空")
    steps = [_norm_step(s) for s in steps_raw]
    steps = _one_superres(steps)
    if len(steps) > MAX_STEPS:
        steps = steps[:MAX_STEPS]

    # 依赖是「相对顺序」约束：若两个算子都出现，前置者必须排在前面；
    # 前置者缺席不算非法（单步计划、配方里的 optional 步都要能成立）。
    positions = {st["op"]: i for i, st in enumerate(steps)}
    for i, st in enumerate(steps):
        for dep in catalog.requires_before(st["op"]):
            if dep in positions and positions[dep] > i:
                raise PlanError(f"算子 {st['op']} 必须排在 {dep} 之后，当前顺序相反")

    intent = str(raw.get("intent") or "").strip()
    return {"intent": intent, "steps": steps}


def execute_plan(
    img: Image.Image,
    plan: Dict[str, Any],
    providers: Any,
    cancel=None,
    on_step: StepEvent | None = None,
) -> Image.Image:
    """按计划逐步执行；每步前检查取消、每步发一次结构化事件。返回终图。

    `on_step({index,total,op,label,status,ms,issues})` 由入口层（workflow）转成
    job meta.steps，前端据此显示「第 N/M 步 · 耗时」。本函数不碰作业状态。

    FEAT-169 结构保全：云端步（整幅重绘）执行后用 `critic.structure_compare` 硬验收；
    一旦检出几何被改动，**以该步输入回退**并把原因写进 issues。为什么是回退而不是
    整单失败：用户要的是「不出改形状的图」，而整批失败等于什么都拿不到 ——
    回退既保住形状，也保住其余步骤的成果。
    """
    cur = img
    steps = plan.get("steps") or []
    total = len(steps)
    for i, st in enumerate(steps, start=1):
        check_cancel(cancel)
        op = st.get("op")
        fn = ops.resolve(op)
        label = (catalog.op_spec(op) or {}).get("label", op)
        if fn is None:
            if on_step:
                on_step({"index": i, "total": total, "op": op, "label": label,
                         "status": "skipped", "ms": 0, "issues": ["算子实现缺失"]})
            continue
        t0 = time.perf_counter()
        if on_step:
            on_step({"index": i, "total": total, "op": op, "label": label,
                     "status": "running", "ms": 0})
        before = cur
        try:
            cur = fn(cur, st.get("params") or {}, providers)
            status = "done"
            issues: List[str] = []
            # 只对云端步做结构验收：本机算子已在实现层保证几何恒等（查表 / 局部 ROI），
            # 再跑一遍检测白白多花几百毫秒。
            if op not in ops.LOCAL_OPS:
                check = critic.structure_compare(before, cur)
                if not check["ok"]:
                    cur = before
                    status = "failed"
                    issues = ["结构被改动，已回退该步：" + "；".join(check["issues"])]
        except Exception as e:  # noqa: BLE001 —— 单步失败不拖垮整条链，降级为原样通过
            cur = before
            status = "failed"
            issues = [str(e)]
        ms = int((time.perf_counter() - t0) * 1000)
        if on_step:
            on_step({"index": i, "total": total, "op": op, "label": label,
                     "status": status, "ms": ms, "issues": issues})
    return cur


def verify_result(
    img: Image.Image,
    intent: str,
    providers: Any,
    extra: str = "",
    source: Image.Image | None = None,
) -> Dict[str, Any]:
    """多模态复核；返回 `{verdict, issues}`，verdict ∈ pass|fail|unknown。

    两处旧缺陷已修：
      - 只喂结果图 → 现在结果图在前、原图在后一起喂，判断身份漂移 / 过度磨皮；
      - 无 provider / 异常 / 解析失败一律 `pass` → 现在一律 `unknown`（复核不可用，
        不等于复核通过；调用方据此决定是否继续反思轮）。
    """
    vp = getattr(providers, "vision", None)
    if not vp or not getattr(vp, "baseUrl", "") or not getattr(vp, "model", ""):
        return {"verdict": "unknown", "issues": []}
    from . import presets

    prompt = presets.verifier_prompt(intent, extra)
    # 复核属于「理解」类调用：喂短边 1080 的 JPEG，而不是 24MP 原图的 PNG。
    # 旧行为实测 24MP 每张 `img.save(PNG)` 要 5140ms、上行 50MB；而复核要同时喂
    # 结果图与原图，等于一次付两份。
    images = [understand_bytes(img)] + (
        [understand_bytes(source)] if source is not None else []
    )
    try:
        text = oc.chat_vision(vp.baseUrl, vp.apiKey, vp.model, images, prompt)
    except Exception:
        return {"verdict": "unknown", "issues": []}
    data = parse_json_loose(text)
    if not isinstance(data, dict) or "pass" not in data:
        return {"verdict": "unknown", "issues": []}
    passed = bool(data.get("pass"))
    issues = [str(x).strip() for x in (data.get("issues") or []) if str(x).strip()]
    return {"verdict": "pass" if passed else "fail", "issues": issues}


__all__ = ["PlanError", "validate_plan", "execute_plan", "verify_result", "MAX_STEPS"]
