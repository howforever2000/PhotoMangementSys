"""眼睛提亮（FEAT-210）：68 点眼周多边形内抬明度、轻微去黄。

**只做一件事**：给定 68 点与强度，把「眼睛开口内、低彩度的像素」（巩膜）提亮，
并给虹膜留一点固定的「眼神」额度，让眼睛更清亮。它不做几何变换、不找脸、不读盘
—— 68 点由调用方（`ops_local`）给。

## 为什么权重按彩度分配额度

眼白（巩膜）彩度低，虹膜/眼线/睫毛彩度高。巩膜拿 `MAX_LIFT` 的大额度，虹膜只拿
固定的 `IRIS_LIFT`（3 L*）—— 全块均匀提亮会让虹膜发灰（像白内障），只提低彩度
又几乎看不出来（实测一张人像里「低彩度且够亮」的巩膜像素只有 29 个）。

## 为什么在形变**之前**提亮

调用方先提亮、再液化：液化是对同一张图重采样，提亮结果会跟着眼睛一起被放大，
掩码与像素始终对得上。反过来（先液化后提亮）68 点已经过时，眼睑边缘会漏色。
"""
from __future__ import annotations

import numpy as np
from PIL import Image, ImageDraw

from . import colortransfer, skinretouch

# amount=1.0 时的最大明度提升（L*）。默认档 0.10 → 巩膜实测约 +1.2 L*
MAX_LIFT = 30.0
# 巩膜彩度上界：高于它的像素（虹膜/眼线）权重趋 0；眼白偏黄/带血丝仍在此带内
CHROMA_SOFT = 16.0
# 虹膜/睫毛的「眼神」保底提亮（amount=1.0 时的 L*）。固定且远小于巩膜：
# 巩膜靠彩度权重吃大额度，虹膜只提一点点 —— 提多了会发灰。
IRIS_LIFT = 3.0
# amount=1.0 时相对彩度削减比例（去黄）：只作用在低彩度像素上
CHROMA_CUT = 0.30
# 多边形向质心收缩比例：羽化会向外扩散，先收一点，避免把眼睑也提亮
SHRINK = 0.92
# 羽化半径 = 眼高的这个比例（挂眼睛尺寸，不挂图幅）
FEATHER_RATIO = 0.15
# 眼睛短边小于此像素不处理（68 点在极小脸上不可信）
MIN_EYE_PX = 6

_EYE_SLICES = (slice(36, 42), slice(42, 48))


def eye_rect(points: np.ndarray) -> tuple[int, int, int, int] | None:
    """两只眼睛的外接矩形（已按收缩后的多边形算），退化时返回 None。"""
    polygons = _eye_polygons(points)
    if not polygons:
        return None
    xs = np.concatenate([p[:, 0] for p in polygons])
    ys = np.concatenate([p[:, 1] for p in polygons])
    if xs.max() - xs.min() < MIN_EYE_PX or ys.max() - ys.min() < MIN_EYE_PX:
        return None
    x1, y1 = int(np.floor(xs.min())), int(np.floor(ys.min()))
    x2, y2 = int(np.ceil(xs.max())) + 1, int(np.ceil(ys.max())) + 1
    return x1, y1, x2, y2


def _eye_polygons(points: np.ndarray) -> list[np.ndarray]:
    pts = np.asarray(points, dtype=np.float64)
    if pts.ndim != 2 or pts.shape[0] < 48:
        return []
    out = []
    for sl in _EYE_SLICES:
        poly = pts[sl, :2]
        center = poly.mean(axis=0)
        out.append(center + (poly - center) * SHRINK)
    return out


def _eye_alpha(points: np.ndarray, shape: tuple[int, int]) -> np.ndarray:
    """眼周 alpha（0~1）：多边形填充 + 羽化。`points` 已平移到 crop 坐标。"""
    h, w = shape
    canvas = Image.new("L", (w, h), 0)
    draw = ImageDraw.Draw(canvas)
    for poly in _eye_polygons(points):
        draw.polygon([(float(x), float(y)) for x, y in poly], fill=255)
    mask = np.asarray(canvas, dtype=np.float32) / 255.0
    if mask.max() <= 0:
        return mask
    ys = np.concatenate([p[:, 1] for p in _eye_polygons(points)])
    eye_h = float(max(ys.max() - ys.min(), 1.0))
    radius = max(1.5, FEATHER_RATIO * eye_h)
    return skinretouch._feather(mask, int(round(radius)))


def brighten_eyes(img: Image.Image, points: np.ndarray, amount: float) -> Image.Image:
    """眼周提亮。`amount` 取值 0~1（默认档 0.1）。amount≈0 / 眼睛过小 → 原样返回。"""
    if amount <= 1e-3:
        return img
    rect = eye_rect(points)
    if rect is None:
        return img
    x1, y1, x2, y2 = rect
    w, h = img.size
    x1, y1 = max(0, x1), max(0, y1)
    x2, y2 = min(w, x2), min(h, y2)
    if x2 - x1 < MIN_EYE_PX or y2 - y1 < MIN_EYE_PX:
        return img

    crop = np.asarray(img.crop((x1, y1, x2, y2)).convert("RGB"), dtype=np.uint8)
    alpha = _eye_alpha(np.asarray(points, dtype=np.float64) - (x1, y1), crop.shape[:2])
    if not alpha.any():
        return img

    lab = colortransfer.rgb_to_lab(crop).astype(np.float32)
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    C = np.hypot(a, b)
    # 巩膜权重：彩度越低越提；虹膜/眼线只吃固定的「眼神」额度（提多了发灰）
    w_c = np.clip((CHROMA_SOFT - C) / CHROMA_SOFT, 0.0, 1.0)
    lift = amount * (MAX_LIFT * w_c + IRIS_LIFT * (1.0 - w_c))
    # 高光保护：最多吃掉剩余余量的 60%，泪膜高光不会被推到截断
    lift = np.minimum(lift, np.maximum(100.0 - L, 0.0) * 0.6)
    L2 = np.clip(L + lift, 0.0, 100.0)
    # 轻微去黄：只削减低彩度像素的彩度（sclera 的血丝/泛黄），虹膜不动
    scale = 1.0 - CHROMA_CUT * amount * w_c
    new_lab = np.stack([L2, a * scale, b * scale], axis=-1)
    new_rgb = colortransfer.lab_to_rgb(new_lab).astype(np.float32)

    blended = crop.astype(np.float32) * (1.0 - alpha[..., None]) + new_rgb * alpha[..., None]
    out = np.clip(blended + 0.5, 0, 255).astype(np.uint8)
    result = img.copy()
    result.paste(Image.fromarray(out), (x1, y1))
    return result


__all__ = ["MAX_LIFT", "CHROMA_SOFT", "eye_rect", "brighten_eyes"]
