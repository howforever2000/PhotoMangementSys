"""智能修图工作流（B 模式：大模型编排 + 反思，FEAT-152 → FEAT-168）

与 A 模式（人工选配方）共用 `workflows/recipe.py::run_plan` 这一个执行入口：
  1) 规划：多模态模型看「图 + 用户意图」→ 受约束 JSON 计划；
           校验失败回喂重试一次，仍失败**降级为默认配方**（不再是硬编码默认计划）。
  2) 执行：`recipe.run_plan` → `pipeline.execute_plan`，带结构化 step 事件。
  3) 反思（可选）：`pipeline.verify_result` 三态复核；unknown（无 provider / 异常）
           不再冒充通过，只在 fail 时触发 `refine` 微调。
  4) 落盘：只输出终图 JPG + 结构化 meta（plans / verifications / steps）。

CS1：编排只在入口层，算法在 ops / ops_local，参数夹取在 catalog / pipeline。
"""
from __future__ import annotations

from .. import catalog, ops, pipeline, presets, recipecatalog
from ..providers import openai_compat as oc
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

_MAX_ROUNDS = 4


def _plan_one(vp, img, user_prompt: str, warnings, stats, local_first: bool = False) -> dict:
    """产出受约束计划；失败重试一次，仍失败返回默认配方（保证能出图）。"""
    base_prompt = presets.planner_prompt(user_prompt, local_first=local_first)
    prompt = base_prompt
    for attempt in range(2):
        try:
            stats["visionCalls"] += 1
            text = oc.chat_vision(
                vp.baseUrl, vp.apiKey, vp.model, [understand_bytes(img)], prompt
            )
            return pipeline.validate_plan(parse_json_loose(text))
        except Exception as e:
            if attempt == 0:
                prompt = base_prompt + f"\n\n上一次输出不合法：{e}\n请只输出合法 JSON，不要多余文字。"
            else:
                warnings.append(f"规划失败，已改用默认配方：{e}")
    return recipecatalog.plan_of(recipecatalog.recipes()[0]["id"])


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
    local_first = bool(opts.get("localFirst", False))
    n = len(req.paths)

    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        img = read_image(path)
        stats["images"] += 1
        span = 100.0 / n
        base_pct = i * span

        progress(int(base_pct + span * 0.05), "规划编辑步骤", f"{i + 1}/{n}")
        plan = _plan_one(vp, img, req.prompt or "", warnings, stats, local_first)
        plan, unready, skipped = recipecatalog.drop_unready(plan)
        warnings.extend(unready)
        plans.append(plan)
        for st in plan.get("steps") or []:
            if st["op"] in ops.LOCAL_OPS:
                stats["localCalls"] += 1
            elif st["op"] == "upscale":
                stats["upscaleCalls"] += 1
            else:
                stats["editCalls"] += 1

        trace = StepTrace()
        for j, op in enumerate(skipped):
            label = (catalog.op_spec(op) or {}).get("label", op)
            trace.skipped(-(j + 1), len(skipped), op, label, "资源未就绪")
        progress(int(base_pct + span * 0.15), "执行编辑", f"{i + 1}/{n}", trace.snapshot())

        def emit(pct: int, stage: str, steps) -> None:
            """把计划内步骤的结构化进度并入总进度（与 recipe 同一形态）。"""
            progress(max(0, min(100, pct)), stage, f"{i + 1}/{n}", steps)

        result = recipe_flow.run_plan(
            img, plan, providers, cancel, trace, emit, base_pct, span
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
                    result, {"issues": v.get("issues") or [], "extra": req.prompt or ""}, providers
                )

        # 本地零模型 critic（默认开，免费）
        from .. import critic
        c = critic.compare(img, result)
        verifications.append(c)
        if c["issues"]:
            warnings.append(f"{stem_of(path)}：客观指标提示 " + "；".join(c["issues"]))

        progress(int(base_pct + span * 0.95), "保存", f"{i + 1}/{n}", trace.snapshot())
        outputs.append(save_image(result, out_dir, f"{stem_of(path)}_智能修图", ".jpg", 95))

    progress(100, "完成", f"{len(outputs)} 张", steps_all)
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "plans": plans,
            "reflect": reflect,
            "rounds": rounds,
            "localFirst": local_first,
            "verifications": verifications,
            "steps": steps_all,
            "stats": stats,
        },
    }
