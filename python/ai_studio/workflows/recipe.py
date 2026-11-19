"""一键出片工作流（FEAT-168 入口层编排）

A 模式（人工选配方）与 B 模式（大模型规划）**共用这一个执行入口**：
  - `options.recipeId` → `recipecatalog.plan_of(...)`
  - `options.plan`     → `recipecatalog.plan_from_llm(...)`；非法自动回落默认配方
  - 两者都经 `drop_unready` → `run_plan`（唯一执行函数）→ 落盘

不在本文件里放任何图像处理算法（在 `ops` / `ops_local`），也不放参数夹取
（在 `catalog` / `pipeline.validate_plan`）。这是 CS1 的「编排只在明确入口层」。

降级纪律：缺资源只摘该步 + warning；单步算子失败由 `execute_plan` 降级为原样
通过并记 failed；整批绝不因可选步骤失败。

并行：照片级。**每条链路的步骤事件（StepTrace）必须按照片隔离**——`StepTrace`
是可变对象，多线程共用一个会让 `meta.steps` 里出现互相覆盖的 index。
所以 trace 在 `process` 内新建，收尾时按照片顺序拼接。
plan 只解析一次（在循环外），与旧版逐张重算是等价的（同一份 options）。
"""
from __future__ import annotations

from typing import Any, Dict, List

from PIL import Image

from .. import catalog, critic, pipeline, recipecatalog
from ..steptrace import StepTrace
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

_ONNX_HEAVY = True


def _on_step_factory(trace: StepTrace, base_pct: float, span: float, emit):
    """每步事件 → 收集 + 顺带把 stage 更新成「正在做第 N 步」。

    **不直接调用 progress**：并行下多张图会同时上报步骤，stage 文案会来回跳。
    改成把「当前活动的步骤快照」交给 `emit`，由外层按「哪一张在动」决定展示
    （见 `run` 里的 `_progress_thunk`）。
    """
    def on_step(event: Dict[str, Any]) -> None:
        trace.on_step(event)
        if event["status"] == "running":
            emit(
                int(base_pct + span * (0.15 + 0.7 * event["index"] / max(1, event["total"]))),
                f"第 {event['index']}/{event['total']} 步 · {event['label']}",
                trace.snapshot(),
            )
    return on_step


def run_plan(
    img: Image.Image,
    plan: Dict[str, Any],
    providers: Any,
    cancel,
    trace: StepTrace,
    emit,
    base_pct: float,
    span: float,
) -> Image.Image:
    """唯一执行函数：A 配方 / B 大模型计划都走这里。"""
    return pipeline.execute_plan(
        img, plan, providers, cancel,
        on_step=_on_step_factory(trace, base_pct, span, emit),
    )


def _critic(source: Image.Image, result: Image.Image) -> Dict[str, Any]:
    """本地零模型客观指标复核：免费、离线、每批只跑一次（不按轮次计费）。"""
    try:
        return critic.compare(source, result)
    except Exception:  # noqa: BLE001 —— critic 失败绝不影响出图
        return {"verdict": "unknown", "score": 0, "dims": {}, "issues": []}


def _resolve_plan(opts: Dict[str, Any], warnings: List[str]) -> tuple[Dict[str, Any], str, List[str]]:
    """计划来源归一（循环外只做一次）：返回 (计划, recipeId, 被摘步的 op 列表)。

    B 模式给 plan，A 模式给 recipeId（可带 paramsOverride 样片微调）；
    都不给 → 默认第一条配方。overrides 与 plan 同给时 overrides 只作用于配方分支，
    显式 plan 优先（LLM 计划有自己的参数，不该被配方级覆盖悄悄改写）。
    """
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
    return plan, recipe_id, skipped_ops


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")
    warnings: List[str] = []
    paths = filter_inputs(list(req.paths), warnings)

    opts = req.options or {}
    plan, recipe_id, skipped_ops = _resolve_plan(opts, warnings)
    total = len(paths)

    # 并行完成后按序收：steps_all / verifications 必须与 paths 对齐
    steps_by_photo: Dict[int, List[Dict[str, Any]]] = {}
    verify_by_photo: Dict[int, Dict[str, Any]] = {}
    # 并行下多张会同时上报步骤：只让「最近开动的那一张」决定 stage 文案，
    # 否则用户会看到步骤名在不同照片之间来回跳。
    active = {"index": -1, "pct": 3, "stage": "准备", "steps": []}

    def emit_for(photo_index: int, photo_label: str):
        """构造该照片专属的步骤上报回调（把结构化步骤一起并入 progress）。"""
        def emit(pct: int, stage: str, steps: List[Dict[str, Any]]) -> None:
            active.update(index=photo_index, pct=pct, stage=stage, steps=steps)
            progress(
                max(3, min(95, pct)),
                stage,
                photo_label,
                steps,
            )
        return emit

    def process(index: int, path: str) -> Outcome:
        w: List[str] = []
        # 每张的进度区间与旧版一致（第 index 张占总进度的 1/total 段）
        span = 100.0 / total
        base_pct = index * span
        img = read_image(path)
        label = f"{index + 1}/{total}"

        trace = StepTrace()
        # 被摘掉的步补 skipped 事件：用负 index，既不与执行步（1..N）撞号，
        # snapshot 排序后又自然排在最前，前端步骤卡不缺行。
        for j, op in enumerate(skipped_ops):
            trace.skipped(-(j + 1), len(skipped_ops), op,
                          (catalog.op_spec(op) or {}).get("label", op), "资源未就绪")

        emit = emit_for(index, label)
        emit(int(base_pct + span * 0.05), "准备", trace.snapshot())
        result = run_plan(img, plan, req.providers, cancel, trace, emit, base_pct, span)
        steps_by_photo[index] = trace.snapshot()

        # 本地零模型 critic：默认开（Q4）；不调用任何外部模型
        verification = _critic(img, result)
        verify_by_photo[index] = verification
        if verification["issues"]:
            w.append(f"{stem_of(path)}：客观指标提示 " + "；".join(verification["issues"]))

        out = save_image(result, out_dir, f"{stem_of(path)}_出片", ".jpg", 95)
        return Outcome(outputs=[out], warnings=w)

    def on_done(finished: int, total_n: int) -> None:
        """完成计数推进 + 保留活动步骤的结构化信息（步骤卡不断更）。"""
        progress(
            3 + int(92.0 / max(1, total_n) * finished),
            active["stage"] if active["index"] >= 0 else "出片处理中",
            f"{finished}/{total_n}",
            active["steps"],
        )

    res = run_batch(
        paths,
        process,
        progress,
        cancel,
        stage="出片处理中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
        on_done=on_done,
    )

    steps_all: List[Dict[str, Any]] = []
    for i in range(total):
        steps_all.extend(steps_by_photo.get(i) or [])
    verifications = [verify_by_photo.get(i, {"verdict": "unknown", "score": 0, "dims": {}, "issues": []})
                     for i in range(total)]

    stats = {"images": len(res.outputs), "steps": 0, "localCalls": 0, "cloudCalls": 0}
    for st in plan.get("steps") or []:
        per_photo = 1 if st["op"] in catalog.local_ops() else 0
        stats["localCalls" if per_photo else "cloudCalls"] += total
    stats["steps"] = sum(1 for _ in steps_all)

    progress(100, "完成", f"{len(res.outputs)} 张", steps_all)
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {
            "recipeId": recipe_id,
            "plans": [plan],
            "verifications": verifications,
            "steps": steps_all,
            "stats": stats,
        },
    }
