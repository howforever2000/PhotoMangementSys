"""画质修复工作流：Real-ESRGAN 超分 + 可选 GFPGAN 人脸修复，输出 JPG。

纯本机、零付费。模型较大，由用户在创意工坊「本地模型」按链接下载；
缺失时明确报错 / 跳过人脸，不假跑。推理在 localmodels.py。
"""
from __future__ import annotations

import os

from PIL import Image

from .. import localmodels
from .base import check_cancel, read_image, save_image, stem_of

# 控制 4× 输出体积：超大图先按长边缩到这个上限，避免几百 MP 的内存峰值
_MAX_INPUT_LONG = 2000


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
    scale = int(opts.get("scale", 4) or 4)
    if scale not in (2, 4):
        scale = 4
    face = bool(opts.get("face", True))

    if not localmodels.esrgan_ready():
        raise FileNotFoundError(
            "超分模型未就绪（real_esrgan_x4.onnx），请在创意工坊「本地模型」里下载后重试"
        )

    outputs: list[str] = []
    warnings: list[str] = []
    n = len(req.paths)
    for i, p in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "超分处理中", f"{i + 1}/{n}")
        img = read_image(p)
        small, capped = _limit(img)
        if capped:
            warnings.append(
                f"{os.path.basename(p)}: 原图较大，已按长边 {_MAX_INPUT_LONG}px 处理后放大"
            )

        up = localmodels.run_esrgan(small, tile=256, cancel=cancel)
        if scale == 2:
            up = up.resize((small.width * 2, small.height * 2), Image.LANCZOS)

        if face:
            check_cancel(cancel)
            progress(3 + int(i / n * 92), "人脸修复中", f"{i + 1}/{n}")
            try:
                if not localmodels.face_det_ready():
                    warnings.append("人脸检测模型（det_10g.onnx）缺失，跳过人脸修复")
                else:
                    faces = localmodels.detect_faces(small)
                    if not faces:
                        warnings.append(f"{os.path.basename(p)}: 未检测到人脸，已仅做超分")
                    else:
                        ratio = up.width / float(small.width)
                        up = localmodels.restore_faces(up, faces, ratio)
            except FileNotFoundError as e:
                warnings.append(f"人脸修复模型缺失，跳过：{e}")

        out = save_image(up, out_dir, f"{stem_of(p)}_修复", ".jpg", 95)
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {"scale": scale, "face": face},
    }
