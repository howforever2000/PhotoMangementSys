"""胶片模拟的统一执行层：LUT 路径与参数化路径（供工作流与本地算子共用）。

为什么要一个中间层：
  同一条风格在 `workflows/film_sim.py`（批量、并行）和 `ops.py`（预览 / 计划执行）
  里会被调用两次。两处各写一份分支，迟早出现「预览一个样、出片另一个样」；
  这里把「id + 引擎 → 像素」收成一处，上层只负责 IO 与进度。

兼容纪律：LUT 路径（engine=lut）保持原有调用顺序与参数，**逐位不变**；
参数化路径只在新引擎下生效，默认引擎仍是 lut。
"""
from __future__ import annotations

from typing import Any, Tuple

import numpy as np

from . import film_engine, film_styles, lutcube


def lut_array(rgb: np.ndarray, entry: dict, path: str, strength: float = 1.0,
              anchor: float = 0.0, grain: float = 0.0, vignette: float = 0.0,
              seed: int = 0) -> np.ndarray:
    """老路径：.cube 查表（可选黑白场还原 / 颗粒 / 暗角）。"""
    arr = lutcube.apply_cube(rgb, path, strength=strength)
    if anchor > 0:
        arr = lutcube.restore_black_white(arr, anchor)
    if grain > 0:
        arr = lutcube.apply_grain(arr, grain, seed=seed)
    if vignette > 0:
        arr = lutcube.apply_vignette(arr, vignette)
    return arr


def film_array(rgb: np.ndarray, recipe: dict, stats: dict, look: float = 1.0,
               strength: float = 1.0, anchor: float = 0.0, grain: float = 0.0,
               vignette: float = 0.0, grade: str = "", grade_strength: float = 0.0,
               seed: int = 0) -> np.ndarray:
    """参数化路径：L1 风格渲染 → L2 后调 → 强度混合 → 附加颗粒/暗角。"""
    out, _ = film_engine.render(rgb, recipe, stats, seed=seed, look=look)

    if grade:
        preset = film_styles.GRADES.get(grade)
        if preset is None:
            raise ValueError(f"未知的后调: {grade}")
        out = film_engine.apply_grade(out, preset, grade_strength)

    s = float(np.clip(strength, 0.0, 1.0))
    if s < 0.999:
        mixed = rgb.astype(np.float32) * (1.0 - s) + out.astype(np.float32) * s
        out = np.clip(mixed + 0.5, 0, 255).astype(np.uint8)

    if anchor > 0:
        out = lutcube.restore_black_white(out, anchor)
    if grain > 0:
        out = lutcube.apply_grain(out, grain, seed=seed)
    if vignette > 0:
        out = lutcube.apply_vignette(out, vignette)
    return out


def apply(rgb: np.ndarray, opts: dict, lut_file_path: str | None = None,
          seed: int = 0, stats: dict | None = None) -> Tuple[np.ndarray, dict]:
    """按 opts 执行一次胶片模拟，返回 (RGB, meta)。

    opts 字段：
      lut / style   风格 id（两者同义，`style` 优先）
      engine        auto / lut / film（默认 auto：有 LUT 走老路径）
      strength      总强度 0~1（film 路径是「与原始混合」，lut 路径沿用查表强度）
      look          风格强度（轻 0.6 / 标准 1.0 / 浓 1.4，仅 film）
      grade         后调预设 id（空 = 不叠后调，仅 film）
      grade_strength 后调强度 0~1.4
      anchor / grain / vignette   与老参数同义
    """
    style_id = str(opts.get("style") or opts.get("lut") or "provia")
    engine, recipe = film_styles.resolve(style_id, str(opts.get("engine") or "auto"))
    look = float(opts.get("look", 1.0) or 1.0)
    strength = float(opts.get("strength", 1.0) if opts.get("strength") is not None else 1.0)
    anchor = float(opts.get("anchor", 0.0) or 0.0)
    grain = float(opts.get("grain", 0.0) or 0.0)
    vignette = float(opts.get("vignette", 0.0) or 0.0)
    grade = str(opts.get("grade") or "")
    grade_strength = float(opts.get("grade_strength", 1.0) or 1.0)

    if engine == "lut":
        entry = film_styles.lut_entry(style_id)
        assert entry is not None
        if not lut_file_path:
            raise FileNotFoundError("内置 LUT 缺失（未提供路径）")
        out = lut_array(rgb, entry, lut_file_path, strength=strength, anchor=anchor,
                        grain=grain, vignette=vignette, seed=seed)
        meta = {"style": style_id, "engine": "lut", "look": 1.0, "grade": ""}
        return out, meta

    if stats is None:
        stats = film_engine.analyze(rgb)
    out = film_array(rgb, recipe, stats, look=look, strength=strength, anchor=anchor,
                     grain=grain, vignette=vignette, grade=grade,
                     grade_strength=grade_strength, seed=seed)
    meta = {
        "style": style_id,
        "engine": "film",
        "look": look,
        "grade": grade if grade else "",
        "exposureStops": round(float(stats["features"]["contrast_stops"]), 2),
    }
    return out, meta
