"""参考图颜色迁移（纯 numpy）：CIELAB 下的 Reinhard 均值/方差迁移 + 逐通道直方图匹配。

Reinhard et al. 2001 "Color Transfer between Images" 的经典做法；直方图匹配作为对照。
均为无状态纯函数，可被 ai_studio 工作流复用；额外提供把一次迁移固化成 .cube 的导出。
"""
from __future__ import annotations

import numpy as np

_M = np.array(
    [
        [0.4124564, 0.3575761, 0.1804375],
        [0.2126729, 0.7151522, 0.0721750],
        [0.0193339, 0.1191920, 0.9503041],
    ],
    dtype=np.float32,
)
_MINV = np.linalg.inv(_M)
_WHITE = np.array([0.95047, 1.0, 1.08883], dtype=np.float32)
_D = 6.0 / 29.0


def _srgb_to_linear(c: np.ndarray) -> np.ndarray:
    c = c / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def _linear_to_srgb(c: np.ndarray) -> np.ndarray:
    c = np.clip(c, 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1.0 / 2.4) - 0.055)


def rgb_to_lab(img_u8: np.ndarray) -> np.ndarray:
    lin = _srgb_to_linear(img_u8.astype(np.float32))
    xyz = (lin @ _M.T) / _WHITE

    def f(t):
        return np.where(t > _D ** 3, np.cbrt(t), t / (3 * _D * _D) + 4.0 / 29.0)

    fx, fy, fz = f(xyz[..., 0]), f(xyz[..., 1]), f(xyz[..., 2])
    L = 116.0 * fy - 16.0
    a = 500.0 * (fx - fy)
    b = 200.0 * (fy - fz)
    return np.stack([L, a, b], axis=-1)


def lab_to_rgb(lab: np.ndarray) -> np.ndarray:
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0

    def finv(t):
        return np.where(t > _D, t ** 3, 3 * _D * _D * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * _WHITE
    lin = xyz @ _MINV.T
    srgb = _linear_to_srgb(lin)
    return np.clip(srgb * 255.0 + 0.5, 0, 255).astype(np.uint8)


def _reinhard_lab(src_lab: np.ndarray, ref_lab: np.ndarray, l_weight: float) -> np.ndarray:
    out = src_lab.copy()
    weights = (l_weight, 1.0, 1.0)
    for i in range(3):
        ms, ss = src_lab[..., i].mean(), src_lab[..., i].std() + 1e-6
        mr, sr = ref_lab[..., i].mean(), ref_lab[..., i].std() + 1e-6
        target = (src_lab[..., i] - ms) * (sr / ss) + mr
        out[..., i] = src_lab[..., i] * (1 - weights[i]) + target * weights[i]
    out[..., 0] = np.clip(out[..., 0], 0.0, 100.0)
    return out


def reinhard_transfer(
    src_u8: np.ndarray,
    ref_u8: np.ndarray,
    l_weight: float = 1.0,
    strength: float = 1.0,
) -> np.ndarray:
    """把 ref 的 Lab 均值/标准差迁到 src；l_weight<1 时只迁移色度、少动亮度。

    strength∈[0,1] 为与原图的最终混合比例（0 = 原图，1 = 完全迁移）。
    """
    src_lab = rgb_to_lab(src_u8)
    ref_lab = rgb_to_lab(ref_u8)
    transferred = lab_to_rgb(_reinhard_lab(src_lab, ref_lab, l_weight))
    strength = float(max(0.0, min(1.0, strength)))
    if strength >= 1.0:
        return transferred
    blended = src_u8.astype(np.float32) * (1.0 - strength) + transferred.astype(np.float32) * strength
    return np.clip(blended + 0.5, 0, 255).astype(np.uint8)


def histogram_match(src_u8: np.ndarray, ref_u8: np.ndarray, strength: float = 1.0) -> np.ndarray:
    """逐 RGB 通道直方图匹配（对照组）。strength 与 reinhard 同义。"""
    out = np.empty_like(src_u8)
    h, w = src_u8.shape[:2]
    for i in range(3):
        s = src_u8[..., i].ravel()
        r_sorted = np.sort(ref_u8[..., i].ravel())
        order = np.argsort(s, kind="stable")
        ranks = np.empty_like(order)
        ranks[order] = np.arange(s.size)
        q = ranks / max(s.size - 1, 1)
        idx = np.clip((q * (r_sorted.size - 1)).astype(np.int32), 0, r_sorted.size - 1)
        out[..., i] = r_sorted[idx].reshape(h, w)
    strength = float(max(0.0, min(1.0, strength)))
    if strength >= 1.0:
        return out
    blended = src_u8.astype(np.float32) * (1.0 - strength) + out.astype(np.float32) * strength
    return np.clip(blended + 0.5, 0, 255).astype(np.uint8)


def reference_stats(ref_u8: np.ndarray) -> dict:
    """提取参考图 Lab 的均值/标准差，供「先描述后套用」或导出 .cube 复用。"""
    lab = rgb_to_lab(ref_u8)
    return {
        "mean": [float(lab[..., i].mean()) for i in range(3)],
        "std": [float(lab[..., i].std() + 1e-6) for i in range(3)],
    }


def build_cube_text(ref_u8: np.ndarray, size: int = 33, l_weight: float = 1.0) -> str:
    """把「参考图 Reinhard 迁移」固化成 .cube 文本：采样 size^3 个 sRGB 色点做迁移。"""
    line = np.linspace(0.0, 1.0, size, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")  # 轴序 [b,g,r]
    grid = np.stack([r, g, b], axis=-1)
    grid_u8 = np.clip(grid * 255.0 + 0.5, 0, 255).astype(np.uint8).reshape(-1, 3)
    # 逐色点迁移（全局统计，等价于对整幅图做同一变换）——按 256K 一批控制内存
    out = reinhard_transfer(grid_u8.reshape(1, -1, 3), ref_u8, l_weight=l_weight, strength=1.0)
    out = out.reshape(-1, 3)
    body = "\n".join(
        f"{v[0] / 255.0:.6f} {v[1] / 255.0:.6f} {v[2] / 255.0:.6f}" for v in out
    )
    return (
        f"TITLE \"reference-transfer\"\nLUT_3D_SIZE {size}\n"
        "DOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n" + body + "\n"
    )
