"""参考图颜色迁移工作流：把参考图的色调 / 影调迁到每张素材图，输出 JPG。

纯本机、零网络、零付费。算法在 colortransfer.py（Reinhard / 直方图匹配），
可选把这次迁移固化成 .cube 方便以后当滤镜复用。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import colortransfer
from .base import check_cancel, read_image, save_image, stem_of


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    ref_path = str(opts.get("referencePath") or "").strip()
    method = str(opts.get("method", "reinhard") or "reinhard")
    strength = float(opts.get("strength", 1.0) or 1.0)
    export_cube = bool(opts.get("exportCube", False))

    if not ref_path:
        raise ValueError("未选择参考图")
    if not os.path.isfile(ref_path):
        raise FileNotFoundError(f"参考图不存在: {ref_path}")
    ref_arr = np.asarray(read_image(ref_path), dtype=np.uint8)

    outputs: list[str] = []
    warnings: list[str] = []
    n = len(req.paths)
    for i, p in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "颜色迁移中", f"{i + 1}/{n}")
        arr = np.asarray(read_image(p), dtype=np.uint8)
        if method == "histogram":
            out_arr = colortransfer.histogram_match(arr, ref_arr, strength=strength)
        else:
            out_arr = colortransfer.reinhard_transfer(arr, ref_arr, strength=strength)
        out = save_image(Image.fromarray(out_arr), out_dir, f"{stem_of(p)}_迁移", ".jpg", 95)
        outputs.append(out)

    if export_cube:
        try:
            text = colortransfer.build_cube_text(ref_arr, size=33)
            cube_path = os.path.join(out_dir, f"参考图调色_{stem_of(ref_path)}.cube")
            with open(cube_path, "w", encoding="utf-8") as fh:
                fh.write(text)
            outputs.append(cube_path)
        except Exception as e:  # noqa: BLE001 —— 导出失败不应让整批作业失败
            warnings.append(f"导出 .cube 失败: {e}")

    progress(100, "完成", f"{len(req.paths)} 张")
    return {"outputs": outputs, "warnings": warnings, "meta": {"method": method}}
