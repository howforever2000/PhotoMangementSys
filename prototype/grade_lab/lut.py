"""加载 Adobe .cube 3D LUT 并做三线性插值套用（纯 numpy，无新依赖）。

.cube 数据顺序遵循 Adobe 规范：红色(R)变化最快，其次绿(G)，再次蓝(B)，
即扁平索引 k = r + N*g + N^2*b，reshape 后数组轴序为 [b, g, r]。
"""
import numpy as np


def load_cube(path: str):
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
            if head.startswith(("TITLE", "DOMAIN_MIN", "DOMAIN_MAX", "LUT_1D_SIZE", "LUT_3D_DATA")):
                continue
            parts = line.split()
            if len(parts) == 3:
                try:
                    rows.append((float(parts[0]), float(parts[1]), float(parts[2])))
                except ValueError:
                    pass
    data = np.asarray(rows, dtype=np.float32)
    n = size or int(round(len(data) ** (1.0 / 3.0)))
    if data.shape[0] != n ** 3:
        raise ValueError(f"LUT 数据量 {data.shape[0]} 与尺寸 {n}^3 不符: {path}")
    return data.reshape(n, n, n, 3), n


def apply_cube(img_u8: np.ndarray, path_or_lut, strength: float = 1.0) -> np.ndarray:
    """把 sRGB 图片(uint8 HxWx3)套用 LUT，返回 uint8。"""
    if isinstance(path_or_lut, str):
        lut, n = load_cube(path_or_lut)
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

    def corner(bi, gi, ri):
        return lut[bi, gi, ri]

    c000 = corner(b0, g0, r0)
    c001 = corner(b0, g0, r1)
    c010 = corner(b0, g1, r0)
    c011 = corner(b0, g1, r1)
    c100 = corner(b1, g0, r0)
    c101 = corner(b1, g0, r1)
    c110 = corner(b1, g1, r0)
    c111 = corner(b1, g1, r1)

    c00 = c000 * (1 - fr) + c001 * fr
    c01 = c010 * (1 - fr) + c011 * fr
    c10 = c100 * (1 - fr) + c101 * fr
    c11 = c110 * (1 - fr) + c111 * fr
    c0 = c00 * (1 - fg) + c01 * fg
    c1 = c10 * (1 - fg) + c11 * fg
    out = c0 * (1 - fb) + c1 * fb

    if strength < 1.0:
        out = src * (1.0 - strength) + out * strength
    return np.clip(out * 255.0 + 0.5, 0, 255).astype(np.uint8)
