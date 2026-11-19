"""AI 创作 · 单张同步预览（服务层，FEAT-152 延伸能力）

职责（只做一件事）：
    一张照片 + 一个计划 → **只跑本机算子** → 长边缩到 maxEdge → 内存 JPEG →
    base64 data URL。供修图工作台拖动滑杆时同步拿回效果图。
    不含 HTTP 细节（路由在 `server.py`），不含作业/进度框架（在 `jobs.py` / `workflows/`），
    也不含算子实现（在 `ops.py` / `ops_local.py`）。

为什么**不落盘**：
  1. 预览是滑杆级高频调用（一次 100~300ms，用户拖一次滑杆会连发十几次）。落盘会在用户
     输出目录堆出大量 `_预览_N.jpg` 垃圾文件——本项目「相册只是本地文件夹的抽象，不搬移、
     不污染用户文件」，预览不在该职责内，也没有清理机制。
  2. 落盘会把磁盘 IO 塞进批量作业的写成品路径，批量出片时与成品写盘互相争用。
  3. 前端只要一张 data URL 就能渲染，不需要路径 → 也就没有「临时文件何时删」的生命周期问题。

边界纪律：
  - 只跑本机算子（`backend == "local"`）：云端步每次都是计费调用，滑杆拖动会把费用放大上百倍。
    云端步一律摘掉并记入 `dropped` / `warnings`，让用户明确知道「预览里没有这一步」。
  - 本机资源未就绪的步由 `recipecatalog.drop_unready` 摘掉（复用既有降级策略，缺资源不整单失败）。
  - 可读错误一律抛 `PreviewError`（接口层转 HTTP 400），不向接口层抛裸异常栈。
"""
from __future__ import annotations

import base64
import time
from io import BytesIO
from typing import Any, Dict, List, Tuple

from PIL import Image

from . import catalog, pipeline, recipecatalog
from .logutil import log
from .workflows.base import read_image

# maxEdge 的夹取区间：太小看不清效果，太大则单帧 base64 过桥（IPC）就卡。
MAX_EDGE_DEFAULT = 1280
MAX_EDGE_MIN = 320
MAX_EDGE_MAX = 2048
JPEG_QUALITY = 88


class PreviewError(ValueError):
    """预览失败；消息已是可展示的中文，接口层直接 400 返回。"""


def _clamp_edge(raw: Any) -> int:
    """maxEdge 夹到 [320, 2048]；缺失或非法值回落 1280（滑杆预览统一档位）。"""
    try:
        v = int(raw)
    except (TypeError, ValueError):
        return MAX_EDGE_DEFAULT
    return max(MAX_EDGE_MIN, min(MAX_EDGE_MAX, v))


def _uniq(items: List[str]) -> List[str]:
    """去重且保序：同一步可能同时被「非本机」和「未就绪」判定，也只报一次。"""
    out: List[str] = []
    for x in items:
        if x not in out:
            out.append(x)
    return out


def _plan_from_request(
    plan: Any, recipe_id: str | None, params_override: Any
) -> Dict[str, Any]:
    """计划来源：显式 `plan` 优先；否则 `recipeId` + 参数覆盖；都没有走默认第一条配方。

    非法计划的异常（`pipeline.PlanError` 是 `ValueError`）在这里转成可读中文，
    因为 planner/前端传来的宽松 JSON 最常见的问题就是白名单外的算子或空 steps。
    """
    if plan is not None:
        try:
            selected = recipecatalog.plan_from_llm(plan)
        except ValueError as e:
            raise PreviewError(f"计划不可用: {e}") from None
        if not selected.get("steps"):
            raise PreviewError("计划里没有任何可执行的步骤")
        return selected
    try:
        return recipecatalog.apply_overrides((recipe_id or "").strip(), params_override)
    except ValueError as e:
        raise PreviewError(f"配方参数不可用: {e}") from None


def _local_only(plan: Dict[str, Any]) -> Tuple[Dict[str, Any], List[str], List[str]]:
    """摘掉非本机算子与重模型算子，返回 `(计划, 被摘 op, warnings)`。

    为什么在服务层而非法路由层做：这属于「预览能跑什么」的业务语义，
    路由层只该转发请求体。云端步必须摘掉——一次拖滑杆 = 一次计费调用。

    重模型步（`op_catalog.json` 的 `preview:false`，如 Real-ESRGAN 超分）也摘掉：
    预览最终只保留长边 1280，先跑 4× 超分再缩回去纯属浪费；24MP 图上 4× 的
    中间图还有 OOM 风险。摘除名单由 catalog 标记驱动，不在这里硬编码算子名。
    """
    kept: List[Dict[str, Any]] = []
    dropped: List[str] = []
    warnings: List[str] = []
    for st in plan.get("steps") or []:
        op = str(st.get("op") or "")
        spec = catalog.op_spec(op) or {}
        if spec.get("backend") != "local":
            label = spec.get("label") or op
            if op not in dropped:
                dropped.append(op)
                warnings.append(
                    f"「{label}」是云端算子，预览只跑本机免费算子，已跳过（正式出片时仍会执行）"
                )
            continue
        if spec.get("preview") is False:
            label = spec.get("label") or op
            if op not in dropped:
                dropped.append(op)
                warnings.append(
                    f"「{label}」是重模型算子，预览已跳过（正式出片时仍会执行）"
                )
            continue
        kept.append(st)
    return {**plan, "steps": kept}, dropped, warnings


def _step_warnings(events: List[Dict[str, Any]]) -> List[str]:
    """把执行器的步骤事件转成用户可读 warning。

    `pipeline.execute_plan` 对单步失败是「回退到该步输入并继续」，不抛错；
    预览若照原样吞掉，用户看到的就是「拖了滑杆但图没变」。这里把它翻译出来。
    """
    out: List[str] = []
    for ev in events:
        if ev.get("status") not in ("failed", "skipped"):
            continue
        label = ev.get("label") or ev.get("op")
        issues = "；".join(str(x) for x in (ev.get("issues") or []) if str(x).strip())
        if ev.get("status") == "skipped" and not issues:
            out.append(f"「{label}」在本机没有实现，预览已跳过该步")
        elif issues:
            out.append(f"「{label}」未生效，预览已跳过该步：{issues}")
        else:
            out.append(f"「{label}」未生效，预览已跳过该步")
    return out


def _fit_long_edge(img: Image.Image, max_edge: int) -> Image.Image:
    """长边缩到 max_edge（只缩不放）：预览放大只会更慢更糊，且不增加信息。"""
    w, h = img.size
    m = max(w, h)
    if m <= max_edge:
        return img
    s = max_edge / float(m)
    size = (max(1, int(round(w * s))), max(1, int(round(h * s))))
    return img.resize(size, Image.LANCZOS)


def _jpeg_data_url(img: Image.Image) -> str:
    """内存 JPEG（quality=88）→ data URL。全程 BytesIO，不落盘。"""
    buf = BytesIO()
    img.convert("RGB").save(buf, "JPEG", quality=JPEG_QUALITY)
    return "data:image/jpeg;base64," + base64.b64encode(buf.getvalue()).decode("ascii")


def _load_source(path: str) -> Image.Image:
    """读原图（EXIF 摆正 + RGB），并把 IO/解码异常转成可读中文。"""
    try:
        img = read_image(path)
    except FileNotFoundError:
        raise PreviewError(f"照片不存在: {path}") from None
    except Exception as e:  # 损坏文件 / 不支持的编码 / 权限
        raise PreviewError(f"无法读取照片: {e}") from None
    if img.width <= 0 or img.height <= 0:
        raise PreviewError("照片尺寸为 0，无法预览")
    return img


def render(
    path: str,
    recipe_id: str | None = None,
    params_override: Dict[str, Any] | None = None,
    plan: Dict[str, Any] | None = None,
    max_edge: int | None = None,
) -> Dict[str, Any]:
    """同步预览主入口：返回 `{image,width,height,ms,dropped,warnings}`。

    编排顺序（本层是唯一的编排点，供接口层直接调用）：
      选计划 → 摘非本机/重模型步 → 摘本机未就绪步 → 缩到预览长边 → 单图执行 →
      内存 JPEG。
    为什么先缩边再执行：预览最终只 deliver 长边 1280，算子在全分辨率上跑完再缩
    等于白算（滑杆级高频调用，24MP 上是数倍浪费）；先缩边也让重算子不再有机会
    处理原图。
    """
    t0 = time.perf_counter()
    src_path = str(path or "").strip()
    if not src_path:
        raise PreviewError("缺少照片路径")
    edge = _clamp_edge(max_edge)
    log(
        f"预览开始 path={src_path} recipe={recipe_id or '-'} "
        f"plan={'yes' if plan is not None else 'no'} maxEdge={edge}"
    )

    src = _fit_long_edge(_load_source(src_path), edge)
    plan_in = _plan_from_request(plan, recipe_id, params_override)
    local_plan, dropped, warnings = _local_only(plan_in)

    ready_plan, unready_warnings, unready_ops = recipecatalog.drop_unready(local_plan)
    dropped = _uniq(dropped + list(unready_ops))
    warnings = warnings + list(unready_warnings)

    events: List[Dict[str, Any]] = []
    # providers=None：本机算子不读它；万一漏进云端步，会立刻抛「未配置 provider」而不是真的计费。
    out = pipeline.execute_plan(src, ready_plan, None, on_step=events.append)
    warnings = warnings + _step_warnings(events)

    fitted = _fit_long_edge(out, edge)
    if fitted.width <= 0 or fitted.height <= 0:
        raise PreviewError("预览尺寸为 0，无法输出")
    data_url = _jpeg_data_url(fitted)
    ms = int((time.perf_counter() - t0) * 1000)
    log(
        f"预览完成 path={src_path} {fitted.width}x{fitted.height} {ms}ms "
        f"steps={len(ready_plan.get('steps') or [])} dropped={dropped or '-'} "
        f"warnings={len(warnings)} bytes={len(data_url)}"
    )
    return {
        "image": data_url,
        "width": fitted.width,
        "height": fitted.height,
        "ms": ms,
        "dropped": dropped,
        "warnings": warnings,
    }


__all__ = [
    "PreviewError",
    "render",
    "MAX_EDGE_DEFAULT",
    "MAX_EDGE_MIN",
    "MAX_EDGE_MAX",
    "JPEG_QUALITY",
]
