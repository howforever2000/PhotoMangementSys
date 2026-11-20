"""画质增强工作流（FEAT-152 → 切本机）：每张图 → Real-ESRGAN 超分 → 落盘。

历史上这条链路走云端 provider（提示词超分）；现在与「画质修复」同一本机链路
（`localmodels.run_esrgan`），区别只在**不做 GFPGAN 人脸修复**：
  - 画质增强 = 纯超分放大 + 去噪（风景 / 静物 / 通用）；
  - 画质修复 = 超分 + 人脸修复（老照片 / 人脸受损）。
纯本机、零付费、照片不出本机。模型缺失时明确报错，不假跑。

性能纪律与 `workflows/restore.py` 同一口径：超大图先按长边 2000 缩，
tile=256 是 DML 最优单块尺寸（实测 512 反而更慢），不要动。
"""
from __future__ import annotations

import os

from PIL import Image

from .. import localmodels
from .base import filter_inputs, read_image, save_image, stem_of
from .batch import Outcome, run_batch

# 控制 4× 输出体积：超大图先按长边缩到这个上限，避免几百 MP 的内存峰值
_MAX_INPUT_LONG = 2000

_ONNX_HEAVY = True


def _limit(img: Image.Image) -> tuple[Image.Image, bool]:
    w, h = img.size
    m = max(w, h)
    if m <= _MAX_INPUT_LONG:
        return img, False
    s = _MAX_INPUT_LONG / float(m)
    return img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS), True


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    scale = int(opts.get("scale", 2) or 2)
    if scale not in (2, 4):
        scale = 2

    if not localmodels.esrgan_ready():
        raise FileNotFoundError(
            "超分模型未就绪（real_esrgan_x4.onnx），请在创意工坊「本地模型」里下载后重试"
        )

    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)

    def process(index: int, p: str) -> Outcome:
        w: list[str] = []
        img = read_image(p)
        small, capped = _limit(img)
        if capped:
            w.append(
                f"{os.path.basename(p)}: 原图较大，已按长边 {_MAX_INPUT_LONG}px 处理后放大"
            )

        up = localmodels.run_esrgan(small, tile=256, cancel=cancel)
        if scale == 2:
            up = up.resize((small.width * 2, small.height * 2), Image.LANCZOS)

        out = save_image(up, out_dir, f"{stem_of(p)}_超分", ".jpg", 95)
        return Outcome(outputs=[out], warnings=w)

    res = run_batch(
        req.paths,
        process,
        progress,
        cancel,
        stage="超分处理中",
        onnx_heavy=_ONNX_HEAVY,
        initial_warnings=warnings,
    )

    progress(100, "完成", f"{len(res.outputs)} 张")
    return {
        "outputs": res.outputs,
        "warnings": res.warnings,
        "meta": {"scale": scale},
    }
