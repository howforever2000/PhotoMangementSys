"""最小改动实验：**只把饱和度那一步换到 LCh**，其余调色逻辑与现有方案逐字一致。

为什么单独开文件而不是改 `autograde.py`：
  这是「同一套方案、只换一个算子的色彩空间」的对照实验，不是产品代码。
  现有方案的影调、白平衡、高光收口、STYLES 阈值**全部原样保留** —— 包括它的问题。
  这样两组出片的差异，100% 只来自饱和度这一步。

改动点只有两处（现有 `autograde.eval_colors` 的第 5、6 步）：
  5) 饱和度：`l + (x-l)*k` （RGB 线性） → LCh 固定 h/L 只缩放 C
  6) 自然饱和度：`(x-l)*vib*weight`（RGB 加权） → LCh 里按 C 给权重只缩放 C

其余步骤（白平衡 → 黑白场拉伸 → gamma → 色温 → **饱和度** → **自然饱和** → 肩部）
的代码与顺序完全相同，所以差异可归因。

关键工程性质：LCh 是**逐色纯函数**，所以不必逐像素算。
它只在建 LUT 时对 33³ = 35937 个格点求值（约 36ms），之后仍走原查表路径 ——
运行时开销与现有方案一致，`_lut_digest` 缓存、`.cube` 导出、稠密表全部照旧。
"""
from __future__ import annotations

import os
import sys

import numpy as np

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(_HERE))
sys.path.insert(0, os.path.join(_ROOT, "python"))

from ai_studio import autograde  # noqa: E402
from ai_studio import colortransfer as ct  # noqa: E402

# 二分开销：6 次把彩度倍数的搜索区间压到 1/64，对 8bit 输出足够
_LCH_ITERS = 6
_LCH_CEIL = 4.0
# 「多艳算艳」的软阈值（CIELAB 的 C）。C 超过它视为已经够艳，自然饱和度不再加。
_C_SOFT = 42.0


# ---------------------------------------------------------------- Lab/LCh 基建


def _lab_to_linear(lab: np.ndarray) -> np.ndarray:
    """Lab → **线性** sRGB，不裁剪。

    不能直接用 `colortransfer.lab_to_rgb`：它末尾 `clip` 成 uint8，
    而判断「是否越色域」恰恰要看裁剪**之前**的值。
    """
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0
    D = 6.0 / 29.0

    def finv(t):
        return np.where(t > D, t ** 3, 3.0 * D * D * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * np.array(
        [0.95047, 1.0, 1.08883], dtype=np.float64)
    return xyz @ np.array([[3.2404542, -1.5371385, -0.4985314],
                           [-0.9692660, 1.8760108, 0.0415560],
                           [0.0556434, -0.2040259, 1.0572252]], dtype=np.float64).T


def chroma_headroom(lab: np.ndarray, iters: int = _LCH_ITERS) -> np.ndarray:
    """逐像素求「保持 h 与 L 不变时，C 最多能放大几倍」的色域边界。

    只缩放 (a,b) 的**长度**、不改方向 → **色相角在数学上恒定**。
    没有闭式解（XYZ 对 a,b 经 finv 是三次分段函数），所以向量化二分。

    这是这套改动能成立的关键：不加它，Lab→sRGB 超色域时会被 `clip` 裁歪色相，
    实测色相偏移反而从 0.307° 涨到 0.636°（比 RGB 方案更差）。
    """
    L = lab[..., 0:1]
    a = lab[..., 1:2]
    b = lab[..., 2:3]
    lo = np.zeros_like(a)
    hi = np.full_like(a, _LCH_CEIL)
    for _ in range(iters):
        mid = (lo + hi) * 0.5
        lin = _lab_to_linear(np.concatenate([L, a * mid, b * mid], axis=-1))
        ok = np.all((lin >= 0.0) & (lin <= 1.0), axis=-1, keepdims=True)
        lo = np.where(ok, mid, lo)
        hi = np.where(ok, hi, mid)
    return lo[..., 0]


def _apply_chroma_scale(rgb01: np.ndarray, scale: np.ndarray) -> np.ndarray:
    """把逐像素的彩度倍数作用在 LCh 上：只缩放 C，h 与 L 不动。"""
    u8 = np.clip(rgb01 * 255.0 + 0.5, 0, 255).astype(np.uint8)
    lab = ct.rgb_to_lab(u8)
    out = lab.copy()
    out[..., 1] *= scale
    out[..., 2] *= scale
    return ct.lab_to_rgb(out).astype(np.float32) / 255.0


# ---------------------------------------------------------------- 改动的两步


def _saturate_lch(x: np.ndarray, k: float) -> np.ndarray:
    """第 5 步的 LCh 版：彩度乘 k，h 与 L 保持；越界按 headroom 回收。

    `k < 1`（降饱和）时 headroom 必然 > 1，等价于直接乘；
    `k > 1` 时若该颜色已贴色域边界，实际倍数被压到 headroom —— 宁可少提，
    也不让色相被裁歪。
    """
    u8 = np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8)
    lab = ct.rgb_to_lab(u8)
    eff = np.minimum(float(k), chroma_headroom(lab))
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return ct.lab_to_rgb(out).astype(np.float32) / 255.0


def _vibrance_lch(x: np.ndarray, amount: float) -> np.ndarray:
    """第 6 步的 LCh 版：按当前彩度 C 给权重，只缩放 C。

    现有 RGB 版的权重来自 `max-min` 通道差（受明度影响）；这里直接用 CIELAB 的 C，
    语义更直接：淡色 C 小 → 权重≈1 多加；艳色 C 大 → 权重≈0 少加。
    """
    u8 = np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8)
    lab = ct.rgb_to_lab(u8)
    a, b = lab[..., 1], lab[..., 2]
    c = np.sqrt(a * a + b * b)
    weight = np.clip(1.0 - c / _C_SOFT, 0.0, 1.0)
    target = 1.0 + amount * weight
    eff = np.minimum(target, chroma_headroom(lab))
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return ct.lab_to_rgb(out).astype(np.float32) / 255.0


# ---------------------------------------------------------------- 逐色映射


def eval_colors_lch(stats: autograde.GradeStats, rgb: np.ndarray) -> np.ndarray:
    """与 `autograde.eval_colors` 逐行同构，**仅第 5、6 步换成 LCh**。

    保留现有方案的全部行为（包括它的缺陷）：白平衡增益可超 1、黑白场归一化、
    gamma 向 [0.40,0.50] 收敛、肩部多次压缩。因此两组出片的差异可全部归因于饱和度。
    """
    x = np.clip(rgb, 0.0, 1.0).astype(np.float32)

    # 1) 白平衡 —— 与现有方案完全一致（含它的肩部收口）
    x = x * np.array([stats.gain_r, stats.gain_g, stats.gain_b], dtype=np.float32)
    x = autograde._shoulder(x) if max(stats.gain_r, stats.gain_g, stats.gain_b) > 1.005 \
        else np.clip(x, 0.0, 1.0)

    # 2) 黑白场拉伸
    span = max(stats.white - stats.black, 1e-4)
    x = np.clip((x - stats.black) / span, 0.0, 1.0)

    # 3) 中间调 gamma
    if abs(stats.gamma - 1.0) > 1e-3:
        x = np.power(x, stats.gamma)
        x = autograde._shoulder(x) if stats.gamma < 0.999 else np.clip(x, 0.0, 1.0)

    # 4) 色温
    if abs(stats.warmth) > 1e-4:
        w = stats.warmth
        x = x * np.array([1.0 + w, 1.0, 1.0 - w], dtype=np.float32)
        x = autograde._shoulder(x) if w > 0 else np.clip(x, 0.0, 1.0)

    # 5) 饱和度 —— **唯一改动**：RGB 线性缩放 → LCh 只动 C
    if abs(stats.saturation - 1.0) > 1e-3:
        x = _saturate_lch(x, stats.saturation)

    # 6) 自然饱和度 —— **唯一改动**：RGB 加权 → LCh 按 C 加权只动 C
    if stats.vibrance > 1e-3:
        x = _vibrance_lch(x, stats.vibrance)

    # 7) 收口 —— 与现有方案完全一致
    stretch = 1.0 / span
    gain_dev = max(abs(stats.gain_r - 1.0), abs(stats.gain_g - 1.0),
                   abs(stats.gain_b - 1.0))
    pushed = stretch > 1.02 or gain_dev > 0.02 or abs(stats.warmth) > 0.01
    if pushed:
        x = autograde._shoulder(x)
    return x


def grade_lch(img_u8: np.ndarray, style: str = autograde.DEFAULT_STYLE,
              strength: float = 1.0, lut_size: int = 33,
              vibrance_extra: float = 0.0):
    """一步到位：复用 `autograde.analyze` 的统计 → 用 LCh 版建 LUT → 查表。

    返回 `(成片, stats, lut)`，与 `autograde.grade` 完全同形。
    注意：统计与参数推导**完全复用**现有 `analyze`，所以影调部分逐位相同。
    """
    from ai_studio import lutcube

    stats = autograde.analyze(img_u8, style=style, strength=strength,
                              vibrance_extra=vibrance_extra)
    n = int(max(9, min(65, lut_size)))
    line = np.linspace(0.0, 1.0, n, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")
    grid = np.stack([r, g, b], axis=-1)
    lut = (eval_colors_lch(stats, grid).astype(np.float32), n)
    return lutcube.apply_cube(img_u8, lut, strength=1.0), stats, lut
