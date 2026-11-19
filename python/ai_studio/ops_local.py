"""本地原子算子：单图原语 + `fn(img, params, providers)` 形态包装（FEAT-168）。

分层职责：
  - 本模块只处理「一张 PIL.Image」，**不含**读盘 / 落盘 / 进度 / 取消 / 作业编排；
  - `grade_image` / `reshape_image` / `smooth_image` 是 workflows 与 OPS 共用的单图原语，
    避免同一套算法在「单步工具」和「配方链路」里各写一份、参数逐渐漂移；
  - `OPS_LOCAL` 由 `ops.resolve()` 并入总表，`pipeline.execute_plan` 不感知算子在哪。

复用说明：图像处理算法本身全部来自既有纯函数模块（`autograde` / `facewarp` /
`landmark` / `faceparse` / `skinretouch`），本文件只做「PIL.Image 适配 + 多脸循环」。
"""
from __future__ import annotations

from typing import Any, Callable, Dict, Optional

import numpy as np
from PIL import Image

from . import autograde, faceparse, facewarp, landmark, localmodels, skinretouch

Warn = Optional[Callable[[str], None]]

# 与 workflows/local_beauty.py 原有口径一致：检测框外扩比例 / 过小脸阈值
_CONTEXT_MARGIN = 0.30
_MIN_FACE_PX = 48


def _emit(warn: Warn, text: str) -> None:
    if warn is not None:
        warn(text)


def grade_image(
    img: Image.Image,
    style: str = "natural",
    strength: float = 1.0,
    vibrance_extra: float = 0.0,
) -> tuple[Image.Image, Any, Any]:
    """自适应调色：统计 → 逐图 LUT → 查表。返回 (成片, GradeStats, (lut, n))。

    `vibrance_extra` 是配方级自然饱和度微调（默认 0 = 与旧版逐字节一致），
    语义与边界见 `autograde.analyze`。
    """
    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    graded, stats, lut = autograde.grade(
        arr, style=style, strength=strength, vibrance_extra=vibrance_extra
    )
    return Image.fromarray(graded), stats, lut


def reshape_image(
    img: Image.Image,
    slim: float = 0.3,
    eyes: float = 0.15,
    warn: Warn = None,
    stats: Optional[dict] = None,
) -> Image.Image:
    """68 点关键点驱动的瘦脸 + 大眼。单脸失败只跳过该脸，不抛（懒人链路纪律）。"""
    try:
        faces = localmodels.detect_faces(img)
    except Exception as e:  # noqa: BLE001
        _emit(warn, f"人脸检测失败，已跳过形变（{e}）")
        return img
    if not faces:
        _emit(warn, "未检测到人脸，形变步骤原样通过")
        return img

    result = img
    touched = 0
    for face in faces:
        try:
            points, confidence = landmark.detect_68(result, face["bbox"], face.get("kps"))
            if points is None:
                _emit(warn, f"有脸过小（<{landmark.MIN_FACE_PX}px），已跳过该脸")
                continue
            if confidence < landmark.MIN_CONFIDENCE:
                _emit(warn, f"关键点置信度偏低（{confidence:.2f}），已跳过该脸")
                continue
            result = facewarp.reshape(result, points, slim=slim, eyes=eyes)
            touched += 1
        except Exception as e:  # noqa: BLE001
            _emit(warn, f"形变失败，该脸已跳过（{e}）")
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return result


def _context_box(bbox, width: int, height: int, margin: float):
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


def smooth_image(
    img: Image.Image,
    strength: float = 0.45,
    texture: float = 0.55,
    warn: Warn = None,
    stats: Optional[dict] = None,
) -> Image.Image:
    """分区磨皮：SCRFD → BiSeNet → 只压皮肤高频 → 带 alpha 羽化贴回。"""
    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    try:
        faces = localmodels.detect_faces(img)
    except Exception as e:  # noqa: BLE001
        _emit(warn, f"人脸检测失败，已跳过磨皮（{e}）")
        return img

    usable = [
        f for f in faces
        if min(f["bbox"][2] - f["bbox"][0], f["bbox"][3] - f["bbox"][1]) >= _MIN_FACE_PX
    ]
    if not usable:
        _emit(warn, "未检测到可用人脸，磨皮步骤原样通过")
        return img

    result = arr
    touched = 0
    for f in usable:
        box = _context_box(f["bbox"], arr.shape[1], arr.shape[0], _CONTEXT_MARGIN)
        if box is None:
            continue
        x1, y1, x2, y2 = box
        try:
            cls_map = faceparse.predict(Image.fromarray(arr[y1:y2, x1:x2]))
        except Exception as e:  # noqa: BLE001
            _emit(warn, f"人脸解析失败，该脸已跳过（{e}）")
            continue
        mask = faceparse.skin_mask(cls_map)
        if mask.max() <= 0:
            continue
        # 半径挂钩脸宽（不是图幅）：高分辨率原图不再被过度抹平
        r = skinretouch.radius_for_face(f["bbox"][2] - f["bbox"][0])
        patch = skinretouch.smooth_skin(
            arr[y1:y2, x1:x2], mask, strength=strength, radius=r, keep_texture=texture
        )
        result = skinretouch.paste_region(result, patch, box, skinretouch._feather(mask, max(1, int(round(r)))))
        touched += 1
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return Image.fromarray(result)


# ---------------- OPS 形态包装（签名与 ops.py 既有算子一致） ----------------


def op_auto_grade(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    out, _, _ = grade_image(
        img,
        str(params.get("style", "natural")),
        float(params.get("strength", 1.0)),
        float(params.get("vibrance", 0.0) or 0.0),
    )
    return out


def op_face_reshape(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    return reshape_image(
        img,
        float(params.get("slim", 0.3)),
        float(params.get("eyes", 0.15)),
    )


def op_skin_smooth(img: Image.Image, params: Dict[str, Any], providers: Any) -> Image.Image:
    return smooth_image(
        img,
        float(params.get("strength", 0.45)),
        float(params.get("texture", 0.55)),
    )


OPS_LOCAL = {
    "auto_grade": op_auto_grade,
    "face_reshape": op_face_reshape,
    "skin_smooth": op_skin_smooth,
}

__all__ = [
    "grade_image",
    "reshape_image",
    "smooth_image",
    "op_auto_grade",
    "op_face_reshape",
    "op_skin_smooth",
    "OPS_LOCAL",
]
