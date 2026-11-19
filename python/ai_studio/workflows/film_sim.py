"""胶片模拟工作流：每张图套用内置 .cube LUT，可选颗粒 / 暗角，输出 JPG。

纯本机、零网络、零付费。LUT 随包内置在 python/ai_studio/luts/，
通过 config.LUTS 声明；真正的查表计算在 lutcube.py。

并行：照片级（无 ONNX，GIL 在 numpy 大数组运算时释放），整批接近线性加速。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import config, lutcube
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

# LUT 查询是纯 CPU 大数组运算（np.take + 整数算术），实测 4 线程 2.37x，
# 可以放心取满槽位（见 parallel.workers_for）。
_ONNX_HEAVY = False


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)
    lut_id = str(opts.get("lut", "provia") or "provia")
    strength = float(opts.get("strength", 1.0) or 1.0)
    anchor = float(opts.get("anchor", 0.0) or 0.0)
    grain = float(opts.get("grain", 0.0) or 0.0)
    vignette = float(opts.get("vignette", 0.0) or 0.0)

    entry = next((l for l in config.LUTS if l["id"] == lut_id), None)
    if entry is None:
        raise ValueError(f"未知的胶片风格: {lut_id}")
    path = config.lut_path(entry["file"])
    if not os.path.isfile(path):
        raise FileNotFoundError(f"内置 LUT 缺失: {entry['file']}")

    # 颗粒的种子用**照片下标**而不是完成顺序：并行后同一个文件多次运行应得到
    # 同一张颗粒图（可复现），而 seed=i 的语义与旧串行版逐位一致。
    def process(index: int, p: str) -> Outcome:
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)
        arr = lutcube.apply_cube(arr, path, strength=strength)
        if anchor > 0:
            arr = lutcube.restore_black_white(arr, anchor)
        if grain > 0:
            arr = lutcube.apply_grain(arr, grain, seed=index)
        if vignette > 0:
            arr = lutcube.apply_vignette(arr, vignette)
        out = save_image(
            Image.fromarray(arr), out_dir, f"{stem_of(p)}_胶片_{entry['id']}", ".jpg", 95
        )
        return Outcome(outputs=[out])

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
        "meta": {"lut": lut_id},
    }
