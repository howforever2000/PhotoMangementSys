"""自然调色 v2 原型（FEAT-219 三层解耦的「无参考图」版本）。

只用于 A/B 实验，不改生产 `autograde.py`。核心取舍：

1. **影调层**：不再做全局黑白场归一化。只在该修的时候修：
   - 真欠曝（死黑像素 >6% 且中位亮度 <0.26）→ 有界 gamma 提亮；
   - 真过曝（截断像素 >2% 且中位亮度 >0.72）→ 有界 gamma 压暗；
   - 其余 gamma=1，中位亮度不搬。
   之后叠一条以中位亮度为锚的轻微 S 曲线（保意图、保端点）。
2. **色偏层**：只用近中性像素估白平衡，增益上限 ±5%，归一化到最大通道=1
   （只压不提），避免灰世界把蓝天/草地当色偏、也避免把通道推出边界。
3. **饱和度层**：LCh 只缩放彩度 C + 色域 headroom；高彩度滚降；肤色门控。
4. **收口**：亮度等比肩部（保色相/饱和比例）+ maxRGB 软限幅（处理单通道贴顶）。
"""
from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from ai_studio import autograde

# ---- 曝光闸门（与 bench/grade_tone_v2 同一口径，阈值是实测出来的） ----
OVER_CLIP = 0.02          # 任一通道 >=0.98 的像素占比
UNDER_BLACK = 0.06        # 亮度 <=0.02 的像素占比
UNDER_CEIL = 0.26
OVER_FLOOR = 0.72
GAMMA_MIN, GAMMA_MAX = 0.85, 1.20

# ---- 白平衡 ----
NEUTRAL_CHROMA = 0.12     # RGB (max-min)/max 低于它视为近中性
NEUTRAL_MIN_RATIO = 0.02
WB_LIMIT = 0.05
WB_DEADBAND = 0.03

# ---- 饱和度 ----
C_SOFT = 42.0
C_HIGH_KNEE = 45.0
C_HIGH_SPAN = 30.0
SKIN_PROTECT = 0.6

# ---- 收口 ----
SHOULDER_KNEE = 0.88
SHOULDER_TARGET = 0.97
GAMUT_KNEE = 0.90
GAMUT_TARGET = 0.965
TOE_GUARD = 0.03          # 轻微 S 最多把某像素压暗 3% 亮度，防止新死黑
HEAD_GUARD = 0.05         # 最多提亮 5%，其余交给肩部


@dataclass(frozen=True)
class Params:
    gain_r: float
    gain_g: float
    gain_b: float
    gamma: float
    median: float
    contrast_k: float
    saturation: float
    vibrance: float
    protect_skin: bool


def _luma(x: np.ndarray) -> np.ndarray:
    return x[..., 0] * 0.2126 + x[..., 1] * 0.7152 + x[..., 2] * 0.0722


def _smoothstep(e0: float, e1: float, v: np.ndarray) -> np.ndarray:
    t = np.clip((v - e0) / max(e1 - e0, 1e-6), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def _neutral_gains(small: np.ndarray, lum: np.ndarray, strength: float) -> np.ndarray:
    """近中性像素白平衡：只压不提、上限 ±5%、样本不足就不动。"""
    lo, hi = np.percentile(lum, [10, 90])
    mid = (lum >= lo) & (lum <= hi)
    if mid.sum() < 64:
        return np.ones(3, dtype=np.float32)
    px = small[mid]
    mx = px.max(axis=-1)
    chroma = np.where(mx > 1e-6, (mx - px.min(axis=-1)) / np.maximum(mx, 1e-6), 0.0)
    neutral = chroma < NEUTRAL_CHROMA
    if neutral.sum() < NEUTRAL_MIN_RATIO * px.shape[0]:
        return np.ones(3, dtype=np.float32)

    ref = px[neutral]
    means = ref.mean(axis=0) + 1e-6
    gray = float(means.mean())
    raw = gray / means
    raw = raw / raw[1]
    if float(np.abs(raw - 1.0).max()) < WB_DEADBAND:
        return np.ones(3, dtype=np.float32)

    gains = 1.0 + (raw - 1.0) * 0.45 * float(strength)
    gains = np.clip(gains, 1.0 - WB_LIMIT, 1.0 + WB_LIMIT)
    gains = gains / gains.max()          # 只压不提
    return gains.astype(np.float32)


def _exposure_gamma(median: float, clip_ratio: float, black_ratio: float,
                    strength: float) -> float:
    """只修真的坏了曝光，其余返回 1.0（不动亮度意图）。"""
    s = float(np.clip(strength, 0.0, 1.0))
    if s <= 0.0:
        return 1.0
    if black_ratio > UNDER_BLACK and median < UNDER_CEIL:
        target = min(max(median * 1.30, 0.20), UNDER_CEIL)
    elif clip_ratio > OVER_CLIP and median > OVER_FLOOR:
        target = max(median * 0.94, OVER_FLOOR)
    else:
        return 1.0
    g = float(np.log(max(target, 1e-4)) / np.log(max(median, 1e-4)))
    return float(np.clip(1.0 + (g - 1.0) * s, GAMMA_MIN, GAMMA_MAX))


def analyze(img_u8: np.ndarray, *, strength: float = 0.85,
            contrast: float = 0.10, chroma: float = 0.10, vibrance: float = 0.14,
            protect_skin: bool = True) -> Params:
    small = autograde._subsample(img_u8).astype(np.float32) / 255.0
    lum = _luma(small)
    median = float(np.percentile(lum, 50))
    clip_ratio = float((small.max(axis=-1) >= 0.98).mean())
    black_ratio = float((lum <= 0.02).mean())

    gains = _neutral_gains(small, lum, strength)
    gamma = _exposure_gamma(median, clip_ratio, black_ratio, strength)
    s = float(np.clip(strength, 0.0, 1.0))

    return Params(
        gain_r=float(gains[0]), gain_g=float(gains[1]), gain_b=float(gains[2]),
        gamma=gamma, median=median,
        contrast_k=1.0 + float(contrast) * s,
        saturation=1.0 + float(chroma) * s,
        vibrance=float(vibrance) * s,
        protect_skin=bool(protect_skin),
    )


def _skin_weight(lab: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """FEAT-219 A4 的 v1 纯色函数版：h 20–60°、C 8–45、L 35–95，边界软过渡。"""
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    C = np.hypot(a, b)
    h = np.degrees(np.arctan2(b, a)) % 360.0
    w = (_smoothstep(15, 25, h) * (1.0 - _smoothstep(55, 65, h))
         * _smoothstep(4, 10, C) * (1.0 - _smoothstep(40, 50, C))
         * _smoothstep(30, 38, L) * (1.0 - _smoothstep(90, 98, L)))
    return w, C


def _saturate_lch(x: np.ndarray, k: float, protect_skin: bool) -> np.ndarray:
    """全局彩度乘 k，但高彩度滚降、肤色门控，越色域按 headroom 回收。"""
    lab = autograde._to_lab(x)
    w_skin, C = _skin_weight(lab)
    high_cut = np.clip((C_HIGH_KNEE + C_HIGH_SPAN - C) / C_HIGH_SPAN, 0.2, 1.0)
    eff_t = 1.0 + (float(k) - 1.0) * high_cut
    if protect_skin:
        eff_t = 1.0 + (eff_t - 1.0) * (1.0 - SKIN_PROTECT * w_skin)
    head = autograde._chroma_headroom(lab)
    eff = np.minimum(eff_t, head)
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return autograde._from_lab(out)


def _vibrance_lch(x: np.ndarray, amount: float, protect_skin: bool) -> np.ndarray:
    """自然饱和度：淡色多加、艳色少加；肤色同样门控。"""
    lab = autograde._to_lab(x)
    w_skin, C = _skin_weight(lab)
    w = np.clip(1.0 - C / C_SOFT, 0.0, 1.0)
    base = 1.0 + float(amount) * w
    if protect_skin:
        base = 1.0 + (base - 1.0) * (1.0 - SKIN_PROTECT * w_skin)
    head = autograde._chroma_headroom(lab)
    eff = np.minimum(base, head)
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return autograde._from_lab(out)


def _shoulder_luma(x: np.ndarray) -> np.ndarray:
    """FEAT-219 A6：按 Rec.709 亮度等比缩放，保色相与通道比例。"""
    x = np.asarray(x, dtype=np.float32)
    y = _luma(x)
    hi = np.clip((y - SHOULDER_KNEE) / max(1.0 - SHOULDER_KNEE, 1e-4), 0.0, 1.0)
    eased = hi * hi * (3.0 - 2.0 * hi)
    ty = np.where(y > SHOULDER_KNEE,
                  SHOULDER_KNEE + (SHOULDER_TARGET - SHOULDER_KNEE) * eased, y)
    f = np.where(y > 1e-6, ty / np.maximum(y, 1e-6), 1.0)
    out = x * f[..., None]
    if np.any(out > 1.0):
        out = np.where(out > 1.0, autograde._shoulder(np.clip(out, 0, None)), out)
    return np.clip(out, 0.0, 1.0)


def _gamut_shrink(x: np.ndarray) -> np.ndarray:
    """maxRGB 软限幅：只缩总增益、不改通道比例，处理单通道贴顶（如蓝天 B）。"""
    mx = x.max(axis=-1)
    t = np.clip((mx - GAMUT_KNEE) / max(1.0 - GAMUT_KNEE, 1e-4), 0.0, 1.0)
    eased = t * t * (3.0 - 2.0 * t)
    cap = np.where(mx > GAMUT_KNEE, GAMUT_KNEE + (GAMUT_TARGET - GAMUT_KNEE) * eased, mx)
    f = np.where(mx > 1e-6, cap / np.maximum(mx, 1e-6), 1.0)
    return x * f[..., None]


def eval_colors(p: Params, rgb: np.ndarray) -> np.ndarray:
    """逐色映射：0~1 进、0~1 出，无空间运算，可烘焙 33³ LUT。"""
    x = np.clip(rgb, 0.0, 1.0).astype(np.float32)

    # 1) 近中性白平衡（只压不提，不会推过 1.0）
    if abs(p.gain_r - 1) > 1e-4 or abs(p.gain_g - 1) > 1e-4 or abs(p.gain_b - 1) > 1e-4:
        x = x * np.array([p.gain_r, p.gain_g, p.gain_b], dtype=np.float32)

    # 2) 只修真欠曝/过曝的 gamma
    if abs(p.gamma - 1.0) > 1e-3:
        x = np.power(x, p.gamma)

    # 3) 以中位亮度为锚的轻微 S（按亮度倍率回贴 → 保色相）
    if abs(p.contrast_k - 1.0) > 1e-3:
        l = _luma(x)
        target = p.median + (l - p.median) * p.contrast_k
        # 端点守卫：单像素最多压暗/提亮固定幅度，避免把暗部推成死黑、把高光推爆
        target = np.clip(target, l - TOE_GUARD, l + HEAD_GUARD)
        target = np.clip(target, 0.0, 1.0)
        x = x * (target / np.maximum(l, 1e-4))[..., None]

    # 4) 先收口一次：亮度肩部 + maxRGB 软限幅，保证进入 LCh 前已在 0~1 内
    x = _shoulder_luma(x)
    x = _gamut_shrink(x)

    # 5/6) LCh 饱和度 + 自然饱和度（高彩度滚降 + 肤色门控 + 色域回收）
    if abs(p.saturation - 1.0) > 1e-3:
        x = _saturate_lch(x, p.saturation, p.protect_skin)
    if p.vibrance > 1e-3:
        x = _vibrance_lch(x, p.vibrance, p.protect_skin)

    # 7) 最终 maxRGB 软限幅（饱和后可能又贴顶）
    x = _gamut_shrink(x)
    return np.clip(x, 0.0, 1.0)


def build_lut(p: Params, size: int = 33) -> tuple[np.ndarray, int]:
    n = int(max(9, min(65, size)))
    line = np.linspace(0.0, 1.0, n, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")
    return eval_colors(p, np.stack([r, g, b], axis=-1)).astype(np.float32), n


def grade(img_u8: np.ndarray, *, strength: float = 0.85, lut_size: int = 33,
          **kw) -> tuple[np.ndarray, Params, tuple[np.ndarray, int]]:
    from ai_studio import lutcube

    p = analyze(img_u8, strength=strength, **kw)
    lut = build_lut(p, size=lut_size)
    return lutcube.apply_cube(img_u8, lut, strength=1.0), p, lut
