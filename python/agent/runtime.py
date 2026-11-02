"""确定性执行器：按 RunSpec 把配方跑成产物（P0 / P1a / P1c 三条零成本路径）。

和"Agent 循环"的区别（docs/v5.md §10.3、§13）：
- 这里**没有模型**。路径、顺序、参数全部来自 RunSpec，是确定的；
- 规划循环（planner.py）在后续块接入，它负责在**意图模糊/多交付物**时
  决定"调哪几个工具"，而不是重排这里的步骤。

这层的价值就是"点一下就出活"：同一输入同输出、零出网、零成本、可回归测试。
"""
from __future__ import annotations

import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

from . import logutil
from .budget import BudgetExceeded
from .jobs import Job
from .prompt import CompileOutput, finalize_filled
from .recipes.schema import Recipe
from .tools import MAX_IMAGES_PER_JOB, ToolContext, dispatch
from .validators import intensity_level

# 每个工具在轨迹里的中文名（UI 直接显示，不改内部 id）
TOOL_LABEL = {
    "select": "选图",
    "compress": "压缩",
    "style_transfer": "本地调色",
    "beauty_render": "人像润色",
    "card_render": "卡片排版",
    "probe": "产物体检",
}


def _default_out_dir(job_id: int) -> str:
    d = Path(tempfile.gettempdir()) / "pm-agent-out" / f"job{job_id}"
    d.mkdir(parents=True, exist_ok=True)
    return str(d)


def apply_compile_meta(job: Job, compiled: CompileOutput) -> None:
    """把编译结果写进快照（app 与 runtime 共用一处，避免两边各写一份）。"""
    job.p1_path = compiled.p1_path
    job.set_meta(
        prompt=compiled.prompt,
        promptSource=compiled.prompt_source,
        assumptions=list(compiled.assumptions),
        conflicts=list(compiled.conflicts),
        hint=compiled.hint,
        preview=compiled.preview,
    )


@dataclass
class RunEnv:
    """一次 job 的出网环境。由 app.py 从请求体里的 per-job 凭据构造，用完即弃。"""

    provider: object | None = None  # OpenAICompat（图像）
    planner: object | None = None  # Planner
    budget: object | None = None  # BudgetGuard
    size: str = "1024x1024"
    tier: str = "draft"


async def run_job(
    job: Job,
    recipe: Recipe,
    compiled: CompileOutput,
    body: dict,
    env: RunEnv | None = None,
) -> None:
    """跑完一个 job（异常一律收敛成 job 的终态，绝不吊死轮询）。"""
    t0 = time.monotonic()
    apply_compile_meta(job, compiled)
    try:
        await _run(job, recipe, compiled, body, env or RunEnv())
    except BudgetExceeded as e:
        # 预算是“可预期的失败”，不是 bug：状态单列 budget，文案带已花金额
        logutil.err(f"[job {job.job_id}] 预算中止：{e}")
        job.set_meta(budget=e.to_dict())
        job.finish("budget", str(e))
        return
    except Exception as e:  # noqa: BLE001
        logutil.exc(f"[job {job.job_id}] 执行异常", e)
        job.finish("error", f"{type(e).__name__}: {e}")
        return
    logutil.err(
        f"[job {job.job_id}] 完成 status={job.snapshot()['status']} "
        f"steps={len(job.steps)} cost={job.snapshot()['costTotal']:.4f} "
        f"{int((time.monotonic() - t0) * 1000)}ms"
    )


async def _run(
    job: Job, recipe: Recipe, compiled: CompileOutput, body: dict, env: RunEnv
) -> None:
    spec = compiled.run_spec
    paths = [str(p) for p in (body.get("paths") or []) if p]
    out_dir = str(body.get("outputDir") or "") or _default_out_dir(job.job_id)
    ctx = ToolContext(
        out_dir=out_dir,
        thumbs_dir=str(Path(out_dir) / ".thumbs"),
        overwrite=bool(spec.export.overwrite),
        provider=env.provider,
        budget=env.budget,
        size=env.size,
    )
    Path(ctx.thumbs_dir).mkdir(parents=True, exist_ok=True)

    # ① 选图（顺序即用户心里的顺序，不排序）
    sel = await dispatch("select", {"paths": paths, "limit": MAX_IMAGES_PER_JOB}, ctx)
    chosen: list[str] = sel.get("paths", [])
    note = f"手选 {len(chosen)} 张"
    if sel.get("dropped"):
        note += f"（去重/超上限丢弃 {sel['dropped']} 张）"
    job.add_step("select", note=note, latency_ms=0, cost=0.0)
    if not chosen:
        job.finish("error", "没有可处理的照片：请先把照片加入素材托盘")
        return

    produced: list[str] = []
    failures: list[str] = []

    # ①.5 P1b：先让规划模型填槽位（整个 job 里唯一“理解意图”的开销）
    if compiled.plan_pending:
        await _fill_slots(job, recipe, compiled, chosen, env)
        if job.is_cancelled():
            job.finish("cancelled", "已取消")
            return

    # ② 按交付物分支（全是确定路径）
    if spec.style and spec.style.engine == "remote":
        if not compiled.prompt:
            job.finish("error", "远程重绘缺少目标提示词（槽位没有填成功）")
            return
        await _per_image(
            job,
            "style_transfer_remote",
            {"prompt": compiled.prompt, "size": env.size},
            chosen,
            ctx,
            produced,
            failures,
            note_fn=lambda n, total: f"远程重绘 {n}/{total}（{env.tier}）",
        )

    elif spec.deliverable == "card_pdf" or spec.deliverable == "card_images":
        await _run_cards(job, spec, ctx, chosen, produced, failures)

    elif spec.retouch and spec.retouch.faces:
        await _per_image(
            job,
            "beauty_render",
            {"strength": spec.retouch.strength},
            chosen,
            ctx,
            produced,
            failures,
            note_fn=lambda n, total: f"人像润色 {n}/{total}（{spec.retouch.strength}）",
        )

    elif spec.style and spec.style.engine == "local":
        level = intensity_level(spec.style.intensity)
        await _per_image(
            job,
            "style_transfer",
            {"preset": spec.style.preset, "level": level},
            chosen,
            ctx,
            produced,
            failures,
            note_fn=lambda n, total: f"本地调色 {n}/{total}（{spec.style.preset} · {level}）",
        )
        # 批量素材：调色后还要统一尺寸/体积
        if spec.export.target_kb or spec.export.max_edge:
            await _per_image(
                job,
                "compress",
                {
                    "targetKb": spec.export.target_kb,
                    "maxEdge": spec.export.max_edge,
                    "crop": spec.export.crop,
                },
                produced,
                ctx,
                produced,
                failures,
                replace=True,
                note_fn=lambda n, total: f"统一压缩 {n}/{total}"
                + (f"（目标 {spec.export.target_kb}KB）" if spec.export.target_kb else ""),
            )

    elif spec.export.target_kb or spec.export.max_edge:
        await _per_image(
            job,
            "compress",
            {
                "targetKb": spec.export.target_kb,
                "maxEdge": spec.export.max_edge,
                "crop": spec.export.crop,
            },
            chosen,
            ctx,
            produced,
            failures,
            note_fn=lambda n, total: f"压缩 {n}/{total}"
            + (f"（目标 {spec.export.target_kb}KB）" if spec.export.target_kb else ""),
        )

    else:
        job.finish("error", f"配方 {recipe.id} 没有可执行的步骤（RunSpec 缺 style/retouch/export）")
        return

    if job.is_cancelled():
        job.finish("cancelled", "已取消")
        return

    # ③ 产物体检（坏图当场抓出来，而不是等用户打开才发现）
    if produced:
        chk = await dispatch("probe", {"paths": produced}, ctx)
        bad = int(chk.get("badCount", 0))
        job.add_step(
            "probe",
            note=f"体检 {len(produced)} 个产物" + (f"，{bad} 个有问题" if bad else "，全部正常"),
            latency_ms=0,
            cost=0.0,
        )
        if bad and bad == len(produced):
            job.finish("error", "所有产物都没通过体检（详见轨迹）")
            return

    if not produced:
        job.finish("error", "没有产出任何文件" + (f"（失败 {len(failures)} 张）" if failures else ""))
        return

    summary = f"完成 {len(produced)} 项"
    if failures:
        summary += f"，失败 {len(failures)} 项：{failures[0][:60]}"
    job.add_step("done", note=summary + f" · 产物目录 {out_dir}", latency_ms=0, cost=0.0)
    job.set_meta(outputs=produced)
    job.finish("done")


async def _vision_thumbs(paths: list[str], n: int = 4, edge: int = 512) -> list[bytes]:
    """规划阶段只传缩略图：≤4 张、≤512px（§14.4 额度合并计数）。

    传原图会直接撞端点限制（或把 token 烧光），所以尺寸必须封顶；
    单张读不了也不阻止规划（让它少一张上下文，而不是整个任务失败）。
    """
    import asyncio as _aio

    from .tools import _to_png_bytes

    out: list[bytes] = []
    for p in paths[:n]:
        try:
            out.append(await _aio.to_thread(_to_png_bytes, p, edge))
        except Exception:  # noqa: BLE001
            continue
    return out


async def _fill_slots(
    job: Job,
    recipe: Recipe,
    compiled: CompileOutput,
    chosen: list[str],
    env: RunEnv,
) -> None:
    """P1b：模型只填槽位，最终文本由模板渲染（锚点/禁用词/隐私校验全在本地）。"""
    tpl = recipe.prompt_template
    if tpl is None or env.planner is None:
        raise RuntimeError("P1b 需要提示词模板与规划模型，但其中一项缺失")

    thumbs = await _vision_thumbs(chosen)
    t0 = time.monotonic()
    raw_slots, notes = await env.planner.fill_slots(
        recipe, tpl, str(compiled.user_text or ""), thumbs
    )
    assumptions = list(notes)
    slots, prompt = finalize_filled(tpl, raw_slots, assumptions)

    cost = _planner_cost(env)
    model = getattr(getattr(env.planner, "client", None), "ep", None)
    job.add_step(
        "fill_slots",
        model=getattr(model, "model", None),
        latency_ms=int((time.monotonic() - t0) * 1000),
        cost=cost,
        note=f"模型填参：{_slots_brief(slots)}",
    )
    job.set_meta(
        slots=slots,
        prompt=prompt,
        promptSource="template",
        assumptions=list(job.snapshot().get("assumptions") or []) + assumptions,
    )
    compiled.slots = slots
    compiled.prompt = prompt
    logutil.err(f"[job {job.job_id}] P1b slots={slots}")


def _planner_cost(env: RunEnv) -> float:
    """规划调用拿不到端点真实计费时的估价（用用户填的单价）。"""
    table = getattr(env.budget, "table", None)
    return float(getattr(table, "planner", 0.0) or 0.0)


def _slots_brief(slots: dict) -> str:
    order = ["look", "intensity", "note"]
    parts = [f"{k}={slots[k]}" for k in order if k in slots and slots[k] != ""]
    return "，".join(parts) or "（空）"


async def _per_image(
    job: Job,
    tool: str,
    args_base: dict,
    paths: list[str],
    ctx: ToolContext,
    produced: list[str],
    failures: list[str],
    *,
    replace: bool = False,
    note_fn=None,
) -> None:
    """逐张执行，每张一条轨迹（进度可见、可取消、出问题能定位到具体文件）。"""
    total = len(paths)
    results: list[str] = []
    for i, src in enumerate(paths, start=1):
        if job.is_cancelled():
            return
        t0 = time.monotonic()
        args = dict(args_base)
        args["paths"] = [src]
        res = await dispatch(tool, args, ctx)
        latency = int((time.monotonic() - t0) * 1000)
        items = res.get("items") or []
        out_path = items[0].get("out") if items else None
        thumb = items[0].get("thumb") if items else None
        # 远程重绘的每一步带真实成本（拿不到计费时用预算预占价，Rust 侧标 estimated）
        step_cost = float(items[0].get("cost") or 0.0) if items else 0.0
        failures.extend(res.get("failures") or [])
        if not res.get("ok"):
            failures.append(f"{Path(src).name}：{res.get('error') or '未产出'}")
        if out_path:
            results.append(str(out_path))
        note = None
        if note_fn:
            note = note_fn(i, total)
        if items and items[0].get("note"):
            note = (note or "") + f" · {items[0]['note']}"
        job.add_step(tool, note=note or f"{TOOL_LABEL.get(tool, tool)} {i}/{total}", latency_ms=latency, cost=step_cost, thumb=thumb)
    if replace:
        produced.clear()
    produced.extend(results)


async def _run_cards(
    job: Job,
    spec,
    ctx: ToolContext,
    chosen: list[str],
    produced: list[str],
    failures: list[str],
) -> None:
    layout = spec.layout.template if spec.layout else "magazine_3up"
    title = spec.layout.title if spec.layout else ""
    subtitle = spec.layout.subtitle if spec.layout else ""
    formats = list(spec.export.formats) or ["pdf"]
    if spec.deliverable == "card_images" and "pdf" in formats:
        formats = [f for f in formats if f != "pdf"] or ["jpg"]

    t0 = time.monotonic()
    res = await dispatch(
        "card_render",
        {
            "paths": chosen,
            "layout": layout,
            "title": title,
            "subtitle": subtitle,
            "formats": formats,
            "pageSize": spec.export.page_size,
            "maxPages": spec.export.max_pages,
        },
        ctx,
    )
    latency = int((time.monotonic() - t0) * 1000)
    if not res.get("ok"):
        failures.append(f"卡片排版失败：{res.get('error') or '未产出'}")
        return
    page_count = int(res.get("pageCount") or 0)
    note = f"排版 {layout} → {page_count} 页"
    if res.get("imagesDropped"):
        note += f"（超页数上限舍弃 {res['imagesDropped']} 张）"
    job.add_step("card_render", note=note, latency_ms=latency, cost=0.0, thumb=res.get("thumb"))
    if res.get("pdf"):
        produced.append(str(res["pdf"]))
    produced.extend(res.get("pages") or [])
