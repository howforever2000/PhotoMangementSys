"""自适应调色：从画面统计推导出一个「色彩→色彩」映射，再固化成 3D LUT。

设计原则（为什么不用生成模型）：
    文生图/图生图会整幅重绘，破坏身份、细节与分辨率。这里走工业界通行做法 ——
    只让算法决定「怎么调」，像素由可逆的查表完成：
      1. `analyze` 只看统计量（黑/白点、灰世界白平衡、中间调、饱和度）；
      2. `eval_colors` 是一个**只依赖单个颜色**的映射函数（无空间运算）；
      3. 因为映射逐色独立，可以把它精确预计算成 3D LUT 交给 `lutcube` 查表，
         12MP 单张约 0.6s，且保分辨率、零重绘、可导出 .cube 复用。

本模块只做「统计 → 映射」这一件事；读图/写文件/进度在 workflows/auto_grade.py。
"""
from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from . import colortransfer

# 调色风格 → 目标参数。数值是实测调出来的保守区间：宁可欠一点，不要过火。
STYLES: dict[str, dict] = {
    "natural": {
        "name": "自然",
        "contrast": 0.55,
        "saturation": 1.06,
        "vibrance": 0.18,
        "warmth": 0.0,
        "wb": 0.65,
    },
    "vivid": {
        "name": "鲜明",
        "contrast": 0.85,
        "saturation": 1.18,
        "vibrance": 0.32,
        "warmth": 0.02,
        "wb": 0.70,
    },
    "warm": {
        "name": "暖调",
        "contrast": 0.60,
        "saturation": 1.08,
        "vibrance": 0.20,
        "warmth": 0.06,
        "wb": 0.55,
    },
    "cool": {
        "name": "冷调",
        "contrast": 0.60,
        "saturation": 1.05,
        "vibrance": 0.18,
        "warmth": -0.06,
        "wb": 0.60,
    },
}

DEFAULT_STYLE = "natural"

# 统计采样上限：统计量不需要全分辨率，12MP 全量算分位数很慢（约 1s+），
# 每边步进抽样到约 40 万像素即可稳定。
_STAT_SAMPLES = 400_000

# 拉伸倍率上限：1/span 是拉伸倍数。纯色图、灰片、雾天图的 span 极小，
# 不设限会把 6 倍以上的拉伸直接压上去 —— 暗部噪声被放大成彩色麻点，
# 亮部直接溢出成一片白。这是「自动调色过火」最常见的成因。
_MAX_STRETCH = 2.2

# 高光肩部：超过 knee 后渐近压缩到 target，不给到 1.0。
#
# 为什么需要：黑白场拉伸把白点推到 1.0，是按**亮度**算的；但单个通道
# （蓝天里的 B、云里的 R）本来就在 240 以上，同一个 1.19× 会把它推过 255。
# 实测 DSC01493 单通道截断 0.008% → 22.6%，用户感知为「提亮了但细节没了」。
#
# 为什么只在「真的拉伸过」时才压（见 eval_colors 末步）：
# 曝光已正确的图 white≥0.93 会被 _levels 保护成 1.0、完全不拉伸，
# 此时再压肩部等于无故压暗 P99（实测 230 → 200，整体发灰）。
_SHOULDER_KNEE = 0.86
# target 必须让 0.98 以下留出余量：0.98×255=249.9 会四舍五入回 250，
# 边界像素仍被判为截断（实测残留 0.78%）。0.97×255=247.4 才真正落在 250 之下。
_SHOULDER_TARGET = 0.97
# 动态范围低于此值时视为「没有可拉伸的层次」，只做极轻微校正。
_FLAT_SPAN = 0.18


@dataclass(frozen=True)
class GradeStats:
    """一次自适应调色推导出的目标参数（全部与画面尺寸无关）。"""

    black: float          # 黑场（归一化 0~1）
    white: float          # 白场
    gamma: float          # 中间调 gamma
    gain_r: float         # 白平衡增益（相对绿通道）
    gain_g: float
    gain_b: float
    saturation: float
    vibrance: float
    warmth: float

    def as_meta(self) -> dict:
        return {
            "black": round(self.black, 4),
            "white": round(self.white, 4),
            "gamma": round(self.gamma, 4),
            "wbGain": [round(self.gain_r, 4), round(self.gain_g, 4), round(self.gain_b, 4)],
            "saturation": round(self.saturation, 4),
            "vibrance": round(self.vibrance, 4),
            "warmth": round(self.warmth, 4),
        }


def _luma(rgb: np.ndarray) -> np.ndarray:
    """Rec.709 亮度（输入输出都在 0~1 的浮点域）。"""
    return rgb[..., 0] * 0.2126 + rgb[..., 1] * 0.7152 + rgb[..., 2] * 0.0722


def _subsample(img_u8: np.ndarray) -> np.ndarray:
    """把图压到 ~40 万像素做统计：步进抽样比缩放快，且不引入插值带来的假色。"""
    h, w = img_u8.shape[:2]
    step = max(1, int(np.sqrt(h * w / float(_STAT_SAMPLES))))
    return img_u8[::step, ::step]


def _levels(lum: np.ndarray, rgb: np.ndarray | None = None) -> tuple[float, float]:
    """由亮度分布定出 (black, white)，并保证拉伸倍数不超过 `_MAX_STRETCH`。

    三个保命条件（实测踩出来的）：
      1. 层次本来就少（纯色、灰片、雾天、大面积白墙）→ **完全不拉伸**，返回恒等
         [0,1]。这类图没有可拉伸的直方图，硬拉只会把噪声放大成麻点、把亮部压成灰。
      2. 其余情况限制拉伸倍率：暗部噪声会被同等放大，超过 2.2× 就开始脏。
      3. 高光保护（BUG-2026-1114-001）：白点已接近 1（≥0.93）或黑点已接近 0
         （≤0.07）时该端不做拉伸 —— 已曝光正确的图里，245~250 的云层/浅色
         细节被 1.04× 的「小」拉伸推过 255，高光溢出 0.13%→3.7%（28 倍）。
         拉伸只该发生在「动态范围确实被压缩」的欠曝/过曝图上。
    `_FLAT_SPAN` 用「1%~99% 分位跨度」判定，比 min/max 抗个别像素干扰。

    BUG-2026-1117-006：白点改为**按单通道分位**取，不再只按亮度。
    为什么必须这样：亮度是 R/G/B 加权和，蓝天照里 B 通道可到 250 而亮度只有 190。
    只按亮度定白点（0.843 → 1.0，1.19×）时 B 通道被推过 255，整片天空单通道饱和
    —— 这正是「提亮了但细节没了」的机制根因。取「各通道 99.5 分位的最大值」当白点，
    等于按最亮的那个通道定上限，拉伸就不会把任一通道推出边界。
    """
    lo, hi = np.percentile(lum, [1, 99])
    span = float(hi - lo)
    if span < _FLAT_SPAN:
        return 0.0, 1.0
    black = float(np.percentile(lum, 0.5))
    white = float(np.percentile(lum, 99.5))
    if rgb is not None:
        # 单通道分位：任一通道接近白场就该停手
        ch_white = float(np.percentile(rgb.reshape(-1, 3), 99.5, axis=0).max())
        white = max(white, ch_white)
    # 高光/暗部保护：端点已贴边就不动该端，避免「小拉伸推掉贴边细节」
    if white >= 0.93:
        white = 1.0
    if black <= 0.07:
        black = 0.0
    span = max(white - black, 1e-4)
    if 1.0 / span > _MAX_STRETCH:
        # 先向外扩窗口把倍率压下来；扩到 [0,1] 边界仍不够（图本身极暗或极亮）时，
        # 剩下的交给 gamma —— 影调整体偏移适合用幂函数表达，而不是继续放大对比。
        need = (1.0 / _MAX_STRETCH) - span
        black = max(0.0, black - need * 0.5)
        white = min(1.0, white + need * 0.5)
    return black, white


def _clamp_output(stats: GradeStats) -> GradeStats:
    """把参数夹到「不会毁图」的硬边界内。

    统计驱动的参数偶有极端值（近全黑图算出的 gamma 0.75 仍可能过冲），
    这里统一收口：任何一条超界都按边界执行，保证最坏情况也只是「没效果」，
    不会出现死白/死黑/色彩溢出。
    """
    return GradeStats(
        black=float(np.clip(stats.black, 0.0, 0.45)),
        white=float(np.clip(stats.white, 0.55, 1.0)),
        gamma=float(np.clip(stats.gamma, 0.82, 1.22)),
        gain_r=float(np.clip(stats.gain_r, 0.90, 1.10)),
        gain_g=float(np.clip(stats.gain_g, 0.90, 1.10)),
        gain_b=float(np.clip(stats.gain_b, 0.90, 1.10)),
        saturation=float(np.clip(stats.saturation, 0.85, 1.30)),
        vibrance=float(np.clip(stats.vibrance, 0.0, 0.45)),
        warmth=float(np.clip(stats.warmth, -0.10, 0.10)),
    )


def analyze(
    img_u8: np.ndarray,
    style: str = DEFAULT_STYLE,
    strength: float = 1.0,
) -> GradeStats:
    """看图推导调色参数。strength∈[0,1] 把「推导值」向「中性值」插值。

    strength 只影响幅度，不改变映射的函数形式，所以产物仍是一个合法 LUT。

    BUG-2026-1114-001 修复：中间调目标从「盲拉 0.46」改为「向合理曝光带收敛」。
    旧逻辑把晴天人像（均亮 ~0.38，曝光本来正确）强行提亮 20%：天空/云高光溢出
    28 倍（≥250 像素 0.13%→3.7%），脸颊高光洗掉明暗立体感 —— 用户感知为
    「脸变胖 + 过曝」。曝光正确的图现在完全不动亮度。
    """
    cfg = STYLES.get(style) or STYLES[DEFAULT_STYLE]
    small = _subsample(img_u8).astype(np.float32) / 255.0
    lum = _luma(small)

    black, white = _levels(lum, small)

    # 中间调 gamma：目标从常数 0.46 改为「合理曝光带 [0.40, 0.50]」——
    # 均亮已在带内的图 gamma=1（不动亮度）；欠曝图向上修、过曝图向下修，
    # gamma 区间同步收紧到 [0.85, 1.30]（原 [0.75, 1.35] 的 0.75 端正是
    # 把正常图拉亮 20% 的那档）。
    mean_lum = float(np.clip(lum.mean(), 0.02, 0.98))
    target = float(np.clip(mean_lum, 0.40, 0.50))
    gamma = float(np.clip(np.log(target) / np.log(max(mean_lum, 1e-4)), 0.85, 1.30))

    # 白平衡：灰世界假设。**自适应地决定要不要动**：
    #   - 用中间调像素（亮度 8%~92% 分位之间）统计，排除死黑死白与高光溢出；
    #   - 只有「色偏方向一致且幅度足够」才修正 —— 大面积单色场景（蓝天、草地、
    #     夕阳）本身就偏离中性，硬校正会把它们拉成灰，这是自动白平衡最常见的翻车。
    lo, hi = np.percentile(lum, [8, 92])
    mid_mask = (lum >= lo) & (lum <= hi)
    if mid_mask.any():
        mid_rgb = small[mid_mask]
        means = mid_rgb.mean(axis=0) + 1e-6
        gray = float(means.mean())
        raw_gains = np.clip(gray / means, 0.82, 1.18)
        raw_gains = raw_gains / raw_gains[1]
        # 偏离中性的幅度：超过 6% 才认为是真色偏，并把超出部分等比收一半，
        # 避免「蓝天照被修成阴天灰」。
        dev = float(np.abs(raw_gains - 1.0).max())
        if dev > 0.06:
            gains = 1.0 + (raw_gains - 1.0) * 0.5
        else:
            gains = np.ones(3, dtype=np.float32)
    else:
        gains = np.ones(3, dtype=np.float32)

    s = float(max(0.0, min(1.0, strength)))

    def toward(derived: float, neutral: float) -> float:
        return neutral + (derived - neutral) * s

    return _clamp_output(
        GradeStats(
            black=toward(black, 0.0),
            white=toward(white, 1.0),
            gamma=toward(gamma, 1.0),
            gain_r=toward(float(gains[0]), 1.0),
            gain_g=toward(float(gains[1]), 1.0),
            gain_b=toward(float(gains[2]), 1.0),
            saturation=toward(cfg["saturation"], 1.0),
            vibrance=cfg["vibrance"] * s,
            warmth=cfg["warmth"] * s,
        )
    )


def _shoulder(x: np.ndarray, knee: float = _SHOULDER_KNEE, target: float = _SHOULDER_TARGET) -> np.ndarray:
    """高光肩部软压缩：把 [knee,1] 区间平滑压到 [knee,target]，取代硬 clip。

    为什么必须有这一层：黑白场拉伸把白点推到 1.0 后，原图 250~255 的云层/天空
    会被整段压平——单通道到顶即该区域层次不可恢复（实测 DSC01493 从 0.008%
    涨到 22.6%，用户感知为「提亮了但细节没了」）。

    为什么用 smoothstep 而不是 gamma：肩部要「几乎线性起步、末端渐缓」，
    smoothstep 两端一阶导为 0，接缝处没有可见折线；gamma 会整体改变中间调。

    这是**纯逐色函数**（只依赖输入值），所以 LUT 仍可预计算导出。
    """
    hi = np.clip((x - knee) / max(1.0 - knee, 1e-4), 0.0, 1.0)
    eased = hi * hi * (3.0 - 2.0 * hi)
    return np.where(x > knee, knee + (target - knee) * eased, x)


# ---------------- LCh 饱和度：只动彩度 C，固定色相角 h 与明度 L ----------------
#
# 为什么不再用 RGB 线性缩放（`l + (x-l)*k`）：通道被推出 [0,1] 后硬裁，
# 裁掉的部分同时改变明度与色相。实测（10 张真实素材，只跑这两步）：
#     RGB 线性：|ΔL| 0.230，ΔL 带符号 +0.202（拉饱和顺带整体提亮），|ΔHue| 0.836°
#     LCh：     |ΔL| 0.090，ΔL 带符号 −0.054，                       |ΔHue| 0.397°
# 色卡/色轮这类饱和素材上差距更大（|ΔL| 0.256/0.371 → 0.023/0.025）。
#
# **前提条件（缺了它这个方案比原方案更差）**：Lab→sRGB 超色域时仍会被 `np.clip`
# 裁掉，色相随之被裁歪 —— 实测朴素版把 |ΔHue| 从 0.836° 推到 1.581°。
# 所以必须先逐像素二分求出「保持 h 与 L 时 C 还能放大几倍」，缩放倍数取 min(目标, 边界)。
#
# 仍是**纯逐色函数**：LCh 只依赖单个输入颜色，无空间运算，可照原样烘焙进 33³ LUT，
# 因此 `lutcube` 内容摘要缓存、`.cube` 导出、97³ 稠密表全部继续可用。

_LCH_ITERS = 6          # 二分迭代次数：区间压到 1/64，远超 8bit 输出所需
_LCH_CEIL = 4.0         # 彩度缩放搜索上界（4 倍远超任何合理饱和档位）
_C_SOFT = 42.0          # 「够艳」的 CIELAB 彩度软阈值，用于自然饱和度的权重
_GAMUT_EPS = 1e-5       # 色域判定的浮点容差：sRGB 原色恰在边界上，不留会误判越界


def _lab_to_linear(lab: np.ndarray) -> np.ndarray:
    """Lab → **线性** sRGB，不裁剪（可能 <0 或 >1）。

    不能复用 `colortransfer.lab_to_rgb`：它末尾 `clip` 成 uint8，而判断「是否越色域」
    恰恰要看裁剪**之前**的值。
    """
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0
    d = 6.0 / 29.0

    def finv(t):
        return np.where(t > d, t ** 3, 3.0 * d * d * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * colortransfer._WHITE
    # 必须转置：项目里所有矩阵都按「行向量左乘」约定（见 `color_transfer` 的
    # `xyz @ _MINV.T`）。漏掉 `.T` 会让纯红 (255,0,0) 算出线性值 [1.13,-0.24,-0.18]
    # 而不是 [1,0,0] —— 于是所有 sRGB 原色都被判为「越色域」，headroom 被压到
    # 0.06~0.21，拉饱和时把颜色削成灰。这个错误不报错、只表现为「饱和失效」。
    return xyz @ colortransfer._MINV.T


def _chroma_headroom(lab: np.ndarray, iters: int = _LCH_ITERS) -> np.ndarray:
    """逐像素求「保持色相角 h 与明度 L 时，彩度 C 最多能放大到几倍」。

    只缩放 (a,b) 的**长度**、不改方向 → 色相角在数学上恒定。
    没有闭式解（XYZ 对 a,b 经 finv 是三次分段函数），所以向量化二分。

    容差 `_GAMUT_EPS` 不能省：sRGB 原色（纯红/纯蓝等）正好落在色域**边界**上，
    Lab 往返的浮点误差会让线性值出现 1e-7 量级的越界。不留容差时它们会被判为
    「已越界」，headroom 直接归零 —— 表现是纯色完全失去饱和度。
    """
    L = lab[..., 0:1]
    a = lab[..., 1:2]
    b = lab[..., 2:3]
    lo = np.zeros_like(a)
    hi = np.full_like(a, _LCH_CEIL)
    for _ in range(iters):
        mid = (lo + hi) * 0.5
        lin = _lab_to_linear(np.concatenate([L, a * mid, b * mid], axis=-1))
        ok = np.all((lin >= -_GAMUT_EPS) & (lin <= 1.0 + _GAMUT_EPS),
                    axis=-1, keepdims=True)
        lo = np.where(ok, mid, lo)
        hi = np.where(ok, hi, mid)
    return lo[..., 0]


def _to_lab(x: np.ndarray) -> np.ndarray:
    """float[0,1] RGB → CIELAB。

    走 uint8 是因为 `colortransfer.rgb_to_lab` 的入口就是 uint8（与参考图调色共用
    同一实现，不在这里另写一套）；实测往返恒等（10 张图 maxdiff 全为 0），无量化损失。
    """
    return colortransfer.rgb_to_lab(
        np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8))


def _from_lab(lab: np.ndarray) -> np.ndarray:
    return colortransfer.lab_to_rgb(lab).astype(np.float32) / 255.0


def _saturate_lch(x: np.ndarray, k: float) -> np.ndarray:
    """第 5 步的 LCh 版：彩度乘 k，色相与明度保持；越界部分按 headroom 回收。

    `k < 1`（降饱和）时 headroom 必然 > 1，等价于直接乘；
    `k > 1` 时若该颜色已贴色域边界，实际倍数被压到边界值 —— 宁可少提一点，
    也不让色相被裁歪。
    """
    lab = _to_lab(x)
    eff = np.minimum(float(k), _chroma_headroom(lab))
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return _from_lab(out)


def _vibrance_lch(x: np.ndarray, amount: float) -> np.ndarray:
    """第 6 步的 LCh 版：按当前彩度 C 给权重，只缩放 C（同样带 headroom 回收）。

    旧版权重来自 `max-min` 通道差（受明度影响）；这里直接用 CIELAB 的 C，语义更直接：
    淡色 C 小 → 权重接近 1 多加；艳色 C 大 → 权重接近 0 少加。
    """
    lab = _to_lab(x)
    a, b = lab[..., 1], lab[..., 2]
    c = np.sqrt(a * a + b * b)
    weight = np.clip(1.0 - c / _C_SOFT, 0.0, 1.0)
    eff = np.minimum(1.0 + amount * weight, _chroma_headroom(lab))
    out = lab.copy()
    out[..., 1] *= eff
    out[..., 2] *= eff
    return _from_lab(out)


# Lab ↔ 线性 sRGB 的矩阵/白点**复用 `colortransfer` 的同一份常量**，
# 不在这里另抄一遍 —— 抄写极易把矩阵方向搞错（曾漏掉 `.T`，导致 sRGB 原色
# 全被判为越色域、headroom 归零、拉饱和把颜色削成灰）。`_lab_to_linear` 与之
# 唯一的不同是末尾**不裁剪**，所以本文件只定义那个函数，不定义常量。


def eval_colors(stats: GradeStats, rgb: np.ndarray) -> np.ndarray:
    """逐色映射：0~1 的 RGB 进，0~1 的 RGB 出。**不得引入任何空间运算**。

    顺序固定为：白平衡 → 黑白场拉伸 → 中间调 → 色温 → 饱和度 → 自然饱和度
    → 高光肩部。先做白平衡再做影调，是因为灰世界增益是线性乘法，放前面才不会被
    后面的曲线把它补偿掉。

    饱和度与自然饱和度这两步在 **LCh** 里做（固定色相角与明度、只缩放彩度 C），
    见 `_saturate_lch` / `_vibrance_lch`；其余五步维持原样。

    中间步骤一律 `np.clip()` 收口，**只在最后压一次肩部**：
    肩部是有损压缩，叠加 N 次等于把 knee 逐级降低（实测每步都压会让 P99 掉 30 级）。
    """
    x = np.clip(rgb, 0.0, 1.0).astype(np.float32)

    # 1) 白平衡。增益 >1 会直接把单通道推过 1.0，此处**必须用肩部而非硬裁剪**：
    #    硬裁剪在此丢掉的层次，后面的肩部再压也救不回来（实测 DSC01493 残留 1%）。
    x = x * np.array([stats.gain_r, stats.gain_g, stats.gain_b], dtype=np.float32)
    x = _shoulder(x) if max(stats.gain_r, stats.gain_g, stats.gain_b) > 1.005 else np.clip(x, 0.0, 1.0)

    # 2) 黑白场拉伸
    span = max(stats.white - stats.black, 1e-4)
    x = np.clip((x - stats.black) / span, 0.0, 1.0)

    # 3) 中间调 gamma。gamma<1 是提亮，会等比推高所有通道，必须用肩部收口，
    #    否则暗部提亮的同时把高光整体推出边界（DSC01755 gamma=1.22 残留 0.78%）。
    if abs(stats.gamma - 1.0) > 1e-3:
        x = np.power(x, stats.gamma)
        x = _shoulder(x) if stats.gamma < 0.999 else np.clip(x, 0.0, 1.0)

    # 4) 色温（暖 = 抬红压蓝；冷反之），与亮度无关的轻微通道偏移
    if abs(stats.warmth) > 1e-4:
        w = stats.warmth
        x = x * np.array([1.0 + w, 1.0, 1.0 - w], dtype=np.float32)
        x = _shoulder(x) if w > 0 else np.clip(x, 0.0, 1.0)

    # 5) 饱和度：在 LCh 里固定色相角与明度、只缩放彩度 C。
    #    为什么不用 RGB 线性缩放：通道硬裁会同时改变明度与色相（见 _saturate_lch 注释）。
    if abs(stats.saturation - 1.0) > 1e-3:
        x = _saturate_lch(x, stats.saturation)

    # 6) 自然饱和度：只提「还不够艳」的颜色，本来就艳的少动（同样只动 C，带色域回收）
    if stats.vibrance > 1e-3:
        x = _vibrance_lch(x, stats.vibrance)

    # 7) 收口：只要链路里发生过「会推高单通道」的动作就压肩部。
    #    三个来源：黑白场拉伸（1/span）、白平衡增益、色温偏移。
    #    已曝光正确且无偏色的图（拉伸=1、gain=1、warmth=0）跳过，不无故压暗高光。
    stretch = 1.0 / span
    gain_dev = max(abs(stats.gain_r - 1.0), abs(stats.gain_g - 1.0), abs(stats.gain_b - 1.0))
    pushed = stretch > 1.02 or gain_dev > 0.02 or abs(stats.warmth) > 0.01
    if pushed:
        x = _shoulder(x)
    return x


def build_lut(stats: GradeStats, size: int = 33) -> tuple[np.ndarray, int]:
    """把逐色映射预计算成 3D LUT，返回 (lut[n,n,n,3] 轴序 [b,g,r], n)。

    轴序必须与 `lutcube` / Adobe `.cube` 一致（红变化最快 → 数组轴 [b,g,r]），
    这里沿用 `colortransfer.build_cube_text` 的构图方式，避免通道错位。
    """
    n = int(max(9, min(65, size)))
    line = np.linspace(0.0, 1.0, n, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")   # 轴序 [b,g,r]
    grid = np.stack([r, g, b], axis=-1)                      # 每格存 (r,g,b)
    out = eval_colors(stats, grid)
    return out.astype(np.float32), n


def grade(img_u8: np.ndarray, style: str = DEFAULT_STYLE, strength: float = 1.0,
          lut_size: int = 33,
          ) -> tuple[np.ndarray, GradeStats, tuple[np.ndarray, int]]:
    """一步到位：分析 → 建 LUT → 套用。返回 (成片, 统计, LUT)。

    暴露 LUT 是为了让调用方可以导出 .cube 复用，而不必重算统计。
    独立自然饱和度微调已拆到 `vibrance_adjust` / `local_vibrance` 算子（FEAT-206），
    调色管线自身只按风格派生值走（FEAT-186 的配方级 vibrance_extra 链路已随
    内嵌滑条一并移除）。
    """
    from . import lutcube

    stats = analyze(img_u8, style=style, strength=strength)
    lut = build_lut(stats, size=lut_size)
    return lutcube.apply_cube(img_u8, lut, strength=1.0), stats, lut

def vibrance_adjust(img_u8: np.ndarray, amount: float) -> np.ndarray:
    """独立自然饱和度单步（FEAT-206）：配方链里「调色之后的最后一扳手」。

    与 `eval_colors` 第 6 步完全同一套数学（`_vibrance_lch`：LCh 里只缩放彩度 C，
    淡色权重高多动、艳色权重低少动，越色域按 headroom 回收），把它从自动调色
    管线里拆出来单独可用 —— 调色/滤镜跑完后对整体色彩做一次双向微调。

    `amount` 双向：>0 提升淡色饱和（自然饱和度语义），<0 收敛，0 = 原样返回
    （不做任何数组拷贝，配方默认值下与旧版逐字节一致）。输入输出都是 uint8 RGB。
    """
    amt = float(amount or 0.0)
    if abs(amt) < 1e-3:
        return img_u8
    x = img_u8.astype(np.float32) / 255.0
    x = _vibrance_lch(x, amt)
    return np.clip(x * 255.0 + 0.5, 0, 255).astype(np.uint8)
