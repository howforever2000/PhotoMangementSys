"""一键出片工作流（FEAT-168 入口层编排）

A 模式（人工选配方）与 B 模式（大模型规划）**共用这一个执行入口**：
  - `options.recipeId` → `recipecatalog.plan_of(...)`
  - `options.plan`     → `recipecatalog.plan_from_llm(...)`；非法自动回落默认配方
  - 两者都经 `drop_unready` → `run_plan`（唯一执行函数）→ 落盘

不在本文件里放任何图像处理算法（在 `ops` / `ops_local`），也不放参数夹取
（在 `catalog` / `pipeline.validate_plan`）。这是 CS1 的「编排只在明确入口层」。

降级纪律：缺资源只摘该步 + warning；单步算子失败由 `execute_plan` 降级为原样
通过并记 failed；整批绝不因可选步骤失败。
"""
from __future__ import annotations

from typing import Any, Dict, List

from PIL import Image

from .. import catalog, critic, pipeline, recipecatalog
from ..steptrace import StepTrace
from .base import check_cancel, filter_inputs, read_image, save_image, stem_of


def _on_step_factory(trace: StepTrace, progress, base_pct: float, span: float):
    """每步事件 → 收集 + 顺带把自由文本 stage 更新成「正在做第 N 步」。"""
    def on_step(event: Dict[str, Any]) -> None:
        trace.on_step(event)
        if event["status"] == "running":
            progress(
                int(base_pct + span * (0.15 + 0.7 * event["index"] / max(1, event["total"]))),
                f"第 {event['index']}/{event['total']} 步 · {event['label']}",
                "",
                trace.snapshot(),
            )
    return on_step


def run_plan(
    img: Image.Image,
    plan: Dict[str, Any],
    providers: Any,
    cancel,
    trace: StepTrace,
    progress,
    base_pct: float,
    span: float,
) -> Image.Image:
    """唯一执行函数：A 配方 / B 大模型计划都走这里。"""
    return pipeline.execute_plan(
        img, plan, providers, cancel,
        on_step=_on_step_factory(trace, progress, base_pct, span),
    )


def _critic(source: Image.Image, result: Image.Image) -> Dict[str, Any]:
    """本地零模型客观指标复核：免费、离线、每批只跑一次（不按轮次计费）。"""
    try:
        return critic.compare(source, result)
    except Exception:  # noqa: BLE001 —— critic 失败绝不影响出图
        return {"verdict": "unknown", "score": 0, "dims": {}, "issues": []}


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")
    warnings: List[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    opts = req.options or {}
    plans: List[Dict[str, Any]] = []
    verifications: List[Dict[str, Any]] = []
    steps_all: List[Dict[str, Any]] = []
    n = len(paths)

    # 计划来源：B 模式给 plan，A 模式给 recipeId（可带 paramsOverride 样片微调）；
    # 都不给 → 默认第一条配方。overrides 与 plan 同给时 overrides 只作用于配方分支，
    # 显式 plan 优先（LLM 计划有自己的参数，不该被配方级覆盖悄悄改写）。
    raw_plan = opts.get("plan")
    recipe_id = str(opts.get("recipeId") or recipecatalog.recipes()[0]["id"])
    overrides = opts.get("paramsOverride")
    if isinstance(raw_plan, dict) and raw_plan.get("steps"):
        try:
            plan = recipecatalog.plan_from_llm(raw_plan)
        except pipeline.PlanError as e:
            warnings.append(f"AI 计划不合法，已改用默认配方：{e}")
            plan = recipecatalog.apply_overrides(recipe_id, overrides)
    else:
        plan = recipecatalog.apply_overrides(recipe_id, overrides)

    plan, unready, skipped_ops = recipecatalog.drop_unready(plan)
    warnings.extend(unready)
    plans.append(plan)

    outputs: List[str] = []
    stats = {"images": 0, "steps": 0, "localCalls": 0, "cloudCalls": 0}
    for i, path in enumerate(paths):
        check_cancel(cancel)
        span = 100.0 / n
        base_pct = i * span
        img = read_image(path)
        stats["images"] += 1

        trace = StepTrace()
        # 被摘掉的步补 skipped 事件：用负 index，既不与执行步（1..N）撞号，
        # snapshot 排序后又自然排在最前，前端步骤卡不缺行。
        for j, op in enumerate(skipped_ops):
            trace.skipped(-(j + 1), len(skipped_ops), op,
                          (catalog.op_spec(op) or {}).get("label", op), "资源未就绪")

        progress(int(base_pct + span * 0.05), "准备", f"{i + 1}/{n}", trace.snapshot())
        result = run_plan(img, plan, req.providers, cancel, trace, progress, base_pct, span)
        steps_all.extend(trace.snapshot())
        stats["steps"] += len(trace.snapshot())
        for st in plan.get("steps") or []:
            if st["op"] in catalog.local_ops():
                stats["localCalls"] += 1
            else:
                stats["cloudCalls"] += 1

        # 本地零模型 critic：默认开（Q4）；不调用任何外部模型
        verification = _critic(img, result)
        verifications.append(verification)
        if verification["issues"]:
            warnings.append(f"{stem_of(path)}：客观指标提示 " + "；".join(verification["issues"]))

        progress(int(base_pct + span * 0.95), "保存", f"{i + 1}/{n}", trace.snapshot())
        outputs.append(save_image(result, out_dir, f"{stem_of(path)}_出片", ".jpg", 95))

    progress(100, "完成", f"{len(outputs)} 张", steps_all)
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "recipeId": recipe_id,
            "plans": plans,
            "verifications": verifications,
            "steps": steps_all,
            "stats": stats,
        },
    }
