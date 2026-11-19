"""饱和度处理的三条路线对照（实验模块，不改 v2 主文件）。

为什么单独开一个文件：这是「同一件事的三种做法」的可比实验，不是产品代码。
验证通过后再决定是否把结论并入 `grade_tone_v2.py`。

三条路线：
  A. RGB 线性缩放（v2 现状）：`x = luma + (x - luma) * k`
     问题：通道硬裁 → 明度漂移；luma 与 Lab-L 不同源 → 感知明度漂移。
  B. Lab 直接缩放 a/b（上一轮量化的"朴素版"）
     问题：`lab_to_rgb` 末尾硬裁 → 超色域时色相被裁歪（实测 0.659° > RGB 的 0.249°）。
  C. LCh 固定 h/L，只缩放 C，并做**色域感知的 C 回收**（本文件主推）
     做法：逐像素二分求出「当前颜色在 sRGB 色域内可放大的最大倍数」，
     目标缩放超过它就回退到边界值 —— 色相角严格不变，且**仍是逐色纯函数**。

关键性质（决定能不能并进产品）：C 只依赖单个输入颜色，没有任何空间运算，
因此可以照原样预计算成 33³ LUT，`_lut_digest` 缓存与 `.cube` 导出全部继续可用。
"""
from __future__ import annotations

import os
import sys

import numpy as np

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(_HERE))
sys.path.insert(0, os.path.join(_ROOT, "python"))

from ai_studio import colortransfer as ct  # noqa: E402

# 二分迭代次数。8 次把区间压到 1/256，对 C 的精度已远超 8bit 输出需要；
# 每多一次就是一遍全图数组运算，不值得。
_BISECT_ITERS = 8
# 色域搜索上界：允许把彩度放大到 4 倍（远超任何合理饱和度档位），
# 只为让「当前颜色离边界还有多远」可被二分准确表达。
_GAMUT_CEIL = 4.0


def lab_to_linear(lab: np.ndarray) -> np.ndarray:
    """Lab → **线性** sRGB，返回未裁剪的浮点值（可能 <0 或 >1）。

    不能直接复用 `colortransfer.lab_to_rgb`：它末尾 `np.clip` 成 uint8，
    而判断「是否在色域内」恰恰需要看裁剪**之前**的值。
    """
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0

    def finv(t):
        return np.where(t > ct._D, t ** 3, 3 * ct._D * ct._D * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * ct._WHITE
    return xyz @ ct._MINV.T


def chroma_headroom(lab: np.ndarray, iters: int = _BISECT_ITERS) -> np.ndarray:
    """逐像素求「保持色相角与明度不变时，彩度最多能放大到几倍」的边界。

    做法：固定 `L` 与 `(a,b)` 的方向，只缩放长度 s，二分找最大的 s 使
    线性 sRGB 三通道都落在 [0,1]。因为缩放只改长度不改方向，**色相角恒定**。

    为什么不用解析解：`finv` 是三次分段函数，XYZ 对 (a,b) 不是线性的，
    没有闭式解。向量化二分 8 次即可，且省掉一个 scipy 依赖。
    """
    L = lab[..., 0:1]
    a = lab[..., 1:2]
    b = lab[..., 2:3]
    lo = np.zeros_like(a)
    hi = np.full_like(a, _GAMUT_CEIL)
    for _ in range(iters):
        mid = (lo + hi) * 0.5
        lin = lab_to_linear(np.concatenate([L, a * mid, b * mid], axis=-1))
        ok = np.all((lin >= 0.0) & (lin <= 1.0), axis=-1, keepdims=True)
        lo = np.where(ok, mid, lo)
        hi = np.where(ok, hi, mid)
    return lo[..., 0]


# ---------------------------------------------------------------- 三条路线


def sat_rgb(rgb_u8: np.ndarray, k: float) -> np.ndarray:
    """A：RGB 线性缩放（v2 现状）。"""
    x = rgb_u8.astype(np.float32) / 255.0
    l = x[..., 0] * 0.2126 + x[..., 1] * 0.7152 + x[..., 2] * 0.0722
    y = l[..., None] + (x - l[..., None]) * k
    return np.clip(y * 255.0 + 0.5, 0, 255).astype(np.uint8)


def sat_lab_naive(rgb_u8: np.ndarray, k: float) -> np.ndarray:
    """B：Lab 直接缩放 a/b（朴素版，硬裁）。"""
    lab = ct.rgb_to_lab(rgb_u8)
    out = lab.copy()
    out[..., 1] *= k
    out[..., 2] *= k
    return ct.lab_to_rgb(out)


def sat_lch(rgb_u8: np.ndarray, k: float) -> np.ndarray:
    """C：LCh 固定 h/L 只缩放 C，并做色域感知的 C 回收（本文件主推）。

    `eff = min(k, headroom)`：
      - 降饱和（k<1）时 headroom 必然够用，eff=k，行为与朴素版一致；
      - 提饱和（k>1）时若当前颜色已贴色域边界，eff 被压到边界值，
        宁可「少提一点」也不让色相被裁歪。
    """
    lab = ct.rgb_to_lab(rgb_u8)
    head = chroma_headroom(lab)
    eff = np.minimum(float(k), head)
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return ct.lab_to_rgb(out)


ROUTES = (
    ("rgb", "A. RGB 线性缩放（现状）", sat_rgb),
    ("lab", "B. Lab 直接缩放 a/b", sat_lab_naive),
    ("lch", "C. LCh 恒色相 + 色域回收", sat_lch),
)


# ---------------------------------------------------------------- 度量


def hue_and_sat(rgb_u8: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """HSV 色相角（度）与饱和度代理，用于跨方案比较「色相是否被推歪」。"""
    y = rgb_u8.astype(np.float32) / 255.0
    mx, mn = y.max(axis=-1), y.min(axis=-1)
    d = mx - mn
    r, g, b = y[..., 0], y[..., 1], y[..., 2]
    h = np.zeros_like(mx)
    m = d > 1e-6
    i = (mx == r) & m
    h[i] = ((g - b)[i] / d[i]) % 6
    i = (mx == g) & m
    h[i] = ((b - r)[i] / d[i]) + 2
    i = (mx == b) & m
    h[i] = ((r - g)[i] / d[i]) + 4
    return (h * 60.0) % 360.0, np.where(mx > 1e-6, d / np.maximum(mx, 1e-6), 0.0)


def measure(src: np.ndarray, out: np.ndarray) -> dict:
    """一张图一组改动的四项读数。"""
    l0 = ct.rgb_to_lab(src)[..., 0]
    l1 = ct.rgb_to_lab(out)[..., 0]
    h0, s0 = hue_and_sat(src)
    h1, s1 = hue_and_sat(out)
    m = s0 > 0.12                       # 只统计有意义的彩色像素
    dh = ((h1[m] - h0[m] + 180.0) % 360.0) - 180.0

    lin = lab_to_linear(ct.rgb_to_lab(out))
    oog = float((np.any((lin < -1e-4) | (lin > 1 + 1e-4), axis=-1)).mean())

    return {
        "dL": round(float(np.abs(l1 - l0).mean()), 4),
        "dL_signed": round(float((l1 - l0).mean()), 4),
        "dHue": round(float(np.abs(dh).mean()), 3),
        "dSat": round(float((s1[m] - s0[m]).mean()), 4),
        "outOfGamut": round(oog * 100, 3),
    }


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    from grade_tone_eval import load_rgb

    d = r"D:\YUAN HAO\Pictures\img\TEST"
    for name in ("IMG_20260404_153242.jpg", "标准色卡_色值表.png", "色轮_标准_color-wheel.jpg"):
        p = os.path.join(d, name)
        if not os.path.isfile(p):
            continue
        src = load_rgb(p, 720)
        print("=" * 78)
        print(name)
        print("  %-28s %8s %9s %9s %8s %9s" % ("方案", "|ΔL|", "ΔL带符号", "|ΔHue|°", "ΔSat", "超色域%"))
        for _k, label, fn in ROUTES:
            m = measure(src, fn(src, 1.12))
            print("  %-28s %8.3f %9.4f %9.3f %8.4f %9.3f" % (
                label, m["dL"], m["dL_signed"], m["dHue"], m["dSat"], m["outOfGamut"]))
