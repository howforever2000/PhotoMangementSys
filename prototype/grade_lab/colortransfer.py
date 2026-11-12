"""参考图颜色迁移（纯 numpy）：CIELAB 下的 Reinhard 均值/方差迁移 + 逐通道直方图匹配。

Reinhard et al. 2001 "Color Transfer between Images" 的经典做法；
直方图匹配作为对照。均为无状态纯函数，便于复用/单测。
"""
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


def reinhard_transfer(src_u8: np.ndarray, ref_u8: np.ndarray, l_weight: float = 1.0) -> np.ndarray:
    """把 ref 的 Lab 均值/标准差迁到 src；l_weight<1 时只迁移色度、少动亮度。"""
    src = rgb_to_lab(src_u8)
    ref = rgb_to_lab(ref_u8)
    out = src.copy()
    weights = (l_weight, 1.0, 1.0)
    for i in range(3):
        ms, ss = src[..., i].mean(), src[..., i].std() + 1e-6
        mr, sr = ref[..., i].mean(), ref[..., i].std() + 1e-6
        target = (src[..., i] - ms) * (sr / ss) + mr
        out[..., i] = src[..., i] * (1 - weights[i]) + target * weights[i]
    out[..., 0] = np.clip(out[..., 0], 0.0, 100.0)
    return lab_to_rgb(out)


def histogram_match(src_u8: np.ndarray, ref_u8: np.ndarray) -> np.ndarray:
    """逐 RGB 通道直方图匹配（对照组）。"""
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
    return out
