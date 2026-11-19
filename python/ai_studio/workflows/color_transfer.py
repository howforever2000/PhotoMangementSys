"""参考图颜色迁移工作流：把参考图的色调 / 影调迁到每张素材图，输出 JPG。

纯本机、零网络、零付费。算法在 colortransfer.py（Reinhard / 直方图匹配），
可选把这次迁移固化成 .cube 方便以后当滤镜复用。

两个性能纪律（都是实测出来的）：

1. **参考图只取统计，不按原分辨率转 Lab**。Reinhard / 直方图匹配只用 ref 的
   Lab 均值、标准差与分位分布，这些量与分辨率无关；而 24MP 转一次 Lab 要 2145ms。
   参考图降到长边 `_REF_LONG_EDGE` 后转，同样的统计量只要 24ms。
2. **照片级并行**（纯 CPU，GIL 释放充分），整批接近线性。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import colortransfer
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

_ONNX_HEAVY = False

# 参考图分析用的长边：统计量与分辨率无关，1024 已足够稳定（0.19MP 转 Lab 仅 24ms；
# 24MP 是 2145ms）。直方图匹配的分位精度在 1MP 上误差已是千分级。
_REF_LONG_EDGE = 1024


def _fit_ref(img: Image.Image) -> Image.Image:
    """参考图缩到长边 `_REF_LONG_EDGE`（只缩不放）。"""
    w, h = img.size
    m = max(w, h)
    if m <= _REF_LONG_EDGE:
        return img
    s = _REF_LONG_EDGE / float(m)
    return img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)


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
    # 参考图只在这一处解码一次（旧版每张图都全量转 Lab，同一张 ref 被算 N 次）。
    ref_arr = np.asarray(_fit_ref(read_image(ref_path)), dtype=np.uint8)

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, p: str) -> Outcome:
        arr = np.asarray(read_image(p), dtype=np.uint8)
        if method == "histogram":
            out_arr = colortransfer.histogram_match(arr, ref_arr, strength=strength)
        else:
            out_arr = colortransfer.reinhard_transfer(arr, ref_arr, strength=strength)
        out = save_image(Image.fromarray(out_arr), out_dir, f"{stem_of(p)}_迁移", ".jpg", 95)
        return Outcome(outputs=[out])

    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="颜色迁移中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
    )

    outputs = list(res.outputs)
    if export_cube:
        try:
            text = colortransfer.build_cube_text(ref_arr, size=33)
            cube_path = os.path.join(out_dir, f"参考图调色_{stem_of(ref_path)}.cube")
            with open(cube_path, "w", encoding="utf-8") as fh:
                fh.write(text)
            outputs.append(cube_path)
        except Exception as e:  # noqa: BLE001 —— 导出失败不应让整批作业失败
            res.warnings.append(f"导出 .cube 失败: {e}")

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": res.warnings,
        "meta": {"method": method},
    }
