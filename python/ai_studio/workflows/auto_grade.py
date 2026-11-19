"""自适应调色工作流：每张图先由统计推出调色参数，固化成 LUT 后查表输出。

纯本机、零网络、零付费、零模型下载。算法在 `autograde.py`，查表在 `lutcube.py`，
本文件只负责编排（读图 / 逐张处理 / 进度 / 落盘 / 可选导出 .cube）。

并行：照片级（纯 CPU，统计 + 查表都会释放 GIL）。.cube 导出仍只按**最后一张
成功处理的图**，与旧串行版语义一致 —— 所以导出放在并行收尾之后、用保序结果的
末项，而不是在某个工作线程里顺手写。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import autograde, colortransfer
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

_ONNX_HEAVY = False


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
    # 自然饱和度微调（FEAT-186）：加法叠加在风格派生值上，默认 0 = 与旧版一致。
    # 上限夹取交给 catalog.norm_params（op_catalog.json 声明 0~0.25）与
    # autograde._clamp_output（0.45 兜底），这里只做类型收敛。
    vibrance = float(opts.get("vibrance", 0.0) or 0.0)
    export_cube = bool(opts.get("exportCube", False))

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, p: str) -> Outcome:
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)
        graded, stats, lut = autograde.grade(
            arr, style=style, strength=strength, vibrance_extra=vibrance
        )
        out = save_image(
            Image.fromarray(graded), out_dir, f"{stem_of(p)}_自适应调色", ".jpg", 95
        )
        return Outcome(outputs=[out], meta={"lut": lut, "stats": stats})

    # 「最后一张」按**照片下标**取（不是最后完成的那张）：并行完成顺序是乱序的，
    # 只有收尾回调给的 index 才与 paths 对齐。
    last: dict = {}

    def keep_last(index: int, outcome: Outcome) -> None:
        last["entry"] = outcome.meta

    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="自适应调色中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
        on_result=keep_last,
    )

    outputs = list(res.outputs)
    entry = last.get("entry") or {}
    last_lut, last_stats = entry.get("lut"), entry.get("stats")

    # 导出 .cube：用**最后一张**图的参数（同一风格下，逐图参数才是「自适应」的本意，
    # 所以明确按「这张图」导出，而不是假装有一个全局预设）。
    if export_cube and last_lut is not None and last_stats is not None:
        try:
            cube_path = os.path.join(out_dir, "自适应调色_autograde.cube")
            with open(cube_path, "w", encoding="utf-8") as fh:
                fh.write(colortransfer.cube_text_from_lut(last_lut[0], last_lut[1]))
            outputs.append(cube_path)
        except Exception as e:  # noqa: BLE001 —— 导出失败不应让整批作业失败
            res.warnings.append(f"导出 .cube 失败: {e}")

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": res.warnings,
        "meta": {
            "style": style,
            "strength": round(strength, 4),
            "vibrance": round(vibrance, 4),
            "stats": last_stats.as_meta() if last_stats else None,
        },
    }
