"""分区磨皮：只对皮肤区做「高频分离」柔化，五官/头发/衣物/背景像素原样保留。

为什么是高频分离而不是简单模糊：
    直接对皮肤做高斯模糊会把毛孔、绒毛、皮肤纹理一并抹掉，出来是「塑料脸」。
    拆成两层后只压高频（噪点/痘印），保留低频（肤色明暗、面部立体感），
    强度旋钮才有连续的「轻→重」手感，而不是「有/没有」两档。

为什么不做整图：
    整图模糊会把头发丝糊成色块、把眼神光抹平。工业界（美图类）都是先分区再处理，
    分区掩码来自 `faceparse.py`，本模块只负责「给定掩码 → 怎么改像素」。
"""
from __future__ import annotations

import numpy as np
from PIL import Image, ImageFilter


def _blur(arr: np.ndarray, radius: float) -> np.ndarray:
    """对 uint8 RGB 做高斯模糊。Pillow 的 C 实现比 numpy 手写快一个量级。"""
    if radius <= 0:
        return arr.astype(np.float32)
    img = Image.fromarray(arr)
    return np.asarray(img.filter(ImageFilter.GaussianBlur(radius)), dtype=np.float32)


def smooth_skin(
    rgb_u8: np.ndarray,
    mask: np.ndarray,
    strength: float = 0.45,
    radius: float = 0.0,
    keep_texture: float = 0.55,
) -> np.ndarray:
    """在 `mask`（0~1，[H,W]）区域内做高频分离磨皮，返回新的 uint8 RGB。

    strength∈[0,1]：磨皮力度（高频被压掉的比例）
    radius：模糊半径，0 表示按**图幅**自动（短边的 ~0.6%）。
        注意：调用方若知道人脸尺寸，应显式传入 `radius_for_face()` 的结果——
        按图幅取半径在 6000px 原图上会到 6px，把皮肤肌理当噪点抹平（BUG-2026-1117-005）。
    keep_texture∈[0,1]：保留的原纹理比例，越高越「有皮肤质感」，越低越平滑
    """
    s = float(max(0.0, min(1.0, strength)))
    if s <= 0.0 or mask.max() <= 0.0:
        return rgb_u8

    h, w = rgb_u8.shape[:2]
    r = float(radius) if radius and radius > 0 else max(1.5, min(h, w) * 0.006)

    src = rgb_u8.astype(np.float32)
    low = _blur(rgb_u8, r)                 # 低频：肤色与明暗
    high = src - low                       # 高频：毛孔 / 噪点 / 瑕疵

    # 只压高频，且保留 keep_texture 比例的原始纹理
    keep = float(max(0.0, min(1.0, keep_texture)))
    smoothed = low + high * (keep + (1.0 - keep) * (1.0 - s))

    # 掩码羽化（否则皮肤与头发/嘴唇的交界会出现一圈硬边）
    alpha = _feather(mask, max(1, int(round(r))))
    alpha = (alpha * s)[..., None]
    out = src * (1.0 - alpha) + smoothed * alpha
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def radius_for_face(face_width_px: float) -> float:
    """按人脸宽度推算磨皮半径（像素），夹在 2~8px。

    为什么挂钩脸宽而不是图幅：磨皮抹掉的是「毛孔级」高频，它的物理尺度跟着脸走。
    同一张脸在 6000px 原图与 1024px 缩略图上，脸宽相差 6 倍，但毛孔的视觉尺度不变。
    按图幅取半径会让高分辨率原图被抹得过度（实测脸部高频保留从 99% 掉到 95%）。
    """
    return float(np.clip(float(face_width_px) * 0.02, 2.0, 8.0))


def _feather(mask: np.ndarray, radius: int) -> np.ndarray:
    """把二值/权重掩码羽化成柔和 alpha，避免贴回时出现可见边界。"""
    m = np.clip(mask, 0.0, 1.0).astype(np.float32)
    img = Image.fromarray((m * 255.0 + 0.5).astype(np.uint8), mode="L")
    img = img.filter(ImageFilter.GaussianBlur(max(1.0, float(radius))))
    return np.asarray(img, dtype=np.float32) / 255.0


def paste_region(
    base_u8: np.ndarray,
    patch_u8: np.ndarray,
    box: tuple[int, int, int, int],
    alpha: np.ndarray | None = None,
) -> np.ndarray:
    """把 `patch` 按 box 贴回 `base`（原地语义，返回新数组）。

    box = (x1, y1, x2, y2)，已裁剪到图像边界内。alpha 为 [h,w] 的 0~1 权重。
    """
    out = base_u8.copy()
    x1, y1, x2, y2 = box
    region = out[y1:y2, x1:x2].astype(np.float32)
    if alpha is None:
        out[y1:y2, x1:x2] = patch_u8
    else:
        a = np.clip(alpha, 0.0, 1.0)[..., None]
        out[y1:y2, x1:x2] = np.clip(
            region * (1.0 - a) + patch_u8.astype(np.float32) * a + 0.5, 0, 255
        ).astype(np.uint8)
    return out
