"""3D LUT（.cube）加载、三线性套用与轻调色效果（纯 numpy / Pillow，无新依赖）。

只做「颜色查表 + 胶片辅助效果」这一件事：
  - load_cube      解析 Adobe .cube（3D）
  - apply_cube     三线性插值套用，支持 strength 与原图混合
  - apply_grain    叠加胶片颗粒
  - apply_vignette 叠加暗角

.cube 数据顺序遵循 Adobe 规范：红色(R)变化最快，其次绿(G)，再次蓝(B)，
即扁平索引 k = r + N*g + N^2*b，reshape 后数组轴序为 [b, g, r]。
本模块不依赖 cv2，可被 ai_studio 各工作流复用。
"""
from __future__ import annotations

import os
from typing import Tuple

import numpy as np

# 3D LUT 立方体缓存：路径 -> (lut, size)，避免同一批照片反复解析同一个 .cube
_CACHE: dict[str, Tuple[np.ndarray, int]] = {}


def load_cube(path: str) -> Tuple[np.ndarray, int]:
    """解析 .cube，返回 (lut[ n,n,n,3 ] 轴序 [b,g,r], n)。"""
    size = None
    rows = []
    with open(path, "r", errors="ignore") as fh:
        for raw in fh:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            head = line.upper()
            if head.startswith("LUT_3D_SIZE"):
                size = int(line.split()[1])
                continue
            if head.startswith(
                ("TITLE", "DOMAIN_MIN", "DOMAIN_MAX", "LUT_1D_SIZE", "LUT_3D_DATA")
            ):
                continue
            parts = line.split()
            if len(parts) == 3:
                try:
                    rows.append((float(parts[0]), float(parts[1]), float(parts[2])))
                except ValueError:
                    pass
    data = np.asarray(rows, dtype=np.float32)
    if data.ndim != 2 or data.shape[0] == 0:
        raise ValueError(f"LUT 内容为空或格式不正确: {path}")
    n = size or int(round(len(data) ** (1.0 / 3.0)))
    if data.shape[0] != n ** 3:
        raise ValueError(f"LUT 数据量 {data.shape[0]} 与尺寸 {n}^3 不符: {path}")
    return data.reshape(n, n, n, 3), n


def get_cube(path: str) -> Tuple[np.ndarray, int]:
    """带缓存的 load_cube。"""
    lut = _CACHE.get(path)
    if lut is None:
        lut = load_cube(path)
        _CACHE[path] = lut
    return lut


def apply_cube(img_u8: np.ndarray, path_or_lut, strength: float = 1.0) -> np.ndarray:
    """把 sRGB 图片(uint8 HxWx3)套用 LUT，返回 uint8。strength∈[0,1] 与原图混合。"""
    if isinstance(path_or_lut, str):
        lut, n = get_cube(path_or_lut)
    else:
        lut, n = path_or_lut
    src = img_u8.astype(np.float32) / 255.0
    r = src[..., 0] * (n - 1)
    g = src[..., 1] * (n - 1)
    b = src[..., 2] * (n - 1)
    r0 = np.clip(np.floor(r), 0, n - 1).astype(np.int32)
    g0 = np.clip(np.floor(g), 0, n - 1).astype(np.int32)
    b0 = np.clip(np.floor(b), 0, n - 1).astype(np.int32)
    r1 = np.minimum(r0 + 1, n - 1)
    g1 = np.minimum(g0 + 1, n - 1)
    b1 = np.minimum(b0 + 1, n - 1)
    fr = (r - r0)[..., None]
    fg = (g - g0)[..., None]
    fb = (b - b0)[..., None]

    c000 = lut[b0, g0, r0]
    c001 = lut[b0, g0, r1]
    c010 = lut[b0, g1, r0]
    c011 = lut[b0, g1, r1]
    c100 = lut[b1, g0, r0]
    c101 = lut[b1, g0, r1]
    c110 = lut[b1, g1, r0]
    c111 = lut[b1, g1, r1]

    c00 = c000 * (1 - fr) + c001 * fr
    c01 = c010 * (1 - fr) + c011 * fr
    c10 = c100 * (1 - fr) + c101 * fr
    c11 = c110 * (1 - fr) + c111 * fr
    c0 = c00 * (1 - fg) + c01 * fg
    c1 = c10 * (1 - fg) + c11 * fg
    out = c0 * (1 - fb) + c1 * fb

    strength = float(max(0.0, min(1.0, strength)))
    if strength < 1.0:
        out = src * (1.0 - strength) + out * strength
    return np.clip(out * 255.0 + 0.5, 0, 255).astype(np.uint8)


def apply_grain(img_u8: np.ndarray, amount: float, seed: int | None = None) -> np.ndarray:
    """叠加胶片颗粒：amount∈[0,1]，0 表示不处理。"""
    amount = float(max(0.0, min(1.0, amount)))
    if amount <= 0:
        return img_u8
    rng = np.random.default_rng(seed)
    noise = rng.normal(0.0, 18.0 * amount, img_u8.shape)
    out = img_u8.astype(np.float32) + noise
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def apply_vignette(img_u8: np.ndarray, amount: float) -> np.ndarray:
    """叠加暗角：amount∈[0,1]，越大四角越暗；中心不变。"""
    amount = float(max(0.0, min(1.0, amount)))
    if amount <= 0:
        return img_u8
    h, w = img_u8.shape[:2]
    ys = np.linspace(-1.0, 1.0, h, dtype=np.float32)[:, None]
    xs = np.linspace(-1.0, 1.0, w, dtype=np.float32)[None, :]
    d = np.sqrt(xs * xs + ys * ys) / np.sqrt(2.0)  # 0(中心)~1(角)
    mask = 1.0 - amount * np.clip((d - 0.35) / 0.65, 0.0, 1.0) ** 1.4
    out = img_u8.astype(np.float32) * mask[..., None]
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def lut_path(luts_dir: str, file_name: str) -> str:
    return os.path.join(luts_dir, file_name)
