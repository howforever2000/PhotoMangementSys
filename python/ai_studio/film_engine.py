"""参数化自适应胶片渲染引擎（纯 numpy，不依赖 opencv）。

为什么单独一个模块、且必须纯 numpy：
  1. 打包版 ai-server.exe 的 `excludes` 里有 `cv2`（见 python/ai-studio.spec），
     产品代码里加 `import cv2` 会直接让打包后的胶片模拟不可用；
  2. Lab 用的矩阵常量复用 `colortransfer`（同样是纯 numpy 实现），避免两套色彩口径。

管线（与 lab/adaptive_film 的验证脚本同源，差别只在实现层做了查表化）：
  场景统计 → 白平衡（只补色偏）→ 自动曝光归一 → 光晕 → 胶片曲线（LUT）
  → 分区色相/彩度重建 → 层间串色 → 分色调 → 色域压缩 → 颗粒 → 高光收口

硬约束（来自方案 v2 的验收线）：
  - 高光截断 ≤0.5%：`soft_limit` 把最亮通道压到 0.984 以下，逐通道 clamp 不会产生 254；
  - 同一风格跨场景亮度稳定（p50σ ≤0.02）：曝光归一是部分归一（0.8 次幂）+ 高光上限；
  - 强度可调且连续：`look` 对偏色/色相/彩度/对比做**乘性**放大，1.0 = 原配方。
"""
from __future__ import annotations

import math
from typing import Any, Dict, Tuple

import numpy as np

from .colortransfer import _D, _M, _MINV, _WHITE

LUMA = np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)

# 自动曝光目标中灰：0.190 是 lab 实测出来的（0.235 会把阴天高光一起抬白）
TARGET_MID = 0.190
LOWCON_GAMMA = 0.12

# 曲线查表：sqrt(E) 域 16k 点、逐通道。实测与逐像素幂函数的最大误差 <1e-5，
# 而单张 8MP 的曲线耗时从 2.4s 降到 0.5s 量级。
_CURVE_N = 16384
_CURVE_EMAX = 6.0
_CURVE_UMAX = math.sqrt(_CURVE_EMAX)


# --------------------------------------------------------------------------- sRGB 查表
def _build_srgb_tables(n: int = 65536) -> Tuple[np.ndarray, np.ndarray]:
    """sqrt 域 64k 的 sRGB 正/反变换表（float→float 的高频调用走这里）。

    为什么按 sqrt(x) 采样：sRGB 曲线在 0 附近斜率大，按 x 均匀采样会在暗部丢精度；
    sqrt 域等距采样把相对误差压到 1e-6 量级，且索引只要一次 sqrt + 一次 take。

    两张表都以「自己定义域」的 sqrt 为索引：
      dec：索引 sqrt(显示值) → 线性光；
      enc：索引 sqrt(线性光) → 显示值。
    """
    u = np.linspace(0.0, 1.0, n, dtype=np.float32)
    x = (u * u).astype(np.float32)
    dec = np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)
    enc = np.where(x <= 0.0031308, x * 12.92, 1.055 * np.power(x, 1.0 / 2.4) - 0.055)
    return np.clip(dec, 0.0, 1.0).astype(np.float32), np.clip(enc, 0.0, 1.0).astype(np.float32)


_DEC_TAB, _ENC_TAB = _build_srgb_tables()


def _build_dec8() -> np.ndarray:
    """uint8 → 线性光的 256 项**精确**表（不走 sqrt 近似：8bit 输入只有 256 个值）。"""
    x = np.arange(256, dtype=np.float32) / 255.0
    lin = np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)
    return lin.astype(np.float32)


_DEC8 = _build_dec8()


def _idx(x: np.ndarray) -> np.ndarray:
    u = np.sqrt(np.clip(x, 0.0, 1.0), dtype=np.float32)
    return np.clip(u * (len(_DEC_TAB) - 1), 0, len(_DEC_TAB) - 1).astype(np.uint16)


def srgb_decode(x: np.ndarray) -> np.ndarray:
    return _DEC_TAB[_idx(x)]


def srgb_encode(x: np.ndarray) -> np.ndarray:
    return _ENC_TAB[_idx(x)]


def decode_u8(rgb: np.ndarray) -> np.ndarray:
    """uint8 RGB → 线性光 float32（精确查表）。"""
    return _DEC8[rgb]


# --------------------------------------------------------------------------- 小工具
def luma_of(rgb: np.ndarray) -> np.ndarray:
    return rgb @ LUMA


def smoothstep(x: np.ndarray, a: float, b: float) -> np.ndarray:
    if a == b:
        return np.zeros_like(x) if b < a else np.ones_like(x)
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def _box(a: np.ndarray, r: int, axis: int) -> np.ndarray:
    """沿一个轴的均值模糊（滑窗 + 前缀和，O(n)）。小图上做三次≈高斯。"""
    if r <= 0:
        return a
    width = 2 * r + 1
    pad = [(0, 0)] * a.ndim
    pad[axis] = (r, r)
    p = np.pad(a, pad, mode="edge")
    cs = np.cumsum(p, axis=axis, dtype=np.float32)
    cs_shape = list(cs.shape)
    cs_shape[axis] = 1
    cs0 = np.concatenate([np.zeros(cs_shape, np.float32), cs], axis=axis)
    n = a.shape[axis]
    if axis == 0:
        out = (cs0[width:width + n] - cs0[0:n]) / float(width)
    else:
        out = (cs0[:, width:width + n] - cs0[:, 0:n]) / float(width)
    return out


def blur(a: np.ndarray, sigma: float) -> np.ndarray:
    r = max(1, int(round(sigma * 0.9)))
    for _ in range(3):
        a = _box(a, r, 0)
        a = _box(a, r, 1)
    return a


def _down2(a: np.ndarray) -> np.ndarray:
    """2× 块平均。用 4 个切片相加而不是 reshape+mean：numpy 的 reduce 在 8MP 上
    要 0.44s，切片加法只要 0.12s（省下的是 reduce 的临时缓冲与第二次遍历）。"""
    h, w = a.shape[:2]
    h2, w2 = h // 2, w // 2
    if h2 < 1 or w2 < 1:
        return a
    a = a[: h2 * 2, : w2 * 2]
    out = a[0::2, 0::2] + a[1::2, 0::2]
    out += a[0::2, 1::2]
    out += a[1::2, 1::2]
    out *= 0.25
    return out


def _up2_smooth(a: np.ndarray, out_h: int, out_w: int) -> np.ndarray:
    """2× 最近邻放大 + 3 抽头平滑（色度是低频量，放大后再补一次平滑就够）。"""
    a = np.repeat(np.repeat(a, 2, axis=0), 2, axis=1)
    pad_h, pad_w = out_h - a.shape[0], out_w - a.shape[1]
    if pad_h > 0 or pad_w > 0:
        pad = ((0, max(0, pad_h)), (0, max(0, pad_w))) + ((0, 0),) * (a.ndim - 2)
        a = np.pad(a, pad, mode="edge")
    a = a[:out_h, :out_w]
    p0 = np.pad(a, ((1, 1), (0, 0)) + ((0, 0),) * (a.ndim - 2), mode="edge")
    a = 0.25 * p0[:-2] + 0.5 * p0[1:-1] + 0.25 * p0[2:]
    p1 = np.pad(a, ((0, 0), (1, 1)) + ((0, 0),) * (a.ndim - 2), mode="edge")
    return 0.25 * p1[:, :-2] + 0.5 * p1[:, 1:-1] + 0.25 * p1[:, 2:]


def _down4(a: np.ndarray) -> np.ndarray:
    """4× 块平均降采样（光晕只需要低频）。"""
    return _down2(_down2(a))


def _up4_smooth(a: np.ndarray, out_h: int, out_w: int) -> np.ndarray:
    """4× 最近邻放大 + 3 抽头平滑（比双线性索引快，且光晕/颗粒都不需要严格插值）。"""
    a = np.repeat(np.repeat(a, 4, axis=0), 4, axis=1)
    pad_h, pad_w = out_h - a.shape[0], out_w - a.shape[1]
    if pad_h > 0 or pad_w > 0:
        pad = ((0, max(0, pad_h)), (0, max(0, pad_w))) + ((0, 0),) * (a.ndim - 2)
        a = np.pad(a, pad, mode="edge")
    a = a[:out_h, :out_w]
    p0 = np.pad(a, ((1, 1), (0, 0)) + ((0, 0),) * (a.ndim - 2), mode="edge")
    a = 0.25 * p0[:-2] + 0.5 * p0[1:-1] + 0.25 * p0[2:]
    p1 = np.pad(a, ((0, 0), (1, 1)) + ((0, 0),) * (a.ndim - 2), mode="edge")
    return 0.25 * p1[:, :-2] + 0.5 * p1[:, 1:-1] + 0.25 * p1[:, 2:]


# --------------------------------------------------------------------------- 场景统计
def scene_features(lin: np.ndarray, disp: np.ndarray) -> dict:
    """统计 8 个场景信号并折算成 5 个连续权重（不做 if/else 硬切）。"""
    h, w = disp.shape[:2]
    y_disp = luma_of(disp)
    y_lin = luma_of(lin)

    p1, p5, p25, p50, p75, p95, p99 = np.percentile(y_lin, [1, 5, 25, 50, 75, 95, 99])
    d1, d5, d25, d50, d75, d95, d99 = np.percentile(y_disp, [1, 5, 25, 50, 75, 95, 99])

    contrast_stops = float(math.log2((p95 + 1e-5) / (p5 + 1e-5)))

    mx = disp.max(axis=2)
    mn = disp.min(axis=2)
    sat_mean = float(np.mean((mx - mn) / np.maximum(mx, 1e-4)))

    top = disp[: max(2, h // 3)]
    top_warmth = float(np.mean(top[..., 0] - top[..., 2]))
    top_green = float(np.mean(top[..., 1] - top[..., 2]))

    top_half = disp[: max(2, h // 2)]
    sky_mask = (top_half[..., 2] > top_half[..., 0] * 1.10) & (
        top_half[..., 2] > top_half[..., 1] * 1.05
    )
    sky_frac = float(sky_mask.mean())

    high_frac = float((y_disp > 0.92).mean())
    low_frac = float((y_disp < 0.07).mean())

    # 肤色占比：YCbCr 近似（与 cv2 的 YCrCb 同口径，但纯 numpy）
    y8 = y_disp
    cb = 0.5 + 0.564 * (disp[..., 2] - y8)
    cr = 0.5 + 0.713 * (disp[..., 0] - y8)
    skin = (y8 > 0.30) & (y8 < 0.85) & (cr > 0.56) & (cr < 0.70) & (cb > 0.40) & (cb < 0.53)
    skin_frac = float(skin.mean())

    hi_mask = y_disp > np.percentile(y_disp, 92)
    hi_rgb = disp[hi_mask]
    hi_warmth = float(np.mean(hi_rgb[:, 0] - hi_rgb[:, 2])) if hi_rgb.size else 0.0

    w_sunset = float(smoothstep(np.float32(hi_warmth), 0.08, 0.22))
    w_night = float(1.0 - smoothstep(np.float32(d75), 0.12, 0.30))
    w_backlit = float(smoothstep(np.float32(contrast_stops), 6.2, 9.2))
    w_bluesky = float(smoothstep(np.float32(sky_frac), 0.30, 0.65))
    w_lowcon = float(1.0 - smoothstep(np.float32(contrast_stops), 1.6, 3.4))
    w_skin = float(smoothstep(np.float32(skin_frac), 0.02, 0.15))

    dominant = max(
        [("日落/黄金时刻", w_sunset), ("强蓝天/高饱和", w_bluesky), ("逆光/大光比", w_backlit),
         ("低对比/阴天", w_lowcon), ("夜景/暗调", w_night)],
        key=lambda t: t[1],
    )
    label = dominant[0] if dominant[1] > 0.35 else "常规日光"

    return {
        "p1": float(p1), "p5": float(p5), "p25": float(p25), "p50": float(p50),
        "p75": float(p75), "p95": float(p95), "p99": float(p99),
        "d5": float(d5), "d50": float(d50), "d75": float(d75), "d95": float(d95),
        "contrast_stops": contrast_stops, "sat_mean": sat_mean,
        "top_warmth": top_warmth, "top_green": top_green, "hi_warmth": hi_warmth,
        "sky_frac": sky_frac, "high_frac": high_frac, "low_frac": low_frac,
        "skin_frac": skin_frac,
        "w_sunset": w_sunset, "w_night": w_night, "w_backlit": w_backlit,
        "w_bluesky": w_bluesky, "w_lowcon": w_lowcon, "w_skin": w_skin,
        "label": label,
    }


def estimate_wb(lin: np.ndarray, disp: np.ndarray) -> Tuple[np.ndarray, dict]:
    """灰世界 + 低梯度约束：只用「平坦、中亮、近中性」的像素估白点。

    用全图灰世界会把夕阳/蓝天本身当色偏校掉；这里先筛掉高饱和与高梯度，
    再把强度按场景（日落/蓝天）减权（见 wb_strength_for）。
    """
    y = luma_of(disp)
    gray = 0.299 * disp[..., 0] + 0.587 * disp[..., 1] + 0.114 * disp[..., 2]
    gx = np.zeros_like(gray)
    gy = np.zeros_like(gray)
    gx[:, 1:-1] = (gray[:, 2:] - gray[:, :-2]) * 0.5
    gy[1:-1, :] = (gray[2:, :] - gray[:-2, :]) * 0.5
    grad = np.sqrt(gx * gx + gy * gy)

    mx = disp.max(axis=2)
    mn = disp.min(axis=2)
    sat = (mx - mn) / np.maximum(mx, 1e-4)

    grad_thr = np.percentile(grad, 55)
    lo, hi = np.percentile(y, 25), np.percentile(y, 96)
    mask = (grad < grad_thr) & (y > lo) & (y < hi) & (sat < 0.30)
    if int(mask.sum()) < 2000:
        mask = (y > lo) & (y < hi) & (sat < 0.45)
    if int(mask.sum()) < 1000:
        return np.ones(3, dtype=np.float32), {"gains": [1.0, 1.0, 1.0], "cast_stops": 0.0}

    mean_rgb = lin[mask].reshape(-1, 3).mean(axis=0) + 1e-6
    luma_mean = float(mean_rgb @ LUMA)
    gains = np.clip(luma_mean / mean_rgb, 0.90, 1.12).astype(np.float32)
    gains = gains / float(gains @ LUMA)
    dev = float(np.max(np.abs(np.log2(gains))))
    return gains, {"gains": [float(g) for g in gains], "cast_stops": dev}


def wb_strength_for(f: dict, cast_stops: float) -> float:
    s = 0.18 + 0.55 * float(smoothstep(np.float32(cast_stops), 0.12, 0.50))
    s *= 1.0 - 0.55 * f["w_sunset"]
    s *= 1.0 - 0.35 * f["w_bluesky"]
    s += 0.05 * f["w_lowcon"] + 0.05 * f["w_skin"]
    return float(np.clip(s, 0.0, 0.75))


def exposure_for(lin: np.ndarray, f: dict, ev_bias: float = 0.0) -> Tuple[float, dict]:
    y_lin = luma_of(lin)
    p50 = float(np.percentile(y_lin, 50))
    p995 = float(np.percentile(y_lin, 99.5))
    target_mid = TARGET_MID - 0.014 * f["w_sunset"] - 0.010 * f["w_backlit"]
    s_mid = (target_mid / max(p50, 1e-5)) ** 0.80
    s_mid *= 2.0 ** ev_bias
    s_hi = 0.86 / max(p995, 1e-5)
    scale = min(s_mid, s_hi)
    if f["w_night"] > 0.4:
        scale = min(scale, 1.8)
    scale = float(np.clip(scale, 0.30, 4.0))
    return scale, {
        "scale": scale, "stops": float(math.log2(scale)),
        "s_mid": float(s_mid), "s_hi": float(s_hi),
        "limited_by": "高光" if s_hi < s_mid else "中灰",
    }


# --------------------------------------------------------------------------- 曲线
def _soft_shoulder(y: np.ndarray, knee: float, k: float) -> np.ndarray:
    t = np.maximum(y - knee, 0.0) / (1.0 - knee)
    g = 1.0 - np.power(1.0 + k * t, -1.0 / k)
    return np.where(y <= knee, y, knee + (1.0 - knee) * g)


def _contrast_s(y: np.ndarray, p: float) -> np.ndarray:
    if abs(p - 1.0) < 1e-4:
        return y
    yp = np.power(np.clip(y, 1e-6, 1.0), p)
    yn = np.power(np.clip(1.0 - y, 1e-6, 1.0), p)
    return yp / (yp + yn)


def _film_curve_point(E: np.ndarray, p: dict) -> np.ndarray:
    pivot = float(p["pivot"])
    cg = np.array(p.get("ch_gamma", (1.0, 1.0, 1.0)), dtype=np.float32)
    cp = np.array(p.get("ch_pivot", (0.0, 0.0, 0.0)), dtype=np.float32)
    base = 0.5 * np.power(E / (pivot * (2.0 ** cp)), 1.0 / (float(p["gamma"]) * cg))
    base = _contrast_s(base, float(p.get("s_curve", 1.0)))
    y = _soft_shoulder(base, float(p["knee"]), float(p["shoulder"]))
    y = float(p["fog"]) + (1.0 - float(p["fog"])) * np.clip(y, 0.0, 1.0)
    return y


def curve_tables(p: dict) -> np.ndarray:
    """建 3×16384 的曲线表（sqrt(E) 域）；返回 shape (3, n)。"""
    u = np.linspace(0.0, _CURVE_UMAX, _CURVE_N, dtype=np.float32)
    E = (u * u).astype(np.float32)
    tabs = np.empty((3, _CURVE_N), dtype=np.float32)
    work = np.zeros((_CURVE_N, 3), dtype=np.float32)
    for c in range(3):
        work[:] = 0.0
        work[:, c] = E
        tabs[c] = _film_curve_point(work, p)[:, c]
    return tabs


def curve_apply(lin: np.ndarray, exp_scale: float, tabs: np.ndarray) -> np.ndarray:
    E = np.maximum(lin * exp_scale, 1e-7)
    u = np.sqrt(E, dtype=np.float32)
    idx = np.clip(u / _CURVE_UMAX * (_CURVE_N - 1), 0, _CURVE_N - 1).astype(np.uint16)
    out = np.empty_like(lin)
    for c in range(3):
        out[..., c] = np.take(tabs[c], idx[..., c])
    return out


# --------------------------------------------------------------------------- 光学 / 色彩
def add_halation(lin: np.ndarray, strength: float) -> np.ndarray:
    """高光扩散：阈值以上的亮部做暖色光晕。

    只对**亮度**做一次降采样→模糊→放大，再乘固定色偏（红橙）。光晕本身没有色相细节，
    逐通道放大会多付 3 倍内存搬运，没有视觉收益。
    """
    if strength <= 1e-4:
        return lin
    y = luma_of(lin)
    thr = np.percentile(y, 99.0)
    m = np.clip((y - thr) / max(1.0 - thr, 1e-3), 0.0, 1.0)
    src = (y * m).astype(np.float32)
    h, w = lin.shape[:2]
    small = _down4(src)
    sigma = max(1.0, 0.010 * min(small.shape[:2]))
    glow = blur(small, sigma)
    glow = _up4_smooth(glow, h, w)
    tint = np.array([1.0, 0.42, 0.20], dtype=np.float32)
    return lin + glow[..., None] * tint * (strength * 1.4)


def add_grain(y: np.ndarray, amount: float, seed: int) -> np.ndarray:
    """亮度相关颗粒：中间调最强、纯黑纯白最弱；半分辨率生成再放大，颗粒成团且省时。"""
    if amount <= 1e-4:
        return y
    h, w = y.shape[:2]
    hh, ww = max(1, h // 2), max(1, w // 2)
    rng = np.random.default_rng(seed)
    n = rng.normal(0.0, 1.0, (hh, ww)).astype(np.float32)
    n /= max(float(n.std()), 1e-6)
    n = _up4_smooth(n[..., None], h, w)[..., 0]
    l = luma_of(y)
    weight = 4.0 * l * (1.0 - l)
    return y + (n * weight)[..., None] * (amount * 0.055)


def _rgb01_to_lab(rgb: np.ndarray) -> np.ndarray:
    """显示域 float[0,1] → CIELAB（numpy 实现，口径与 colortransfer 一致）。"""
    lin = srgb_decode(rgb)
    xyz = (lin.astype(np.float32) @ _M.T) / _WHITE
    f = np.where(xyz > _D ** 3, np.cbrt(xyz), xyz / (3 * _D * _D) + 4.0 / 29.0)
    L = 116.0 * f[..., 1] - 16.0
    a = 500.0 * (f[..., 0] - f[..., 1])
    b = 200.0 * (f[..., 1] - f[..., 2])
    return np.stack([L, a, b], axis=-1).astype(np.float32)


def _lin_to_lab(lin: np.ndarray) -> np.ndarray:
    """线性光 → CIELAB（白平衡后的原图正好在这一层，省掉一次 encode+decode）。"""
    xyz = (lin.astype(np.float32) @ _M.T) / _WHITE
    f = np.where(xyz > _D ** 3, np.cbrt(xyz), xyz / (3 * _D * _D) + 4.0 / 29.0)
    L = 116.0 * f[..., 1] - 16.0
    a = 500.0 * (f[..., 0] - f[..., 1])
    b = 200.0 * (f[..., 1] - f[..., 2])
    return np.stack([L, a, b], axis=-1).astype(np.float32)


def _lab_to_rgb01(lab: np.ndarray) -> np.ndarray:
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0
    xyz = np.stack([
        np.where(fx > _D, fx ** 3, 3 * _D * _D * (fx - 4.0 / 29.0)),
        np.where(fy > _D, fy ** 3, 3 * _D * _D * (fy - 4.0 / 29.0)),
        np.where(fz > _D, fz ** 3, 3 * _D * _D * (fz - 4.0 / 29.0)),
    ], axis=-1) * _WHITE
    lin = xyz @ _MINV.T
    return srgb_encode(np.clip(lin, 0.0, 1.0))


def _hue_table(h_deg: np.ndarray, ctrl) -> np.ndarray:
    xs = np.array([c[0] for c in ctrl], dtype=np.float32)
    ys = np.array([c[1] for c in ctrl], dtype=np.float32)
    return np.interp(h_deg, xs, ys, period=360.0).astype(np.float32)


_HUE_LUT_N = 3600  # 0.1° 一格：插值表按色相角展开成定长表，之后只做 np.take


def _hue_lut(ctrl) -> np.ndarray:
    xs = np.array([c[0] for c in ctrl], dtype=np.float32)
    ys = np.array([c[1] for c in ctrl], dtype=np.float32)
    grid = np.linspace(0.0, 360.0, _HUE_LUT_N, endpoint=False, dtype=np.float32)
    return np.interp(grid, xs, ys, period=360.0).astype(np.float32)


def _lookup_hue(lut: np.ndarray, h_s: np.ndarray) -> np.ndarray:
    hd = np.degrees(h_s) % 360.0
    idx = np.clip(hd * (_HUE_LUT_N / 360.0), 0, _HUE_LUT_N - 1).astype(np.uint16)
    return np.take(lut, idx)


def apply_chroma(y: np.ndarray, lin_src: np.ndarray, p: dict, skin_mask) -> np.ndarray:
    """用「原图彩度 × 风格系数」重建彩度，再按 Lab 色相分区做风格偏移。

    与 lab 版同口径，但为速度做了两处工程取舍：
      1. **半分辨率**：CIELAB 的 a/b 是低频量，在 1/2 图上算「彩度/色相改动量」，
         再把 RGB 增量放大回全图。明度完全走全图，细节不受影响；
      2. 源色相直接从**线性光**算 Lab，省一次 encode→decode；指纹式的
         「分区彩度比」折进这里的 s（按源色相查表），省掉渲染后再做一次 Lab 往返。
    """
    h, w = y.shape[:2]
    y_s = _down2(np.clip(y, 0.0, 1.0))
    ls_s = _down2(np.clip(lin_src, 0.0, 1.0))
    lab_o = _rgb01_to_lab(y_s)
    lab_s = _lin_to_lab(ls_s)

    a_o, b_o = lab_o[..., 1], lab_o[..., 2]
    a_s, b_s = lab_s[..., 1], lab_s[..., 2]
    c_o = np.sqrt(a_o ** 2 + b_o ** 2)
    c_s = np.sqrt(a_s ** 2 + b_s ** 2)
    h_o = np.arctan2(b_o, a_o)
    h_s = np.arctan2(b_s, a_s)

    L = lab_o[..., 0] / 100.0
    s = float(p["sat"]) * (1.0 - float(p["sat_hi"]) * smoothstep(L, 0.72, 1.0))
    s = s * (1.0 + 0.10 * (1.0 - smoothstep(L, 0.15, 0.55)))
    vib = float(p.get("vibrance", 0.0))
    s = s * (1.0 + vib * (1.0 - smoothstep(c_s, 8.0, 55.0)))

    chroma_tab = p.get("chroma")
    if chroma_tab:
        s = s * _lookup_hue(_hue_lut(chroma_tab), h_s)

    inner_mask = None
    if skin_mask is not None and float(p.get("skin_protect", 0.0)) > 0:
        inner_mask = _down2(skin_mask)
        s = s * (1.0 - float(p["skin_protect"]) * inner_mask)

    shift = np.zeros_like(h_o)
    hue_ctrl = p.get("hue_shift")
    if hue_ctrl:
        shift = np.deg2rad(_lookup_hue(_hue_lut(hue_ctrl), h_s)) \
            * float(p.get("hue_strength", 0.0))
        if inner_mask is not None:
            shift = shift * (1.0 - 0.8 * inner_mask)

    target = c_s * s
    wgt = smoothstep(c_s, 3.0, 12.0)
    c_new = c_o + (target - c_o) * wgt

    h_t = h_s + shift
    dh = np.arctan2(np.sin(h_t - h_o), np.cos(h_t - h_o))
    h_new = h_o + dh * wgt

    lab_new = lab_o.copy()
    lab_new[..., 1] = c_new * np.cos(h_new)
    lab_new[..., 2] = c_new * np.sin(h_new)
    rgb_new = _lab_to_rgb01(lab_new)
    delta = rgb_new - y_s
    return y + _up2_smooth(delta, h, w)


def compress_gamut(y: np.ndarray, rounds: int = 10, pull: float = 0.82) -> np.ndarray:
    """越界像素朝亮度轴收彩度。

    优化：绝大多数图只有零点几个百分点的像素越界（蓝天最亮处），
    只在**越界像素列表**上迭代，避免每轮对 8MP 全图做 3 次比较 + where；
    越界比例大（>12.5%）时才走全图路径，两条路径结果一致。
    """
    flat = y.reshape(-1, 3)
    bad = ((flat[:, 0] < 0.0) | (flat[:, 0] > 1.0) | (flat[:, 1] < 0.0)
           | (flat[:, 1] > 1.0) | (flat[:, 2] < 0.0) | (flat[:, 2] > 1.0))
    n_bad = int(bad.sum())
    if n_bad == 0:
        return np.clip(y, 0.0, 1.0)
    if n_bad * 8 < flat.shape[0]:
        idx = np.nonzero(bad)[0]
        sub = flat[idx]
        l = sub @ LUMA
        for _ in range(rounds):
            b2 = (sub < 0.0) | (sub > 1.0)
            if not b2.any():
                break
            sub = np.where(b2, l[:, None] + (sub - l[:, None]) * pull, sub)
        flat[idx] = sub
        return np.clip(flat.reshape(y.shape), 0.0, 1.0)

    l = luma_of(y)[..., None]
    for _ in range(rounds):
        bad2 = (y < 0.0) | (y > 1.0)
        if not bad2.any():
            break
        y = np.where(bad2, l + (y - l) * pull, y)
    return np.clip(y, 0.0, 1.0)


def soft_limit(y: np.ndarray, start: float = 0.96, ceil: float = 0.984) -> np.ndarray:
    m = np.maximum(np.maximum(y[..., 0], y[..., 1]), y[..., 2])[..., None]
    over = m > start
    if not over.any():
        return y
    t = np.maximum(m - start, 0.0) / (ceil - start)
    m_new = start + (ceil - start) * (1.0 - np.exp(-t))
    scale = np.where(over, m_new / np.maximum(m, 1e-6), 1.0)
    return y * scale


# --------------------------------------------------------------------------- 参数
def adapt_params(recipe: dict, f: dict) -> dict:
    """按场景特征连续调参：同一套公式覆盖所有场景。"""
    p = dict(recipe)
    cs = f["contrast_stops"]

    p["gamma"] = float(np.clip(
        recipe["gamma"] + 0.40 * max(0.0, cs - 6.0) + LOWCON_GAMMA * max(0.0, 5.0 - cs),
        2.05, 3.45,
    ))
    p["sat"] = float(np.clip(
        recipe["sat"] * (1.0 - 0.10 * smoothstep(np.float32(f["sat_mean"]), 0.26, 0.52))
        * (1.0 + 0.12 * (1.0 - smoothstep(np.float32(f["sat_mean"]), 0.10, 0.32))),
        0.35, 1.15,
    ))
    p["knee"] = float(np.clip(recipe["knee"] + 0.04 * f["w_sunset"] + 0.02 * f["w_bluesky"], 0.80, 0.90))
    p["shoulder"] = float(np.clip(
        recipe["shoulder"] + 0.5 * f["w_backlit"] + 0.25 * f["w_sunset"], 1.2, 2.8))
    p["s_curve"] = float(np.clip(
        recipe.get("s_base", 1.20) + 0.14 * f["w_lowcon"] - 0.06 * f["w_backlit"]
        - 0.03 * f["w_sunset"], 1.05, 1.45))
    p["fog"] = float(np.clip(recipe["fog"] + 0.015 * f["w_night"], 0.0, 0.05))
    p["halation"] = float(np.clip(
        recipe["halation"] * (1.0 + 0.9 * f["w_sunset"] + 0.6 * f["w_night"]), 0.0, 0.35))
    p["grain"] = float(np.clip(
        recipe["grain"] * (1.0 + 0.5 * f["w_night"]) * (1.0 - 0.35 * f["w_bluesky"]), 0.0, 0.12))
    p["sat_hi"] = float(np.clip(recipe["sat_hi"] + 0.20 * f["w_sunset"], 0.2, 0.85))
    p["skin_protect"] = float(np.clip(0.55 * f["w_skin"], 0.0, 0.55))
    p["pivot"] = float(recipe["pivot"])
    return p


def _skin_mask_from(y: np.ndarray) -> np.ndarray:
    """显示域判定肤色 → 羽化。只在 w_skin>0 时才走到这里。"""
    from PIL import Image, ImageFilter

    y8 = luma_of(y)
    cb = 0.5 + 0.564 * (y[..., 2] - y8)
    cr = 0.5 + 0.713 * (y[..., 0] - y8)
    m = ((y8 > 0.30) & (y8 < 0.85) & (cr > 0.56) & (cr < 0.70) & (cb > 0.40) & (cb < 0.53))
    m8 = (m.astype(np.float32) * 255.0)
    img = Image.fromarray(m8.astype(np.uint8), mode="L").filter(ImageFilter.GaussianBlur(2.0))
    return np.asarray(img, dtype=np.float32) / 255.0


# --------------------------------------------------------------------------- 渲染
def analyze(rgb: np.ndarray) -> dict:
    """每张图只做一次的场景分析（多风格共用，不随风格重复）。

    统计一律在 1/2 抽样图上做：8MP 的 percentile / max-min 占 0.5s，抽样后 0.15s，
    对曝光与权重的实际影响 <0.01 EV（比 clip 门槛小两个数量级）。
    """
    rgb_s = rgb[::3, ::3]
    disp_s = rgb_s.astype(np.float32) / 255.0
    lin_s = decode_u8(rgb_s)
    feats = scene_features(lin_s, disp_s)
    gains, wb_info = estimate_wb(lin_s, disp_s)
    wb_strength = wb_strength_for(feats, wb_info["cast_stops"])
    return {
        "features": feats,
        "wb": {**wb_info, "strength": round(wb_strength, 3)},
        "wb_gains": gains,
        "wb_strength": wb_strength,
    }


def render(rgb: np.ndarray, recipe: dict, stats: dict, seed: int = 0,
           look: float = 1.0) -> Tuple[np.ndarray, dict]:
    """渲染一张（uint8 RGB 入、uint8 RGB 出）。

    look：风格强度（轻 0.6 / 标准 1.0 / 浓 1.4）。偏色与对比按乘性放大，
    1.0 时与原配方逐位一致；0.6 更接近写实，1.4 更外放。
    """
    f = stats["features"]
    gains = stats["wb_gains"]
    wb_strength = stats["wb_strength"]

    lin = decode_u8(rgb)
    p = adapt_params(recipe, f)

    lk = float(look)
    if abs(lk - 1.0) > 1e-3:
        p["shadow_tint"] = tuple(v * (1.0 + (lk - 1.0) * 2.2) for v in recipe["shadow_tint"])
        p["high_tint"] = tuple(v * (1.0 + (lk - 1.0) * 2.2) for v in recipe["high_tint"])
        p["hue_strength"] = float(p["hue_strength"] * (1.0 + (lk - 1.0) * 0.8))
        p["chroma"] = [(h, 1.0 + (v - 1.0) * (1.0 + (lk - 1.0) * 0.9)) for h, v in recipe["chroma"]]
        p["s_curve"] = float(np.clip(p["s_curve"] * (1.0 + (lk - 1.0) * 0.14), 1.0, 1.6))

    p.setdefault("skin_protect", 0.0)
    p.setdefault("s_curve", 1.0)

    g = 1.0 + (gains - 1.0) * wb_strength
    lin = np.clip(lin * g.astype(np.float32), 0.0, None)

    exp_scale, exp_info = exposure_for(lin, f, float(p.get("ev_bias", 0.0)))

    lin = add_halation(lin, float(p["halation"]))
    y = curve_apply(lin, exp_scale, curve_tables(p))

    skin_mask = None
    if p.get("skin_protect", 0.0) > 0:
        skin_mask = _skin_mask_from(y)

    y = apply_chroma(y, lin, p, skin_mask)

    c = float(p["crosstalk"])
    Mx = np.full((3, 3), c / 2.0, dtype=np.float32)
    np.fill_diagonal(Mx, 1.0 - c)
    y = y @ Mx.T

    l = luma_of(y)[..., None]
    sh = np.array(p["shadow_tint"], dtype=np.float32)
    hi = np.array(p["high_tint"], dtype=np.float32)
    y = y + sh * ((1.0 - l) ** 2) + hi * (l ** 2)

    y = compress_gamut(y)
    y = add_grain(y, float(p["grain"]), seed)
    y = soft_limit(y)
    out = np.clip(y * 255.0 + 0.5, 0, 255).astype(np.uint8)
    return out, {"exp": exp_info, "params": {k: (list(v) if isinstance(v, tuple) else v)
                                            for k, v in p.items() if k != "name"}}


def apply_grade(rgb: np.ndarray, preset: dict, strength: float = 1.0) -> np.ndarray:
    """L2 后调层：分色调（阴影/中调/高光）+ 降饱和 + 对比 + 抬黑。

    与风格渲染分开：L1 决定「像不像原厂」，L2 决定「偏色多明显」。
    strength=0 原样返回；1.0 是配方原值。
    """
    s = float(np.clip(strength, 0.0, 1.4))
    if s <= 1e-3:
        return rgb
    x = rgb.astype(np.float32) / 255.0
    l = luma_of(x)
    for zone, w in (("shadow", (1.0 - l) ** 2), ("mid", 4.0 * l * (1.0 - l)), ("high", l ** 2)):
        x = x + np.array(preset[zone], np.float32) * (w[..., None] * s)
    x = np.clip(x + float(preset["fog"]) * s, 0.0, None)

    sat = 1.0 + (float(preset["sat"]) - 1.0) * s
    if abs(sat - 1.0) > 1e-4:
        lab = _rgb01_to_lab(np.clip(x, 0.0, 1.0))
        lab[..., 1] *= sat
        lab[..., 2] *= sat
        x = _lab_to_rgb01(lab)

    k = 1.0 + (float(preset["contrast"]) - 1.0) * s
    x = np.clip(0.5 + (x - 0.5) * k, 0.0, None)
    x = compress_gamut(np.clip(x, 0.0, 1.2))
    x = soft_limit(x)
    return np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8)
