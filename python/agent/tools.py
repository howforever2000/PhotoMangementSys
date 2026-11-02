"""原子 Skill：单步骤、无状态、输入输出明确。

分层的意义（docs/v5.md §10）：
- **原子 Skill**（本文件）：Agent 可直接调，Expert Skill 内部也组合它们；
- **Expert Skill**（runtime.py 的编排）：一个完整交付物的生产流程；
- **Scenario**：意图模糊/多交付物时的路径规划（规划层负责，不硬编码）。

`select` 与 `compress` 这类"选图/改图"必须分开：选图不该有副作用，
改图不该悄悄改变选中的集合。
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
from pathlib import Path

from .imaging import beauty, compress as comp, journal, probe, style
from .imaging.imageio import ImageIoError, make_thumb

# 单次任务最多处理多少张（防手滑全选几千张把磁盘写满）
MAX_IMAGES_PER_JOB = 60


@dataclass
class ToolContext:
    """工具执行上下文。刻意不放 job 对象：工具是纯功能，进度由 runtime 记。"""

    out_dir: str
    thumbs_dir: str
    overwrite: bool = False


def unique_path(path: str) -> str:
    """同名已存在时追加 `_2`、`_3`…（overwrite=false 时绝不覆盖用户文件）。"""
    p = Path(path)
    if not p.exists():
        return str(p)
    for i in range(2, 999):
        cand = p.with_name(f"{p.stem}_{i}{p.suffix}")
        if not cand.exists():
            return str(cand)
    return str(p.with_name(f"{p.stem}_{Path(p).stat().st_mtime_ns}{p.suffix}"))


def _thumb(ctx: ToolContext, path: str, tag: str) -> str:
    dst = str(Path(ctx.thumbs_dir) / f"{Path(path).stem}_{tag}.jpg")
    try:
        return make_thumb(path, dst)
    except Exception:  # noqa: BLE001 —— 缩略图是"锦上添花"，失败不该毁任务
        return ""


# ------------------------------------------------------------------ select


def select(paths: list[str], limit: int = MAX_IMAGES_PER_JOB) -> dict:
    """按给定顺序去重截断。

    顺序有意义：**手选托盘的顺序就是用户心里的顺序**，不要排序、
    不要按文件名重排（那会让"第一张当主图"变成随机结果）。
    """
    seen: set[str] = set()
    kept: list[str] = []
    dropped = 0
    for p in paths:
        key = str(p).lower()
        if not p or key in seen:
            dropped += 1
            continue
        seen.add(key)
        kept.append(str(p))
    if len(kept) > limit:
        dropped += len(kept) - limit
        kept = kept[:limit]
    return {"ok": True, "paths": kept, "count": len(kept), "dropped": dropped}


# ------------------------------------------------------------------ 单图变换


def compress(paths: list[str], ctx: ToolContext, *, target_kb: int = 0, max_edge: int = 0, crop: str = "") -> dict:
    outs: list[dict] = []
    failures: list[str] = []
    for src in paths:
        dst = str(Path(ctx.out_dir) / f"{Path(src).stem}_c.jpg")
        if not ctx.overwrite:
            dst = unique_path(dst)
        try:
            r = comp.compress(src, dst, target_kb=target_kb, max_edge=max_edge, crop=crop)
            item = r.to_dict()
            item["thumb"] = _thumb(ctx, dst, "c")
            outs.append(item)
            if not r.ok:
                failures.append(f"{Path(src).name}：{r.note}")
        except ImageIoError as e:
            failures.append(f"{Path(src).name}：{e}")
    return {"ok": bool(outs), "items": outs, "failures": failures}


def style_transfer(paths: list[str], ctx: ToolContext, *, preset: str, level: str) -> dict:
    outs: list[dict] = []
    failures: list[str] = []
    for src in paths:
        dst = str(Path(ctx.out_dir) / f"{Path(src).stem}_{preset}.jpg")
        if not ctx.overwrite:
            dst = unique_path(dst)
        try:
            r = style.apply_style(src, dst, preset, level)
            item = r.to_dict()
            item["thumb"] = _thumb(ctx, dst, preset)
            outs.append(item)
        except ImageIoError as e:
            failures.append(f"{Path(src).name}：{e}")
    return {"ok": bool(outs), "items": outs, "failures": failures}


def beauty_render(paths: list[str], ctx: ToolContext, *, strength: str = "natural") -> dict:
    outs: list[dict] = []
    failures: list[str] = []
    for src in paths:
        dst = str(Path(ctx.out_dir) / f"{Path(src).stem}_retouch.jpg")
        if not ctx.overwrite:
            dst = unique_path(dst)
        try:
            r = beauty.smooth(src, dst, strength)
            item = r.to_dict()
            item["thumb"] = _thumb(ctx, dst, "retouch")
            outs.append(item)
        except ImageIoError as e:
            failures.append(f"{Path(src).name}：{e}")
    return {"ok": bool(outs), "items": outs, "failures": failures}


# ------------------------------------------------------------------ 组合交付物


def card_render(
    paths: list[str],
    ctx: ToolContext,
    *,
    layout: str,
    title: str,
    subtitle: str,
    formats: list[str],
    page_size: str,
    max_pages: int,
) -> dict:
    try:
        r = journal.export_cards(
            paths,
            ctx.out_dir,
            layout=layout,
            title=title,
            subtitle=subtitle,
            formats=formats,
            page_size=page_size,
            max_pages=max_pages,
        )
        data = r.to_dict()
        preview = data["pages"][0] if data["pages"] else (data["pdf"] or "")
        data["thumb"] = _thumb(ctx, preview, "card") if preview else ""
        data["ok"] = bool(data["pages"] or data["pdf"])
        return data
    except (ImageIoError, OSError) as e:
        return {"ok": False, "error": str(e), "pages": [], "pdf": None}


def probe_outputs(paths: list[str]) -> dict:
    infos = [probe.inspect(p) for p in paths]
    bad = [i for i in infos if not i.get("ok")]
    return {"ok": not bad, "items": infos, "badCount": len(bad)}


# ------------------------------------------------------------------ dispatch


async def dispatch(name: str, args: dict, ctx: ToolContext) -> dict:
    """工具分发表。CPU 密集的部分丢到线程池，避免阻塞事件循环（轮询会超时）。"""
    if name == "select":
        return await asyncio.to_thread(select, args.get("paths", []), int(args.get("limit", MAX_IMAGES_PER_JOB)))
    if name == "compress":
        return await asyncio.to_thread(
            compress,
            args.get("paths", []),
            ctx,
            target_kb=int(args.get("targetKb", 0) or 0),
            max_edge=int(args.get("maxEdge", 0) or 0),
            crop=str(args.get("crop", "") or ""),
        )
    if name == "style_transfer":
        return await asyncio.to_thread(
            style_transfer,
            args.get("paths", []),
            ctx,
            preset=str(args.get("preset", "warm_dusk")),
            level=str(args.get("level", "moderate")),
        )
    if name == "beauty_render":
        return await asyncio.to_thread(
            beauty_render, args.get("paths", []), ctx, strength=str(args.get("strength", "natural"))
        )
    if name == "card_render":
        return await asyncio.to_thread(
            card_render,
            args.get("paths", []),
            ctx,
            layout=str(args.get("layout", "magazine_3up")),
            title=str(args.get("title", "")),
            subtitle=str(args.get("subtitle", "")),
            formats=list(args.get("formats", ["pdf"])),
            page_size=str(args.get("pageSize", "a4")),
            max_pages=int(args.get("maxPages", 12)),
        )
    if name == "probe":
        return await asyncio.to_thread(probe_outputs, args.get("paths", []))
    raise ValueError(f"未知工具 {name!r}")


TOOL_NAMES = ("select", "compress", "style_transfer", "beauty_render", "card_render", "probe")
