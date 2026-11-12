"""智能修图工作流（FEAT-152 扩展）

编排（agent 式但受约束，CS1：编排只在本入口层，算子/校验/提示词各自分层）：
  1) 规划：多模态模型看「图 + 用户意图」→ 受约束 JSON 计划（`pipeline.validate_plan`）；
          解析/校验失败会把错误回喂重试一次，仍失败降级为默认调色计划（保证能出图）。
  2) 执行：`pipeline.execute_plan` 线性串联白名单算子。
  3) 反思（可选，用户勾选 + 设轮次 N）：每轮 `pipeline.verify_result` 复核；
          - 通过 → 早停出图；
          - 不通过且未到上限 → `ops.op_refine` 把「修过的图 + 问题点」再喂图像模型修正。
         N 的含义：最多出 N 版（首版=按计划执行；第 2..N 版=对上一版微调）。
  4) 落盘：只输出终图 JPG（不导出计划/对比文件）。

进度与花费：`progress(pct, stage, message)` 逐步上报（前端据此显示「在做哪一步」）；
`meta.stats` 记录各类模型调用次数（visionCalls/editCalls/upscaleCalls），供前端按
本地计费规则换算「花费」。
"""
from __future__ import annotations

from .. import ops, pipeline, presets
from ..providers import openai_compat as oc
from .base import (
    check_cancel,
    parse_json_loose,
    png_bytes,
    read_image,
    save_image,
    stem_of,
)

_MAX_ROUNDS = 4


def _plan_one(vp, img, user_prompt: str, warnings, stats) -> dict:
    """产出受约束计划；失败重试一次，仍失败返回默认计划（不抛，保证作业能出图）。"""
    base_prompt = presets.planner_prompt(user_prompt)
    prompt = base_prompt
    for attempt in range(2):
        try:
            stats["visionCalls"] += 1
            text = oc.chat_vision(vp.baseUrl, vp.apiKey, vp.model, [png_bytes(img)], prompt)
            return pipeline.validate_plan(parse_json_loose(text))
        except Exception as e:
            if attempt == 0:
                prompt = base_prompt + f"\n\n上一次输出不合法：{e}\n请只输出合法 JSON，不要多余文字。"
            else:
                warnings.append(f"规划失败，已改用默认调色计划：{e}")
    return {
        "intent": (user_prompt or "").strip(),
        "steps": [{"op": "style_transfer", "params": {"mode": "autograde", "extra": (user_prompt or "").strip()}}],
    }


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

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
    warnings = []
    plans = []
    verifications = []
    stats = {"images": 0, "visionCalls": 0, "editCalls": 0, "upscaleCalls": 0}
    n = len(req.paths)

    for i, path in enumerate(req.paths):
        check_cancel(cancel)
        img = read_image(path)
        stats["images"] += 1
        span = 100.0 / n
        base_pct = i * span

        # 1) 规划
        progress(int(base_pct + span * 0.05), "规划编辑步骤", f"{i + 1}/{n}")
        plan = _plan_one(vp, img, req.prompt or "", warnings, stats)
        plans.append(plan)
        for st in plan.get("steps") or []:
            if st.get("op") == "upscale":
                stats["upscaleCalls"] += 1
            else:
                stats["editCalls"] += 1

        # 2) 执行
        progress(int(base_pct + span * 0.15), "执行编辑", f"{i + 1}/{n}")
        result = pipeline.execute_plan(img, plan, providers, cancel)

        # 3) 反思（可选）
        edits_done = 1
        if reflect:
            while edits_done <= rounds:
                check_cancel(cancel)
                progress(
                    int(base_pct + span * (0.30 + 0.55 * (edits_done - 1) / max(1, rounds))),
                    f"复核（第 {edits_done} 版）",
                    f"{i + 1}/{n}",
                )
                stats["visionCalls"] += 1
                v = pipeline.verify_result(
                    result, plan.get("intent") or (req.prompt or ""), providers, req.prompt or ""
                )
                verifications.append(v)
                if v.get("pass"):
                    break
                if edits_done >= rounds:
                    warnings.append(
                        f"{stem_of(path)}：复核仍有问题，已达到轮次上限 {rounds}："
                        + "；".join(v.get("issues") or [])
                    )
                    break
                progress(
                    int(base_pct + span * (0.55 + 0.4 * edits_done / max(1, rounds))),
                    f"修正（第 {edits_done + 1} 版）",
                    f"{i + 1}/{n}",
                )
                stats["editCalls"] += 1
                result = ops.op_refine(
                    result,
                    {"issues": v.get("issues") or [], "extra": req.prompt or ""},
                    providers,
                )
                edits_done += 1

        # 4) 落盘（只输出终图）
        progress(int(base_pct + span * 0.95), "保存", f"{i + 1}/{n}")
        out = save_image(result, out_dir, f"{stem_of(path)}_智能修图", ".jpg", quality=95)
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "plans": plans,
            "reflect": reflect,
            "rounds": rounds,
            "verifications": verifications,
            "stats": stats,
        },
    }
