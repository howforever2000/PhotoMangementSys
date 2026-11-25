"""胶片模拟工作流：内置 .cube LUT 或参数化胶片引擎，可选颗粒 / 暗角，输出 JPG。

两条引擎（默认 lut，老行为逐位不变）：
  lut   直接三线性查表（`lutcube.apply_cube`），7 套内置风格；
  film  参数化自适应渲染（`film_engine`）：先读场景统计，再按配方渲染，
        新增 Kodak Portra 400 / Kodak 2383 只有这条路径（没有对应 .cube）。

强度语义（两条线分开，避免一个滑杆同时动「还原度」和「偏色」）：
  look    风格强度 0.6 / 1.0 / 1.4：偏离原厂的程度（仅 film）；
  strength 总强度 0~1：film 下是「成片与原图的混合比例」；
  grade   后调层（青橙 / 复古琥珀 / 冷青灰），决定偏色多明显（仅 film）。

并行：照片级（无 ONNX，GIL 在 numpy 大数组运算时释放），整批接近线性加速。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import config, film_render, film_styles
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

# LUT 查询是纯 CPU 大数组运算（np.take + 整数算术），实测 4 线程 2.37x；
# 参数化引擎同样无 ONNX，可以放心取满槽位（见 parallel.workers_for）。
_ONNX_HEAVY = False


def _num(opts: dict, key: str, default: float) -> float:
    """容错取数：前端传 "" / null 时回落到默认值（旧配方里 grain 有 None 的历史）。"""
    v = opts.get(key, default)
    if v is None or v == "":
        return default
    return float(v)


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    style_id = str(opts.get("style") or opts.get("lut") or "provia")
    engine, _recipe = film_styles.resolve(style_id, str(opts.get("engine") or "auto"))
    strength = _num(opts, "strength", 1.0)
    look = _num(opts, "look", 1.0)
    grade = str(opts.get("grade") or "")
    grade_strength = _num(opts, "grade_strength", 1.0)
    anchor = _num(opts, "anchor", 0.0)
    grain = _num(opts, "grain", 0.0)
    vignette = _num(opts, "vignette", 0.0)

    lut_path = None
    if engine == "lut":
        entry = film_styles.lut_entry(style_id)
        if entry is None:
            raise ValueError(f"未知的胶片风格: {style_id}")
        lut_path = config.lut_path(entry["file"])
        if not os.path.isfile(lut_path):
            raise FileNotFoundError(f"内置 LUT 缺失: {entry['file']}")

    # 老 id 显式走 film 时另起后缀：否则会和同一风格的历史 LUT 产物同名互相覆盖
    name_base = f"{style_id}_film" if (engine == "film" and film_styles.has_lut(style_id)) else style_id

    # 颗粒的种子用**照片下标**而不是完成顺序：并行后同一个文件多次运行应得到
    # 同一张颗粒图（可复现），而 seed=i 的语义与旧串行版逐位一致。
    def process(index: int, p: str) -> Outcome:
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)
        stats = film_render.film_engine.analyze(arr) if engine == "film" else None
        out, _meta = film_render.apply(
            arr,
            {
                "style": style_id, "engine": engine, "strength": strength, "look": look,
                "grade": grade, "grade_strength": grade_strength, "anchor": anchor,
                "grain": grain, "vignette": vignette,
            },
            lut_file_path=lut_path,
            seed=index,
            stats=stats,
        )
        path = save_image(
            Image.fromarray(out), out_dir, f"{stem_of(p)}_胶片_{name_base}", ".jpg", 95
        )
        return Outcome(outputs=[path])

    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="胶片模拟中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
    )
    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {
            "style": style_id,
            "engine": engine,
            "look": look,
            "grade": grade,
            "styleVersion": "v3",
        },
    }
