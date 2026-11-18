"""自适应调色工作流：每张图先由统计推出调色参数，固化成 LUT 后查表输出。

纯本机、零网络、零付费、零模型下载。算法在 `autograde.py`，查表在 `lutcube.py`，
本文件只负责编排（读图 / 逐张处理 / 进度 / 落盘 / 可选导出 .cube）。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import autograde, colortransfer
from .base import check_cancel, filter_inputs, read_image, save_image, stem_of


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    style = str(opts.get("style", autograde.DEFAULT_STYLE) or autograde.DEFAULT_STYLE)
    if style not in autograde.STYLES:
        style = autograde.DEFAULT_STYLE
    strength = float(opts.get("strength", 1.0) or 1.0)
    export_cube = bool(opts.get("exportCube", False))

    outputs: list[str] = []
    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)
    n = len(req.paths)
    last_lut = None
    last_stats = None

    for i, p in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "自适应调色中", f"{i + 1}/{n}")
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)
        try:
            graded, stats, lut = autograde.grade(arr, style=style, strength=strength)
        except Exception as e:  # noqa: BLE001 —— 单张失败不拖垮整批
            warnings.append(f"{os.path.basename(p)}: 调色失败，已跳过（{e}）")
            continue
        last_lut, last_stats = lut, stats
        out = save_image(
            Image.fromarray(graded), out_dir, f"{stem_of(p)}_自适应调色", ".jpg", 95
        )
        outputs.append(out)

    # 导出 .cube：用**最后一张**图的参数（同一风格下，逐图参数才是「自适应」的本意，
    # 所以明确按「这张图」导出，而不是假装有一个全局预设）。
    if export_cube and last_lut is not None and last_stats is not None:
        try:
            cube_path = os.path.join(out_dir, "自适应调色_autograde.cube")
            with open(cube_path, "w", encoding="utf-8") as fh:
                fh.write(colortransfer.cube_text_from_lut(last_lut[0], last_lut[1]))
            outputs.append(cube_path)
        except Exception as e:  # noqa: BLE001 —— 导出失败不应让整批作业失败
            warnings.append(f"导出 .cube 失败: {e}")

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "style": style,
            "strength": round(strength, 4),
            "stats": last_stats.as_meta() if last_stats else None,
        },
    }
