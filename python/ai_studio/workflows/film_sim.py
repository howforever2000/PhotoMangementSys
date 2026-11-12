"""胶片模拟工作流：每张图套用内置 .cube LUT，可选颗粒 / 暗角，输出 JPG。

纯本机、零网络、零付费。LUT 随包内置在 python/ai_studio/luts/，
通过 config.LUTS 声明；真正的查表计算在 lutcube.py。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import config, lutcube
from .base import check_cancel, read_image, save_image, stem_of


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    lut_id = str(opts.get("lut", "provia") or "provia")
    strength = float(opts.get("strength", 1.0) or 1.0)
    grain = float(opts.get("grain", 0.0) or 0.0)
    vignette = float(opts.get("vignette", 0.0) or 0.0)

    entry = next((l for l in config.LUTS if l["id"] == lut_id), None)
    if entry is None:
        raise ValueError(f"未知的胶片风格: {lut_id}")
    path = config.lut_path(entry["file"])
    if not os.path.isfile(path):
        raise FileNotFoundError(f"内置 LUT 缺失: {entry['file']}")

    outputs: list[str] = []
    n = len(req.paths)
    for i, p in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "胶片模拟中", f"{i + 1}/{n}")
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)
        arr = lutcube.apply_cube(arr, path, strength=strength)
        if grain > 0:
            arr = lutcube.apply_grain(arr, grain, seed=i)
        if vignette > 0:
            arr = lutcube.apply_vignette(arr, vignette)
        out = save_image(
            Image.fromarray(arr), out_dir, f"{stem_of(p)}_胶片_{entry['id']}", ".jpg", 95
        )
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {"outputs": outputs, "warnings": [], "meta": {"lut": lut_id}}
