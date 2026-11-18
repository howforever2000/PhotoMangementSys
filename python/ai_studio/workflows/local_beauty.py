"""本地美颜（磨皮）工作流：SCRFD 找人脸 → BiSeNet 分区 → 只磨皮肤 → 羽化贴回。

纯本机、零付费。两个模型：`det_10g.onnx`（扫描入库已有，无需下载）与
`face_parsing_bisenet_r34.onnx`（应用内下载，约 93MB）。

设计纪律：
  - 检不到人脸 / 模型缺失 → 记 warning 继续下一张，**绝不整批失败**（与 FEAT-164 一致）；
  - 检测或解析失败时该张原样输出并告警，不产出「半张糊掉」的结果；
  - 逐张的检测框外扩边距，遵循 `local-enhance.md` §2「紧裁人脸检出率低」的既有经验。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from .. import faceparse, localmodels, skinretouch
from .base import check_cancel, filter_inputs, read_image, save_image, stem_of

# 送进解析模型前，在检测框外扩的比例。紧裁人脸会让分割碎裂，
# 留出脸颊/发际/下巴上下文才稳（与 SCRFD 检测的既有坑同源）。
_CONTEXT_MARGIN = 0.30
# 过小的人脸不做处理：解析在 512 上跑，再缩回去细节已不可信，磨了反而糊。
_MIN_FACE_PX = 48


def run(req, progress, cancel) -> dict:
    out_dir = (req.outputDir or "").strip()
    if not out_dir:
        raise ValueError("未指定输出目录")
    if not req.paths:
        raise ValueError("没有可处理的照片")

    opts = req.options or {}
    # 用 `is None` 判缺省，不用 `or`：`or` 会把 0 当缺省吞掉，
    # 导致「磨皮拖到 0 = 关闭」在界面上无效（BUG-2026-1117-001）。
    strength = _opt_float(opts, "strength", 0.45)
    keep_texture = _opt_float(opts, "texture", 0.55)

    if not localmodels.face_det_ready():
        raise FileNotFoundError(
            "人脸检测模型未就绪（det_10g.onnx），请先在「设置 › 模型管理」下载「人脸 · 高精度」"
        )
    if not faceparse.ready():
        raise FileNotFoundError(
            "人脸解析模型未就绪（face_parsing_bisenet_r34.onnx），"
            "请在创意工坊「本地增强模型」里下载后重试"
        )

    outputs: list[str] = []
    warnings: list[str] = []
    req.paths = filter_inputs(list(req.paths), warnings)
    n = len(req.paths)
    faces_total = 0

    for i, p in enumerate(req.paths):
        check_cancel(cancel)
        progress(3 + int(i / n * 92), "美颜处理中", f"{i + 1}/{n}")
        img = read_image(p)
        arr = np.asarray(img, dtype=np.uint8)

        try:
            faces = localmodels.detect_faces(img)
        except Exception as e:  # noqa: BLE001 —— 单张失败不拖垮整批
            warnings.append(f"{os.path.basename(p)}: 人脸检测失败，已跳过（{e}）")
            outputs.append(save_image(img, out_dir, f"{stem_of(p)}_美颜", ".jpg", 95))
            continue

        usable = [f for f in faces if min(f["bbox"][2] - f["bbox"][0],
                                          f["bbox"][3] - f["bbox"][1]) >= _MIN_FACE_PX]
        if not usable:
            warnings.append(f"{os.path.basename(p)}: 未检测到可用人脸，已原样输出")
            outputs.append(save_image(img, out_dir, f"{stem_of(p)}_美颜", ".jpg", 95))
            continue

        result = arr
        for f in usable:
            check_cancel(cancel)
            box = _context_box(f["bbox"], arr.shape[1], arr.shape[0], _CONTEXT_MARGIN)
            if box is None:
                continue
            x1, y1, x2, y2 = box
            crop = Image.fromarray(arr[y1:y2, x1:x2])
            try:
                cls_map = faceparse.predict(crop)
            except Exception as e:  # noqa: BLE001
                warnings.append(f"{os.path.basename(p)}: 人脸解析失败，该脸已跳过（{e}）")
                continue
            mask = faceparse.skin_mask(cls_map)
            if mask.max() <= 0:
                continue
            patch = skinretouch.smooth_skin(
                arr[y1:y2, x1:x2],
                mask,
                strength=strength,
                radius=skinretouch.radius_for_face(f["bbox"][2] - f["bbox"][0]),
                keep_texture=keep_texture,
            )
            # 贴回时用同一张 mask 做 alpha，块边界不出现硬接缝
            result = skinretouch.paste_region(
                result,
                patch,
                box,
                skinretouch._feather(
                    mask,
                    max(1, int(round(skinretouch.radius_for_face(f["bbox"][2] - f["bbox"][0])))),
                ),
            )
            faces_total += 1

        out = save_image(Image.fromarray(result), out_dir, f"{stem_of(p)}_美颜", ".jpg", 95)
        outputs.append(out)

    progress(100, "完成", f"{len(outputs)} 张")
    return {
        "outputs": outputs,
        "warnings": warnings,
        "meta": {
            "strength": round(strength, 4),
            "texture": round(keep_texture, 4),
            "faces": faces_total,
        },
    }


def _opt_float(opts: dict, key: str, default: float) -> float:
    """取浮点选项：只有「缺省 / None / 空串」才用默认值，0 是合法值。"""
    v = opts.get(key)
    if v is None or v == "":
        return float(default)
    return float(v)


def _context_box(bbox, width: int, height: int, margin: float):
    """检测框外扩 `margin` 并夹到图像内；退化框返回 None。"""
    x1, y1, x2, y2 = bbox
    bw, bh = x2 - x1, y2 - y1
    if bw <= 0 or bh <= 0:
        return None
    mx, my = int(bw * margin), int(bh * margin)
    bx1, by1 = max(0, int(x1) - mx), max(0, int(y1) - my)
    bx2, by2 = min(width, int(x2) + mx), min(height, int(y2) + my)
    if bx2 - bx1 < _MIN_FACE_PX or by2 - by1 < _MIN_FACE_PX:
        return None
    return bx1, by1, bx2, by2
