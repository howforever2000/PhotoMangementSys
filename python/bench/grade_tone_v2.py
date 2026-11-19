"""调色新方案原型（P2）：保意图 + 分区影调 + 零溢出收口。

设计出发点 —— 每条对应一处实测问题（证据见 `bench/out/grade_tone/report.md`）：

1. **不收敛到常数均值**。现行 `autograde.analyze` 把均值无条件拉向 `[0.40,0.50]`；
   实测偏暗素材（p50 0.18~0.30）被抬 +0.25，意图斜率 1.62 —— 无论多暗都被拉成
   一样亮。本方案只��「明显欠曝/过曝」时介入（p50 < 0.22 / > 0.72），且幅度封顶。

2. **不做黑白场归一化**。低 key 画面本就该不占满 0~1；现行 `_levels` 的白点拉伸
   会把单通道推爆（实测截断 0.008% → 22.6%），且把「本来就该暗」判成「欠曝」。

3. **分区影调用单调三次曲线**。5 个锚点：中调钉死（保住均值 = 保住意图）、
   暗部微压、亮部微抬。层次来自**斜率变化**而不是整体提亮 —— 这是它与
   「gamma 提亮」的本质差别。

4. **零溢出收口**。白平衡与色温增益归一化到「最大通道 = 1.0」，只压不提，
   通道永远不会被推过 1.0，因而不需要 shoulder 反复压缩（现行实现压 2~3 次，
   实测纯白 255 被压到 235~244）。

5. **白平衡只用近中性像素**。现行灰世界把草地/蓝天当色偏修掉，实测 6/10 张
   顶到 ±0.10 上限、系统性暖化 +0.03~0.075、蓝紫色相偏 −5~−8°。本方案只采信
   低饱和像素（灰墙/路面/云），上限 ±0.05。

仍然是**纯逐色映射**（无空间运算），可直接预计算成 3D LUT 与导出 `.cube`。
"""
from __future__ import annotations

import os
import sys

import numpy as np

_HERE = os.path.dirname(os.path.abspath(__file__))
_PY_ROOT = os.path.dirname(_HERE)
if _PY_ROOT not in sys.path:
    sys.path.insert(0, _PY_ROOT)

from ai_studio import autograde  # noqa: E402 —— 复用 _luma/_subsample/_shoulder，不重写
from ai_studio import colortransfer  # noqa: E402 —— 复用 rgb_to_lab/lab_to_rgb，不重写

# 风格档。与现行 `autograde.STYLES` 的关键差异：`contrast` 这次**真的**接进曲线
# （现行表里 contrast/wb 从未被 GradeStats 读取，是死参数）。
#
# 取值依据（本机 10 张实测扫描，natural 档）：
#   contrast 0.45 → 中段斜率 1.13，死黑 +0.016pp，绝对漂移 0.0130
#   contrast 0.60 → 中段斜率 1.18，死黑 +0.085pp，绝对漂移 0.0153
#   contrast 0.80 → 中段斜率 1.24，死黑 +0.250pp，绝对漂移 0.0184
# natural 取 0.60：层次增益已经肉眼可辨，而死黑增量仍在 0.1pp 以内。
# vivid 取 0.90 是对「鲜明」本该有的对比差异（现行 vivid 与 natural 只差饱和度）。
STYLES: dict[str, dict] = {
    "natural": {"contrast": 0.60, "saturation": 1.03, "vibrance": 0.12, "warmth": 0.0},
    "vivid":   {"contrast": 0.90, "saturation": 1.10, "vibrance": 0.24, "warmth": 0.02},
    "warm":    {"contrast": 0.62, "saturation": 1.04, "vibrance": 0.14, "warmth": 0.05},
    "cool":    {"contrast": 0.62, "saturation": 1.03, "vibrance": 0.12, "warmth": -0.05},
}
DEFAULT_STYLE = "natural"

# 曝光介入闸门：只处理**真的坏了**的曝光，绝不因为「均值偏离某个数」就重定基调。
#
# 为什么不能按中位数判过曝：标准色卡/色轮这类「大面积白底」素材 p50 高达 0.95，
# 但它们的白是内容本身，没有任何像素过曝。实测按 p50>0.72 介入会把这类图整体
# 压暗 0.075（gamma 1.25），属于「把正确的图改坏」。
# 判据改为：**有没有真的截断**（过曝）与**有没有真的死黑**（欠曝）。
_OVER_CLIP = 0.02        # 任一通道 ≥0.98 的像素占比超过 2% → 真过曝
_UNDER_BLACK = 0.06      # 亮度 ≤0.02 的像素占比超过 6% → 真欠曝
_UNDER_CEIL = 0.26       # 欠曝最多提到这里（仍然偏暗）
_OVER_FLOOR = 0.72

# 近中性像素的饱和度上限。0.12 对应「肉眼看起来就是灰/白」的颜色。
_NEUTRAL_CHROMA = 0.12
# 近中性像素太少时不估白平衡：宁可不动，也不拿场景色瞎调。
_NEUTRAL_MIN_RATIO = 0.02
# 白平衡增益幅度上限（现行是 ±0.10，实测 6/10 张顶满）。
_WB_LIMIT = 0.05

_ANCHORS = 5

# 暗部锚点的位置（占中调的比例）与亮部锚点的位置。
# 0.55 意味着暗部锚点落在中调 55% 处：再小会让「暗部段」太窄、斜率增益集中在极暗处，
# 肉眼看不出层次提升（实测 x1/mid=0.16 时中段斜率恒为 1.01，contrast 完全失效）。
_TOE_SPAN = 0.55
_SHOULDER_SPAN = 0.45


class GradeV2:
    """一次分析的产物：全部是标量 + 定长锚点，与画面尺寸无关、可 JSON 化。"""

    __slots__ = ("x", "y", "m", "gamma", "gain_r", "gain_g", "gain_b",
                 "saturation", "vibrance", "warmth", "sat_space")

    def __init__(self, x, y, m, gamma, gain_r, gain_g, gain_b,
                 saturation, vibrance, warmth, sat_space: str = "lch"):
        self.x = tuple(float(v) for v in x)
        self.y = tuple(float(v) for v in y)
        self.m = tuple(float(v) for v in m)
        self.gamma = float(gamma)
        self.gain_r = float(gain_r)
        self.gain_g = float(gain_g)
        self.gain_b = float(gain_b)
        self.saturation = float(saturation)
        self.vibrance = float(vibrance)
        self.warmth = float(warmth)
        # 饱和处理所在色彩空间：`lch`（默认，只动 C）或 `rgb`（旧行为）。
        # 保留 `rgb` 只为 A/B 对照实验 —— 报告里要把两种做法的成片并排给人看。
        self.sat_space = str(sat_space)

    def as_meta(self) -> dict:
        return {
            "anchors": [[round(a, 4), round(b, 4)] for a, b in zip(self.x, self.y)],
            "gamma": round(self.gamma, 4),
            "wbGain": [round(self.gain_r, 4), round(self.gain_g, 4), round(self.gain_b, 4)],
            "saturation": round(self.saturation, 4),
            "vibrance": round(self.vibrance, 4),
            "warmth": round(self.warmth, 4),
            "satSpace": self.sat_space,
        }


# ---------------------------------------------------------------- 曲线内核


def monotone_slopes(xs: np.ndarray, ys: np.ndarray) -> np.ndarray:
    """Fritsch–Carlson 单调切线。

    为什么自己实现而不是用 `scipy.interpolate.PchipInterpolator`：scipy 不在
    ai-studio 的打包清单里，而调色是核心链路 —— 不能为一条曲线引入新的重依赖。
    算法本身只有十几行，且必须保证单调（否则曲线会在暗部造出反向台阶）。
    """
    h = np.diff(xs)
    d = np.diff(ys) / h
    m = np.empty_like(ys)
    m[0], m[-1] = d[0], d[-1]
    for i in range(1, len(xs) - 1):
        if d[i - 1] * d[i] <= 0.0:
            m[i] = 0.0
        else:
            w1 = 2.0 * h[i] + h[i - 1]
            w2 = h[i] + 2.0 * h[i - 1]
            m[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
    return m


def eval_curve(xs, ys, m, t):
    """在 t∈[0,1] 上求单调三次曲线（Hermite 形式，纯逐点运算）。"""
    t = np.clip(t, 0.0, 1.0)
    idx = np.clip(np.searchsorted(xs, t) - 1, 0, len(xs) - 2)
    h = xs[idx + 1] - xs[idx]
    u = (t - xs[idx]) / h
    u2 = u * u
    u3 = u2 * u
    return (ys[idx] * (2 * u3 - 3 * u2 + 1)
            + m[idx] * h * (u3 - 2 * u2 + u)
            + ys[idx + 1] * (-2 * u3 + 3 * u2)
            + m[idx + 1] * h * (u3 - u2))


def tone_anchors(mid: float, contrast: float):
    """构造分区 S 曲线的 5 个锚点。

    形状：`x = (0, x1, mid, x3, 1)`、`y = (0, x1-d1, mid, x3+d2, 1)`。

    - `(mid, mid)` 钉死 → 中调不动 → 均值不漂移，**明暗意图得以保留**；
    - `(0,0)` / `(1,1)` 钉死 → 输出恒在界内，不需要硬 clip，也不需要 shoulder；
    - 暗部锚点下移 d1、亮部锚点上移 d2 → 中间两段斜率 > 1 → **层次增强**。

    关键设计：**d1/d2 用「到该侧端点的比例」表达，而不是绝对值**。

    这里踩过一个坑：第一版写成 `d1 = min((mid-x1)*系数, x1*0.15)`，
    结果低 key 图（p5≈0.03）的 x1 极小，`x1*0.15` 恒为上限 →
    `contrast` 完全失效（实测 0.45/0.60/0.75/0.90 四档的中段斜率都是 1.011）。
    改成比例形式后，暗部段斜率恒为 `1-amount`（≥0.5，暗部不会被焊死），
    中间两段斜率恒为 `1+amount/(1-ratio)`（>1，层次真正变强），
    `contrast` 因此对每一张图都线性生效。

    `amount` 上限 0.5：端点段斜率 `1-amount` 必须为正，低于 0 会把暗部压成死黑。
    """
    a = float(np.clip(contrast, 0.0, 1.0)) * 0.5
    c = float(np.clip(mid, 0.06, 0.94))
    # 锚点位置按「到中调的距离」定，保证四段都存在且比例稳定
    x1 = c * _TOE_SPAN
    x3 = c + (1.0 - c) * (1.0 - _SHOULDER_SPAN)
    d1 = x1 * a
    d2 = (1.0 - x3) * a
    xs = np.array([0.0, x1, c, x3, 1.0], dtype=np.float64)
    ys = np.array([0.0, x1 - d1, c, x3 + d2, 1.0], dtype=np.float64)
    return xs.astype(np.float32), ys.astype(np.float32)


# ---------------------------------------------------------------- 分析


def exposure_gamma(mid: float, clip_ratio: float, black_ratio: float,
                   strength: float) -> float:
    """只在**真的**过曝/欠曝时给出 gamma，其余返回 1.0（完全不动亮度）。

    这是「保明暗意图」的硬闸门：现行实现把均值无条件拉向 `[0.40,0.50]`，
    于是 p50=0.20 的低 key 素材被强行抬到 0.42（+0.22，实测意图斜率 1.62）。

    判据刻意不用「均值是否落在某区间」：
      - 低 key 创作照（p50 0.15~0.30）均值本来就该低，按区间判会把它判成欠曝 →
        这正是「所有地方都亮」的机制根因；
      - 大面积白底素材（色卡/文档/雪景）p50 可达 0.95，按区间判会把它判成过曝 →
        把正确的图整体压暗（实测 −0.075）。
    改用**截断率**与**死黑率**：只有真的丢了信息才介入，且目标值本身也封顶。
    """
    if strength <= 0.0:
        return 1.0
    if black_ratio > _UNDER_BLACK and mid < _UNDER_CEIL:
        target = min(max(mid * 1.30, 0.20), _UNDER_CEIL)
    elif clip_ratio > _OVER_CLIP and mid > _OVER_FLOOR:
        target = max(mid * 0.94, _OVER_FLOOR)
    else:
        return 1.0
    g = float(np.log(max(target, 1e-4)) / np.log(max(mid, 1e-4)))
    return float(np.clip(1.0 + (g - 1.0) * strength, 0.85, 1.20))


def neutral_gains(small: np.ndarray, lum: np.ndarray, strength: float) -> np.ndarray:
    """只用近中性像素估白平衡增益，并归一化到「最大通道 = 1.0」。

    两个关键决定：
      - **只取低饱和像素**：草地/蓝天/夕阳这类场景色占了画面主体时，全量灰世界
        必然把它们判成色偏。实测现行方案 6/10 张顶到 ±0.10 上限，草地照被推成
        「暖绿加粉紫天空」。
      - **归一化到最大通道 = 1.0**：只压不提，任何通道都不会被推过 1.0，
        因此整条链路不需要 shoulder 收口（也就没有「压两三次」的问题）。

    增益只对 `> 1` 的方向做夹取后再归一化，所以「提亮某通道」永远不发生 ——
    代价是整体略微变暗（最多 5%），换来的是零溢出与零重复压缩。
    """
    lo, hi = np.percentile(lum, [10, 90])
    mid_mask = (lum >= lo) & (lum <= hi)
    if mid_mask.sum() < 64:
        return np.ones(3, dtype=np.float32)
    px = small[mid_mask]
    mx = px.max(axis=-1)
    chroma = np.where(mx > 1e-6, (mx - px.min(axis=-1)) / np.maximum(mx, 1e-6), 0.0)
    neutral = chroma < _NEUTRAL_CHROMA
    if neutral.sum() < _NEUTRAL_MIN_RATIO * px.shape[0]:
        return np.ones(3, dtype=np.float32)

    ref = px[neutral]
    means = ref.mean(axis=0) + 1e-6
    gray = float(means.mean())
    raw = gray / means
    raw = raw / raw[1]
    dev = float(np.abs(raw - 1.0).max())
    if dev < 0.03:
        return np.ones(3, dtype=np.float32)

    gains = 1.0 + (raw - 1.0) * 0.45 * strength
    gains = np.clip(gains, 1.0 - _WB_LIMIT, 1.0 + _WB_LIMIT)
    gains = gains / gains.max()          # 只压不提：最大通道恰好 = 1.0
    return gains.astype(np.float32)


def analyze(img_u8: np.ndarray, style: str = DEFAULT_STYLE,
            strength: float = 1.0) -> GradeV2:
    """看图推导新方案的调色参数。`strength` 只缩幅度，不改变曲线形式。"""
    cfg = STYLES.get(style) or STYLES[DEFAULT_STYLE]
    s = float(np.clip(strength, 0.0, 1.0))

    small = autograde._subsample(img_u8).astype(np.float32) / 255.0
    lum = autograde._luma(small)
    mid = float(np.percentile(lum, 50))
    clip_ratio = float((small.max(axis=-1) >= 0.98).mean())
    black_ratio = float((lum <= 0.02).mean())

    gamma = exposure_gamma(mid, clip_ratio, black_ratio, s)
    mid_adj = float(np.clip(mid ** gamma, 0.06, 0.94))

    xs, ys = tone_anchors(mid_adj, cfg["contrast"] * s)
    ms = monotone_slopes(xs.astype(np.float64), ys.astype(np.float64))

    gains = neutral_gains(small, lum, s)

    def toward(derived: float, neutral: float) -> float:
        return neutral + (derived - neutral) * s

    return GradeV2(
        x=xs, y=ys, m=ms, gamma=gamma,
        gain_r=float(gains[0]), gain_g=float(gains[1]), gain_b=float(gains[2]),
        saturation=toward(cfg["saturation"], 1.0),
        vibrance=cfg["vibrance"] * s,
        warmth=cfg["warmth"] * s,
    )


# ---------------------------------------------------------------- 逐色映射


def warm_matrix(warmth: float) -> np.ndarray:
    """色温矩阵，同样归一化到「最大通道 = 1.0」（暖调抬红，但不把红推出边界）。"""
    if abs(warmth) < 1e-4:
        return np.ones(3, dtype=np.float32)
    m = np.array([1.0 + warmth, 1.0, 1.0 - warmth], dtype=np.float32)
    return m / m.max()


# ---------------- LCh 饱和度（只动 C，固定 h 与 L） ----------------
#
# 为什么必须放在这里而不是 RGB：RGB 里「以亮度为中心缩放」会把通道推出 [0,1] 后硬裁，
# 裁掉的部分同时改变明度与色相。实测自然照片 |ΔL| 0.238、饱和色块 0.585。
# LCh 里固定 (h, L) 只缩放 C，则明度和色相在数学上都不该变。
#
# 唯一的坑：Lab→sRGB 超色域时仍然会被裁。所以要**色域感知地回收 C** ——
# 先逐像素二分求出「这个颜色还能放大几倍而不越界」，缩放倍数取 min(目标, headroom)。
# 这样色相角严格保持，且依然只依赖单个输入颜色（可烘焙 LUT）。

_LCH_ITERS = 6          # 二分开销直接乘在这上面；6 次已把区间压到 1/64
_LCH_CEIL = 4.0         # 彩度缩放搜索上界（4 倍远超任何合理档位）


def _lab_to_linear(lab: np.ndarray) -> np.ndarray:
    """Lab → **线性** sRGB（不裁剪）。

    不能复用 `colortransfer.lab_to_rgb`：它末尾 `clip` 成 uint8，而判断
    「是否越色域」恰恰要看裁剪之前的值。
    """
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0
    D = 6.0 / 29.0

    def finv(t):
        return np.where(t > D, t ** 3, 3.0 * D * D * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * np.array(
        [0.95047, 1.0, 1.08883], dtype=np.float32)
    return xyz @ np.array([[3.2404542, -1.5371385, -0.4985314],
                           [-0.9692660, 1.8760108, 0.0415560],
                           [0.0556434, -0.2040259, 1.0572252]], dtype=np.float32).T


def _chroma_headroom(lab: np.ndarray, iters: int = _LCH_ITERS) -> np.ndarray:
    """逐像素求「保持 h 与 L 时，C 最多能放大几倍」的边界（向量化二分）。

    没有闭式解：XYZ 对 (a,b) 经 `finv` 后是三次分段函数。
    二分只缩放 (a,b) 的长度、不改方向，所以**色相角恒定**。
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


def _saturate_lch(x: np.ndarray, k: float) -> np.ndarray:
    """在 LCh 里把彩度乘 k，色相与明度保持不变，越界部分按 headroom 回收。

    `k < 1`（降饱和）时 headroom 必然 > 1，等价于直接乘；
    `k > 1` 时若该颜色已贴色域边界，实际倍数被压到 headroom，宁可少提也不裁歪色相。
    """
    lab = colortransfer.rgb_to_lab(_to_u8(x))
    head = _chroma_headroom(lab)
    eff = np.minimum(float(k), head)
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return colortransfer.lab_to_rgb(out).astype(np.float32) / 255.0


def _vibrance_lch(x: np.ndarray, amount: float) -> np.ndarray:
    """自然饱和度：按 LCh 的 C 判定「够不够艳」，只对 C 做加性缩放。

    RGB 版的权重来自 `max-min`（受明度影响），这里直接用 C：
    淡色 C 小 → 权重接近 1 多加；艳色 C 大 → 权重接近 0 少加。
    `_C_SOFT` 是「多艳算艳」的软阈值（Lab 的 C，0~130 量级）。
    """
    lab = colortransfer.rgb_to_lab(_to_u8(x))
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    c = np.sqrt(a * a + b * b)
    weight = np.clip(1.0 - c / _C_SOFT, 0.0, 1.0)
    target = 1.0 + amount * weight
    head = _chroma_headroom(lab)
    eff = np.minimum(target, head)
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return colortransfer.lab_to_rgb(out).astype(np.float32) / 255.0


_C_SOFT = 42.0          # Lab 彩度的软阈值：C 超过它视为「已经够艳」


def _to_u8(x: np.ndarray) -> np.ndarray:
    """float[0,1] → uint8。Lab 转换走 uint8 是因为 `colortransfer.rgb_to_lab`
    的入口就是 uint8（它与参考图调色共用同一实现，不在这里另写一套）。"""
    return np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8)


def eval_colors(g: GradeV2, rgb: np.ndarray) -> np.ndarray:
    """逐色映射：0~1 的 RGB 进，0~1 的 RGB 出。**不得引入任何空间运算**。

    顺序：白平衡 → 曝光 gamma → 分区 S 曲线 → 色温 → 饱和度 → 自然饱和度 → 收口。
    白平衡与色温都是「只压不提」的归一化增益，所以单通道永远不会被推过 1.0；
    最后的收口只处理 S 曲线抬亮部时饱和像素的少量溢出，**整条链路最多压一次**。
    """
    x = np.clip(rgb, 0.0, 1.0).astype(np.float32)

    # 1) 白平衡（只压不提，零溢出）
    if g.gain_r != 1.0 or g.gain_g != 1.0 or g.gain_b != 1.0:
        x = x * np.array([g.gain_r, g.gain_g, g.gain_b], dtype=np.float32)

    # 2) 曝光 gamma：只修明显欠曝/过曝，区间内 gamma=1 完全不动
    if abs(g.gamma - 1.0) > 1e-3:
        x = np.power(x, g.gamma)

    # 3) 分区 S 曲线：作用在 luma 上再按比例回贴 → 天然保色相、保通道比例
    l = autograde._luma(x)
    mapped = eval_curve(np.asarray(g.x, np.float32), np.asarray(g.y, np.float32),
                        np.asarray(g.m, np.float32), l)
    x = x * (mapped / np.maximum(l, 1e-4))[..., None]

    # 4) 色温（同样只压不提）
    wm = warm_matrix(g.warmth)
    if not np.allclose(wm, 1.0):
        x = x * wm

    # 5) 饱和度：在 LCh 里固定色相角与明度、只缩放彩度 C（FEAT 候选）。
    #    为什么不用 RGB 线性缩放：实测（`grade_lch_saturation.py`）在饱和色块上
    #    RGB 缩放的 |ΔL| 达 0.585（拉饱和顺带提亮 0.51），LCh 只有 0.025；
    #    色相偏移也从 0.519° 降到 0.726°（在饱和件上）／0.307° → 0.634°（自然照片）。
    #    **仍是逐色纯函数**：C 只依赖单色，能原样烘焙进 LUT。
    if abs(g.saturation - 1.0) > 1e-3:
        x = _saturate_lch(x, g.saturation)

    # 6) 自然饱和度：只提「还不够艳」的颜色，本来就艳的少动。
    #    同样走 LCh（只缩放 C），只是按「当前彩度」给权重 —— 淡色多加、艳色少加。
    if g.vibrance > 1e-3:
        x = _vibrance_lch(x, g.vibrance)

    # 7) 收口：只在**确实**有像素被推过 1.0 时压一次肩部（现行实现无条件压 2~3 次）
    if float(np.max(x)) > 1.0:
        x = autograde._shoulder(x)
    return np.clip(x, 0.0, 1.0)


def build_lut(g: GradeV2, size: int = 33) -> tuple[np.ndarray, int]:
    """把逐色映射预计算成 3D LUT，轴序与 `lutcube` / `.cube` 一致（[b,g,r]）。"""
    n = int(max(9, min(65, size)))
    line = np.linspace(0.0, 1.0, n, dtype=np.float32)
    b, gg, r = np.meshgrid(line, line, line, indexing="ij")
    grid = np.stack([r, gg, b], axis=-1)
    return eval_colors(g, grid).astype(np.float32), n


def grade(img_u8: np.ndarray, style: str = DEFAULT_STYLE, strength: float = 1.0,
          lut_size: int = 33) -> tuple[np.ndarray, GradeV2, tuple[np.ndarray, int]]:
    """一步到位：分析 → 建 LUT → 查表。返回 `(成片, 参数, LUT)`，与 `autograde.grade` 同形。"""
    from ai_studio import lutcube

    g = analyze(img_u8, style=style, strength=strength)
    lut = build_lut(g, size=lut_size)
    return lutcube.apply_cube(img_u8, lut, strength=1.0), g, lut
