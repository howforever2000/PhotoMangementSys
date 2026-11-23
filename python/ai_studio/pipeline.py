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

## 重复算子（FEAT-197）

链条里允许**同一算子出现多次**（创意组合支持「同一组件多次插入」）。三条相关纪律：
  - 步骤的**位置**不再由 op 唯一，依赖校验按「是否存在更早的同名步」判断；
  - 超分同族**不再静默丢弃**第二个（旧 `_one_superres` 会把 4×=2×跑两遍的设计
    悄悄吃掉，用户看到「选了两次只放大一次」）；改为超过 2 次直接报错，
    因为 3 次以上必定是误操作；
  - 步数上限 `MAX_STEPS` 从静默截断改为**报错**：截断会让用户精心排的后几步消失，
    而界面上明明还显示着。
"""
from __future__ import annotations

import time
from dataclasses import replace
from typing import Any, Callable, Dict, FrozenSet, List

from PIL import Image

from . import catalog, critic, ops
from .logutil import log
from .providers import openai_compat as oc
from .renderopts import EMPTY as EMPTY_CTX, RunContext
from .workflows.base import check_cancel, parse_json_loose, understand_bytes

# 超分同族：云端/本地各算一次有效放大。4× 由「2× 跑两遍」实现，所以允许 2 次；
# 3 次以上是误操作（×8 起步，体积与伪影都失控），在校验期直接拒。
_SUPERRES_OPS = frozenset({"upscale", "local_upscale"})
_MAX_SUPERRES = 2
MAX_STEPS = 10  # 创意组合的上限（FEAT-197：与前端 stepper 上限同一口径）

StepEvent = Callable[[Dict[str, Any]], None]


class PlanError(ValueError):
    """计划非法（planner 输出不满足白名单/约束）。planner 可据此错误重试一次。"""


def _norm_step(raw: Any, allowed: tuple[str, ...]) -> Dict[str, Any]:
    """单步归一：白名单 + schema 夹取；保留 optional 等编排字段。"""
    if not isinstance(raw, dict):
        raise PlanError("步骤必须是对象")
    op = str(raw.get("op") or "").strip()
    if op not in allowed:
        raise PlanError(f"不支持的算子: {op or '(空)'}，仅允许 {', '.join(allowed)}")
    step: Dict[str, Any] = {
        "op": op,
        "params": catalog.norm_params(op, raw.get("params")),
    }
    if raw.get("optional"):
        step["optional"] = True
    return step


def _check_superres(steps: List[Dict[str, Any]]) -> None:
    """超分同族最多 2 次（4× = 2× 跑两遍）；3 次以上报错而不是静默丢弃。"""
    n = sum(1 for s in steps if s["op"] in _SUPERRES_OPS)
    if n > _MAX_SUPERRES:
        raise PlanError(
            f"超分最多 2 步（4× 用 2× 跑两遍），当前 {n} 步；再多就是误操作"
        )


def validate_plan(raw: Dict[str, Any], allowed: FrozenSet[str] | None = None) -> Dict[str, Any]:
    """宽松 JSON → 受约束计划。非法抛 PlanError。

    顺序不再靠 `sorted(_ORDER[op])` 悄悄改写：前置依赖必须**显式出现在前面**，
    否则报错让 planner 重试。理由：用排序代替依赖无法表达「磨皮必须在形变之后」。

    `allowed` 给出时只接受该算子集合（「模型理解」路线只允许云端算子，
    见 `catalog.cloud_ops()`）；缺省用 `catalog.allowed_ops()`。
    """
    if not isinstance(raw, dict):
        raise PlanError("计划必须是 JSON 对象")
    steps_raw = raw.get("steps")
    if not isinstance(steps_raw, list) or not steps_raw:
        raise PlanError("计划缺少 steps 或为空")
    allow = tuple(sorted(allowed)) if allowed is not None else catalog.allowed_ops()

    steps = [_norm_step(s, allow) for s in steps_raw]
    _check_superres(steps)
    if len(steps) > MAX_STEPS:
        raise PlanError(
            f"步骤最多 {MAX_STEPS} 步，当前 {len(steps)} 步；请删减后再提交"
        )

    # 依赖是「相对顺序」约束：若两个算子都出现，前置者必须排在**每一次**同名步之前；
    # 前置者缺席不算非法（单步计划、配方里的 optional 步都要能成立）。
    # 同一算子可重复出现，所以用「每个算子的首次出现位置」判断：只要某个前置算子
    # 全部出现在本步之后，就是逆序。
    first_pos: Dict[str, int] = {}
    for i, st in enumerate(steps):
        first_pos.setdefault(st["op"], i)
    for i, st in enumerate(steps):
        for dep in catalog.requires_before(st["op"]):
            j = first_pos.get(dep)
            if j is not None and j > i:
                raise PlanError(f"算子 {st['op']} 必须排在 {dep} 之后，当前顺序相反")

    intent = str(raw.get("intent") or "").strip()
    return {"intent": intent, "steps": steps}


def execute_plan(
    img: Image.Image,
    plan: Dict[str, Any],
    providers: Any,
    cancel=None,
    on_step: StepEvent | None = None,
    ctx: RunContext | None = None,
    log_tag: str | None = "",
) -> Image.Image:
    """按计划逐步执行；每步前检查取消、每步发一次结构化事件。返回终图。

    `on_step({index,total,op,label,status,ms,issues})` 由入口层（workflow）转成
    job meta.steps，前端据此显示「第 N/M 步 · 耗时」。本函数不碰作业状态。

    `ctx` 是作业级渲染参数（文生图采样步数等），原样交给每个算子；本机算子收下
    但不使用。缺省等于「不向网关声明步数」。

    `log_tag` 控制每步组件日志（「这次作业用了哪些组件」的排障主线索）：
      - `""`（缺省）：打日志，无前缀标签；
      - 非空字符串：打日志并带上照片/来源标签（如 `photo=2/5:IMG_1234`），
        并行下多张的步骤行可按标签区分；
      - `None`：不打日志 —— 同步预览（preview.py）每次拖滑杆都调一次本函数，
        打日志就是日志洪水，必须显式关掉。

    FEAT-169 结构保全：云端步（整幅重绘）执行后用 `critic.structure_compare` 硬验收；
    一旦检出几何被改动，**以该步输入回退**并把原因写进 issues。为什么是回退而不是
    整单失败：用户要的是「不出改形状的图」，而整批失败等于什么都拿不到 ——
    回退既保住形状，也保住其余步骤的成果。
    """
    cur = img
    # 照片级上下文派生（FEAT-208）：作业级字段（render_steps/style_prompt）原样继承，
    # source = 本张原图（肤色保真的锚点）、scratch = 本张的跨步共享槽（磨皮的人脸
    # 分析结果供肤色复用）。每次调用新建 dict/Image 引用，照片级并行下互不可见。
    context = replace(ctx or EMPTY_CTX, source=img, scratch={})
    steps = plan.get("steps") or []
    total = len(steps)
    prefix = f"{log_tag} " if log_tag else ""
    for i, st in enumerate(steps, start=1):
        check_cancel(cancel)
        op = st.get("op")
        fn = ops.resolve(op)
        label = (catalog.op_spec(op) or {}).get("label", op)
        if fn is None:
            if on_step:
                on_step({"index": i, "total": total, "op": op, "label": label,
                         "status": "skipped", "ms": 0, "issues": ["算子实现缺失"]})
            if log_tag is not None:
                log(f"step {i}/{total} {prefix}op={op}（{label}） skipped | 算子实现缺失")
            continue
        t0 = time.perf_counter()
        if on_step:
            on_step({"index": i, "total": total, "op": op, "label": label,
                     "status": "running", "ms": 0})
        before = cur
        try:
            cur = fn(cur, st.get("params") or {}, providers, context)
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
        if log_tag is not None:
            tail = f" | {'；'.join(issues)}" if issues else ""
            log(f"step {i}/{total} {prefix}op={op}（{label}） {status} | {ms}ms{tail}")
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
