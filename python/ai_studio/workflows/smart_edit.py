"""智能修图工作流（B 模式：大模型编排 + 反思，FEAT-152 → FEAT-197）

## 链路口径（FEAT-197 定稿）

    原图 → 短边 1080 的 JPEG → 语言模型理解并规划步骤 → 文生图模型逐步执行
         → 1K 出图 →（可选）上采样 2× / 4× → 落盘

「不涉及本地处理」是这条路线现在的**硬约束**：规划只允许云端算子
（`catalog.cloud_ops()`），本机算子（调色 / 胶片 / 形变 / 磨皮 / 本机超分）属于
另一条产品路线「自己组合」。理由有三：本机步不进「按模型计费」的口径；
本机步会在用户机器上跑几十秒，会把「一句话出片」变成长时间等待；
两条路线混排后，用户无法预期自己付了几次费。

## 与 A 模式的关系

共用 `workflows/recipe.py::run_plan` 这一个执行入口：
  1. 规划：多模态模型看「图 + 用户意图」→ 受约束 JSON 计划；
            校验失败回喂重试一次，仍失败**降级为默认配方**（不再是硬编码默认计划）。
  2. 上采样：按用户勾选重写超分步（`upsample.plan_with_upsample`）。
  3. 执行：`recipe.run_plan` → `pipeline.execute_plan`，带结构化 step 事件。
  4. 反思（可选）：`pipeline.verify_result` 三态复核；unknown（无 provider / 异常）
            不冒充通过，只在 fail 时触发 `refine` 微调。
  5. 落盘：只输出终图 JPG + 结构化 meta（plans / verifications / steps）。

CS1：编排只在入口层，算法在 ops / ops_local，参数夹取在 catalog / pipeline。
"""
from __future__ import annotations

from .. import catalog, critic, modelstyle, ops, pipeline, presets, recipecatalog, upsample
from ..providers import openai_compat as oc
from ..renderopts import RunContext, clamp_render_steps
from ..steptrace import StepTrace
from . import recipe as recipe_flow
from .base import (
    check_cancel,
    filter_inputs,
    parse_json_loose,
    read_image,
    save_image,
    stem_of,
    understand_bytes,
)

# 反思轮上限：用户只有一个「反思复核」开关（固定 1 轮），这里仍留 4 的上限，
# 供单步工具与历史请求显式传更高的值；界面不再暴露轮数选择。
_MAX_ROUNDS = 4

# 「模型理解」只编排云端算子（见模块头注）
_PLANNABLE = catalog.cloud_ops()


def _plan_one(
    vp, img, user_prompt: str, warnings, stats, strength: float | None = None, style_id: str = ""
) -> dict:
    """产出受约束计划；失败重试一次，仍失败返回默认配方（保证能出图）。"""
    base_prompt = presets.planner_prompt(user_prompt, strength, style_id)
    prompt = base_prompt
    for attempt in range(2):
        try:
            stats["visionCalls"] += 1
            text = oc.chat_vision(
                vp.baseUrl, vp.apiKey, vp.model, [understand_bytes(img)], prompt
            )
            return pipeline.validate_plan(parse_json_loose(text), allowed=_PLANNABLE)
        except Exception as e:
            if attempt == 0:
                prompt = base_prompt + f"\n\n上一次输出不合法：{e}\n请只输出合法 JSON，不要多余文字。"
            else:
                warnings.append(f"规划失败，已改用默认配方：{e}")
    return recipecatalog.plan_of(recipecatalog.recipes()[0]["id"])


def _apply_upsample(plan: dict, opts: dict, warnings: list) -> dict:
    """把用户的上采样选项落到计划上；非法组合（如与配方默认超分撞车）只告警。"""
    enabled = bool(opts.get("upsample", False))
    scale = opts.get("upsampleScale", 2)
    merged = upsample.plan_with_upsample(plan, enabled, scale)
    try:
        return pipeline.validate_plan(merged, allowed=_PLANNABLE)
    except pipeline.PlanError as e:
        warnings.append(f"上采样步骤未启用：{e}")
        return plan


def _count_calls(plan: dict, stats: dict) -> None:
    """按计划统计调用次数（计费口径，必须与真实调用一致）。"""
    for st in plan.get("steps") or []:
        if st["op"] in ops.LOCAL_OPS:
            stats["localCalls"] += 1
        elif st["op"] == "upscale":
            stats["upscaleCalls"] += 1
        else:
            stats["editCalls"] += 1


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    opts = req.options or {}
    reflect = bool(opts.get("reflect", False))
    try:
        rounds = max(1, min(_MAX_ROUNDS, int(opts.get("rounds", 1) or 1)))
    except Exception:
        rounds = 1
    render_steps = clamp_render_steps(opts.get("steps", opts.get("renderSteps", 1)))
    # 风格提示词（FEAT-198）：中文原文由**前端**追加进可见输入框（用户可编辑），
    # 后端只补英文要点这一段，避免同一段描述进提示词两次（重复描述会推高权重）。
    style_id = str(opts.get("styleId") or opts.get("style") or "").strip()
    style_block = modelstyle.en_block(style_id)
    if style_id and not style_block:
        warnings.append(f"未知的风格预设「{style_id}」，本次不追加风格描述")
    ctx = RunContext(render_steps, style_block)
    # 「强度」是改动幅度偏好，进规划提示词（不传就是死控件，见 presets.planner_prompt）
    try:
        strength = float(opts.get("strength")) if opts.get("strength") is not None else None
    except (TypeError, ValueError):
        strength = None

    providers = req.providers
    vp = getattr(providers, "vision", None)
    ep = getattr(providers, "edit", None)
    if not vp or not getattr(vp, "baseUrl", "") or not getattr(vp, "model", ""):
        raise ValueError("智能修图需要『图像理解』模型（用于规划与复核），请先在「模型设置」配置")
    if not ep or not getattr(ep, "baseUrl", "") or not getattr(ep, "model", ""):
        raise ValueError("智能修图需要『图像编辑』模型（用于执行），请先在「模型设置」配置")

    outputs = []
    plans = []
    verifications = []
    steps_all = []
    stats = {"images": 0, "visionCalls": 0, "editCalls": 0, "upscaleCalls": 0, "localCalls": 0}
    n = len(req.paths)

    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        img = read_image(path)
        stats["images"] += 1
        span = 100.0 / n
        base_pct = i * span

        progress(int(base_pct + span * 0.05), "规划编辑步骤", f"{i + 1}/{n}")
        plan = _plan_one(vp, img, req.prompt or "", warnings, stats, strength, style_id)
        plan = _apply_upsample(plan, opts, warnings)
        plan, unready, skipped = recipecatalog.drop_unready(plan)
        warnings.extend(unready)
        plans.append(plan)
        _count_calls(plan, stats)

        trace = StepTrace()
        for j, op in enumerate(skipped):
            label = (catalog.op_spec(op) or {}).get("label", op)
            trace.skipped(-(j + 1), len(skipped), op, label, "资源未就绪")
        progress(int(base_pct + span * 0.15), "执行编辑", f"{i + 1}/{n}", trace.snapshot())

        def emit(pct: int, stage: str, steps) -> None:
            """把计划内步骤的结构化进度并入总进度（与 recipe 同一形态）。"""
            progress(max(0, min(100, pct)), stage, f"{i + 1}/{n}", steps)

        result = recipe_flow.run_plan(
            img, plan, providers, cancel, trace, emit, base_pct, span, ctx
        )
        steps_all.extend(trace.snapshot())

        # 反思轮：only fail 才修正；unknown 不冒充通过，也不无谓烧钱重绘
        if reflect:
            for r in range(1, rounds + 1):
                check_cancel(cancel)
                progress(int(base_pct + span * (0.30 + 0.55 * (r - 1) / max(1, rounds))),
                         f"复核（第 {r} 版）", f"{i + 1}/{n}", trace.snapshot())
                stats["visionCalls"] += 1
                v = pipeline.verify_result(
                    result, plan.get("intent") or (req.prompt or ""), providers,
                    req.prompt or "", source=img,
                )
                verifications.append(v)
                if v.get("verdict") != "fail":
                    break
                if r >= rounds:
                    warnings.append(
                        f"{stem_of(path)}：复核仍有问题，已达到轮次上限 {rounds}："
                        + "；".join(v.get("issues") or [])
                    )
                    break
                stats["editCalls"] += 1
                result = ops.op_refine(
                    result, {"issues": v.get("issues") or [], "extra": req.prompt or ""},
                    providers, ctx,
                )

        # 本地零模型 critic（默认开，免费）
        c = critic.compare(img, result)
        verifications.append(c)
        if c["issues"]:
            warnings.append(f"{stem_of(path)}：客观指标提示 " + "；".join(c["issues"]))

        progress(int(base_pct + span * 0.95), "保存", f"{i + 1}/{n}", trace.snapshot())
        outputs.append(save_image(result, out_dir, f"{stem_of(path)}_智能修图", ".jpg", 95))

    progress(100, "完成", f"{len(outputs)} 张", steps_all)
    meta = {
        "plans": plans,
        "reflect": reflect,
        "rounds": rounds,
        "renderSteps": render_steps,
        "strength": strength,
        "styleId": style_id,
        "styleChars": len(style_block),
        "verifications": verifications,
        "steps": steps_all,
        "stats": stats,
    }
    meta.update(upsample.upsample_meta(bool(opts.get("upsample", False)), opts.get("upsampleScale", 2)))
    return {"outputs": outputs, "warnings": warnings, "meta": meta}
