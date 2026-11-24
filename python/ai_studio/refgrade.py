"""参考图调色·三层解耦（FEAT-219）。

把「像不像参考图」拆成三个互不越权的层，全部是纯 numpy 的逐色映射：

  A1 影调层：按 L 分位对齐参考图的曲线**形状**（默认扣掉中位差，只搬形状不搬曝光）；
  A2 分区色偏：按 10 个 L 分带取 (Δa, Δb)，表达「暖高光 / 冷阴影」这类整体风格；
  A3 同色残差：12 个色相扇区 × 3 个影调层，表达「这里的蓝应该更青一点」这类同色换色。

A4/A5/A6 是收口：肤色门控、色域回收（只缩 C）、高光肩部（按亮度等比缩放，保色相）。
整条链路没有任何空间运算，因此可以原样烘焙进 33³ LUT（轴序 [b,g,r]）。

与 `colortransfer.py` 的关系：只复用它的 Lab↔sRGB 矩阵常量与 `.cube` 写出，
不修改任何旧函数；`reinhard` / `histogram` 旧路径保持逐字节不变。
"""
from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from . import autograde, colortransfer, lutcube

# ---------------- 常量（全部来自 lab/colortransfer 七轮实验，改动必须重跑基准） ----------------

# A1：分位点与限幅。分位差先逐点裁到 ±22°，再取前缀最大值强制曲线单调（不反转影调）。
_KNOTS = np.array([2.0, 10.0, 25.0, 50.0, 75.0, 90.0, 98.0], dtype=np.float64)
_TONE_CAP = 22.0

# A2：10 个等宽 L 带，向量限幅 30（Lab 色度距离，约等于 ΔE 的 ab 分量）。
_BANDS = 10
_BAND_CENTERS = np.arange(5.0, 100.0, 10.0, dtype=np.float32)
_CHROMA_CAP = 30.0

# A3：12 个 30° 色相扇区 × 3 个影调层；残差限幅 12、总权重 0.45。
_HUE_SECTORS = 12
_ZONE_CENTERS = np.array([16.5, 49.5, 83.0], dtype=np.float32)
_ZONE_HALF = 33.5
_RES_CHROMA_CAP = 12.0
_RES_L_CAP = 12.0
_RES_WEIGHT = 0.45
_MIN_CELL_FRAC = 0.002

# 抽样上限：统计量与分辨率无关；200k 足以让 36 个格子的最小样本量稳定。
_REF_SAMPLE = 90_000
_SRC_SAMPLE = 200_000

# A6：高光肩部。knee 从 autograde 的 0.86 抬到 0.88，只压顶端 12%。
_SHOULDER_KNEE = 0.88
_SHOULDER_TARGET = 0.97
# 逐通道兜底只压「已经接近 255」的通道：0.95 以下不碰，避免把正常亮部拉灰。
# 实测只判 >1 不够——线性转 sRGB 时越色域通道已被裁成 1.0，而大量 250~254
# 的贴顶像素仍会被 clip% 计入（清橙组 15%）。0.95 起的窄肩部能收掉它们。
_SAFE_KNEE = 0.95

# A4：肤色门控（CIELAB 近似范围）。边界 ±5° / ±5 C / ±5 L 用 smoothstep 过渡。
_SKIN_H = (15.0, 25.0, 55.0, 65.0)
_SKIN_C = (3.0, 8.0, 40.0, 50.0)
_SKIN_L = (30.0, 40.0, 90.0, 100.0)
_SKIN_KEEP = 0.35   # 肤色区域最多保留 35% 的色偏（即压掉 65%）


@dataclass(frozen=True)
class RefProfile:
    """参考侧的一次性产物：整批素材复用，不随素材变化。"""

    samples_lab: np.ndarray          # (N,3) 簇内池化 Lab 样本
    band_ab: np.ndarray              # (10,2) 池化带均值
    kept: tuple[int, ...]            # 参与池化的参考图下标
    dropped: tuple[int, ...]         # 因色调分歧被剔除的参考图下标


@dataclass(frozen=True)
class Mapping:
    """一张素材的映射（逐色函数），可直接烘 LUT。"""

    tone_xp: np.ndarray              # A1 分位 x（已去重、单调）
    tone_fp: np.ndarray              # A1 位移（已锚定 + toneWeight）
    band_da: np.ndarray              # (10,) A2 Δa（已限幅 + chromaWeight）
    band_db: np.ndarray              # (10,) A2 Δb
    residual: np.ndarray             # (12,3,3) A3 残差（已平滑/限幅/含 0.45 权重）
    strength: float = 1.0
    protect_skin: bool = False
    shoulder_mode: str = "ratio"     # "ratio"（等比保色相）或 "channel"（旧逐通道）
    tone_weight: float = 0.85
    chroma_weight: float = 1.0


# ---------------- 基础工具 ----------------


def _sample_lab(img_u8: np.ndarray, max_n: int) -> np.ndarray:
    """在 RGB 域先抽样、再转 Lab：统计量不需要全分辨率，12MP 全量转 Lab 要 1s+。"""
    rgb = np.asarray(img_u8, dtype=np.uint8).reshape(-1, 3)
    n = rgb.shape[0]
    if n > max_n:
        rgb = np.ascontiguousarray(rgb[:: int(np.ceil(n / max_n))], dtype=np.uint8)
    lab = colortransfer.rgb_to_lab(rgb.reshape(1, -1, 3))[0]
    return np.ascontiguousarray(lab, dtype=np.float32)


def _smooth_bands(v: np.ndarray, passes: int = 1) -> np.ndarray:
    """沿 L 带轴做 (1,2,1)/4 平滑，边缘复制（不把两端拉向 0）。"""
    for _ in range(passes):
        p = np.pad(v, ((1, 1), (0, 0)), mode="edge")
        v = (p[:-2] + 2.0 * p[1:-1] + p[2:]) / 4.0
    return v


def _band_stats(samples_lab: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """按 L 的 10 个等宽带求 (mean_a, mean_b)；空带用有效带线性插值补齐。"""
    L = samples_lab[:, 0]
    idx = np.clip((L / 10.0).astype(np.int16), 0, _BANDS - 1)
    counts = np.bincount(idx, minlength=_BANDS).astype(np.float64)
    sums = [
        np.bincount(idx, weights=samples_lab[:, 1], minlength=_BANDS),
        np.bincount(idx, weights=samples_lab[:, 2], minlength=_BANDS),
    ]
    vals = np.zeros((_BANDS, 2), dtype=np.float64)
    valid = counts > 0
    if valid.any():
        src = np.nonzero(valid)[0]
        for j, s in enumerate(sums):
            vals[:, j] = np.interp(np.arange(_BANDS), src, s[valid] / counts[valid])
    return _smooth_bands(vals).astype(np.float32), counts


def _largest_cluster(band_abs: list[np.ndarray], threshold: float) -> list[int]:
    """按「各 L 带 Δab 的均值」做并查集聚类，返回最大簇（平局取下标最小者）。"""
    n = len(band_abs)
    parent = list(range(n))

    def find(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i in range(n):
        for j in range(i + 1, n):
            d = float(np.linalg.norm(band_abs[i] - band_abs[j], axis=1).mean())
            if d <= threshold:
                ri, rj = find(i), find(j)
                if ri != rj:
                    parent[rj] = ri

    groups: dict[int, list[int]] = {}
    for i in range(n):
        groups.setdefault(find(i), []).append(i)
    # 最大簇；同大小取首元素更小者（先选者胜）
    return max(groups.values(), key=lambda g: (len(g), -g[0]))


# ---------------- A1 影调层 ----------------


def _tone_curve(
    src_lab: np.ndarray,
    ref_lab: np.ndarray,
    tone_weight: float,
    keep_exposure: bool,
) -> tuple[np.ndarray, np.ndarray]:
    """分位对齐曲线：只搬参考图的影调形状，默认锚定素材中位亮度。"""
    qs = np.percentile(src_lab[:, 0], _KNOTS)
    qr = np.percentile(ref_lab[:, 0], _KNOTS)
    d = np.maximum.accumulate(np.clip(qr - qs, -_TONE_CAP, _TONE_CAP))
    xp = np.concatenate(([0.0], qs, [100.0]))
    # 端点直接用最近的位移值（而不是减半）：减半会在 d 为正/为负时破坏单调，
    # 0~2% 与 98~100% 的平段肉眼不可见，单调性优先。
    fp = np.concatenate(([float(d[0])], d, [float(d[-1])]))
    # 分位可能重复（纯色/灰片），保留第一个，保证 np.interp 的 x 严格递增
    keep = np.concatenate(([True], np.diff(xp) > 1e-6))
    xp, fp = xp[keep], fp[keep]
    if keep_exposure and xp.size:
        anchor = float(np.interp(float(np.median(src_lab[:, 0])), xp, fp))
        fp = fp - anchor
    fp = fp * float(np.clip(tone_weight, 0.0, 1.0))
    return xp.astype(np.float32), fp.astype(np.float32)


# ---------------- A3 同色残差 ----------------


def _cell_index(samples_lab: np.ndarray) -> np.ndarray:
    """(色相扇区 0..11, 影调层 0..2) → 0..35。"""
    a, b, L = samples_lab[:, 1], samples_lab[:, 2], samples_lab[:, 0]
    hue = np.degrees(np.arctan2(b, a)) % 360.0
    sector = np.clip((hue // 30.0).astype(np.int16), 0, _HUE_SECTORS - 1)
    zone = np.clip((L // 33.0).astype(np.int16), 0, 2)
    return (sector * 3 + zone).astype(np.int32)


def _shift_cell(grid: np.ndarray, known: np.ndarray, dh: int, dz: int):
    """在 (色相, 影调) 网格上取邻居：色相循环、影调边缘复制。"""
    g = np.roll(grid, dh, axis=0)
    k = np.roll(known, dh, axis=0)
    if dz:
        g2 = np.empty_like(g)
        k2 = np.empty_like(k)
        if dz < 0:
            g2[:, 1:, :] = g[:, :-1, :]
            g2[:, 0, :] = g[:, 0, :]
            k2[:, 1:] = k[:, :-1]
            k2[:, 0] = k[:, 0]
        else:
            g2[:, :-1, :] = g[:, 1:, :]
            g2[:, -1, :] = g[:, -1, :]
            k2[:, :-1] = k[:, 1:]
            k2[:, -1] = k[:, -1]
        return g2, k2
    return g, k


def _residual_grid(src_lab: np.ndarray, ref_lab: np.ndarray) -> np.ndarray:
    """36 格同色残差：格内 mean(ref Lab) - mean(src Lab)，空格邻域外推后平滑。"""
    min_count = max(64, int(_MIN_CELL_FRAC * src_lab.shape[0]))
    s_idx = _cell_index(src_lab)
    r_idx = _cell_index(ref_lab)
    # 近中性像素的色相不可信，拟合时排除（应用时另有彩度门控）
    s_mask = np.hypot(src_lab[:, 1], src_lab[:, 2]) >= 4.0
    r_mask = np.hypot(ref_lab[:, 1], ref_lab[:, 2]) >= 4.0

    s_counts = np.bincount(s_idx[s_mask], minlength=36).astype(np.float64)
    r_counts = np.bincount(r_idx[r_mask], minlength=36).astype(np.float64)
    delta = np.zeros((36, 3), dtype=np.float64)
    for comp in range(3):
        s_sum = np.bincount(s_idx[s_mask], weights=src_lab[s_mask, comp], minlength=36)
        r_sum = np.bincount(r_idx[r_mask], weights=ref_lab[r_mask, comp], minlength=36)
        s_mean = s_sum / np.maximum(s_counts, 1.0)
        r_mean = r_sum / np.maximum(r_counts, 1.0)
        delta[:, comp] = r_mean - s_mean

    known = ((s_counts >= min_count) & (r_counts >= min_count)).reshape(_HUE_SECTORS, 3)
    grid = delta.reshape(_HUE_SECTORS, 3, 3).copy()
    grid[~known] = 0.0

    # 空格外推：60 轮把已知格的值扩散到邻居（只填未知格，已知格不被覆盖）
    for _ in range(60):
        if known.all():
            break
        acc = np.zeros_like(grid)
        cnt = np.zeros((_HUE_SECTORS, 3), dtype=np.float64)
        for dh in (-1, 0, 1):
            for dz in (-1, 0, 1):
                if dh == 0 and dz == 0:
                    continue
                sg, sk = _shift_cell(grid, known, dh, dz)
                acc += sg * sk[..., None]
                cnt += sk
        new = (~known) & (cnt > 0)
        grid[new] = acc[new] / cnt[new][..., None]
        known |= new

    # 平滑：色相轴循环 3 次，影调轴边缘复制 2 次（软过渡，避免层边界台阶）
    for _ in range(3):
        p = np.concatenate((grid[-1:], grid, grid[:1]), axis=0)
        grid = (p[:-2] + 2.0 * p[1:-1] + p[2:]) / 4.0
    for _ in range(2):
        p = np.pad(grid, ((0, 0), (1, 1), (0, 0)), mode="edge")
        grid = (p[:, :-2] + 2.0 * p[:, 1:-1] + p[:, 2:]) / 4.0

    # 限幅：Δab 模长 ≤12、ΔL ∈[-12,12]，再乘总权重 0.45
    ab = grid[..., 1:]
    norm = np.linalg.norm(ab, axis=-1, keepdims=True)
    ab *= np.minimum(1.0, _RES_CHROMA_CAP / np.maximum(norm, 1e-6))
    grid[..., 1:] = ab
    grid[..., 0] = np.clip(grid[..., 0], -_RES_L_CAP, _RES_L_CAP)
    return (grid * _RES_WEIGHT).astype(np.float32)


# ---------------- 构建与拟合 ----------------


def build_profile(refs_u8: list[np.ndarray], cluster_threshold: float = 12.0) -> RefProfile:
    """参考侧一次性产物：抽样 → 分歧聚类取最大簇 → 池化样本与带均值。"""
    if not refs_u8:
        raise ValueError("没有可用的参考图像素")
    labs = [_sample_lab(r, _REF_SAMPLE) for r in refs_u8]
    if len(labs) == 1:
        kept = [0]
    else:
        band_abs = [_band_stats(lab)[0] for lab in labs]
        kept = _largest_cluster(band_abs, float(cluster_threshold))
    dropped = [i for i in range(len(labs)) if i not in kept]
    pooled = np.ascontiguousarray(np.concatenate([labs[i] for i in kept], axis=0), dtype=np.float32)
    band_ab, _ = _band_stats(pooled)
    return RefProfile(pooled, band_ab, tuple(kept), tuple(dropped))


def fit_mapping(
    src_u8: np.ndarray,
    profile: RefProfile,
    *,
    tone_weight: float = 0.85,
    chroma_weight: float = 1.0,
    keep_exposure: bool = True,
    protect_skin: bool = False,
    strength: float = 1.0,
    shoulder_mode: str = "ratio",
) -> Mapping:
    """用一张素材的 L 统计拟合三层参数（不碰像素，只算统计）。"""
    if profile.samples_lab.size == 0:
        raise ValueError("参考图样本为空")
    src_lab = _sample_lab(src_u8, _SRC_SAMPLE)
    tone_xp, tone_fp = _tone_curve(src_lab, profile.samples_lab, tone_weight, keep_exposure)

    src_band, _ = _band_stats(src_lab)
    d = (profile.band_ab - src_band) * float(np.clip(chroma_weight, 0.0, 1.0))
    norm = np.linalg.norm(d, axis=1)
    d *= np.minimum(1.0, _CHROMA_CAP / np.maximum(norm, 1e-6))[:, None]

    residual = _residual_grid(src_lab, profile.samples_lab)
    return Mapping(
        tone_xp=tone_xp,
        tone_fp=tone_fp,
        band_da=np.ascontiguousarray(d[:, 0], dtype=np.float32),
        band_db=np.ascontiguousarray(d[:, 1], dtype=np.float32),
        residual=residual,
        strength=float(np.clip(strength, 0.0, 1.0)),
        protect_skin=bool(protect_skin),
        shoulder_mode=str(shoulder_mode),
        tone_weight=float(np.clip(tone_weight, 0.0, 1.0)),
        chroma_weight=float(np.clip(chroma_weight, 0.0, 1.0)),
    )


# ---------------- 应用（逐色） ----------------


def _ramp(x: np.ndarray, lo: float, hi: float) -> np.ndarray:
    t = np.clip((x - lo) / max(hi - lo, 1e-6), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def _skin_membership(lab: np.ndarray) -> np.ndarray:
    """肤色软隶属度（0~1）：h 20–60°、C 8–45、L 35–95，边界 ±5 过渡。"""
    L, a, b = lab[:, 0], lab[:, 1], lab[:, 2]
    c = np.hypot(a, b)
    h = np.degrees(np.arctan2(b, a)) % 360.0
    return (
        _ramp(h, _SKIN_H[0], _SKIN_H[1])
        * (1.0 - _ramp(h, _SKIN_H[2], _SKIN_H[3]))
        * _ramp(c, _SKIN_C[0], _SKIN_C[1])
        * (1.0 - _ramp(c, _SKIN_C[2], _SKIN_C[3]))
        * _ramp(L, _SKIN_L[0], _SKIN_L[1])
        * (1.0 - _ramp(L, _SKIN_L[2], _SKIN_L[3]))
    )


def _lab_to_rgb01(lab: np.ndarray) -> np.ndarray:
    """Lab → sRGB float[0,1]；复用 autograde 的**不裁剪**线性转换，避免色域判断失真。"""
    return np.clip(colortransfer._linear_to_srgb(autograde._lab_to_linear(lab)), 0.0, 1.0)


def _shoulder_ratio(rgb: np.ndarray, knee: float = _SHOULDER_KNEE, target: float = _SHOULDER_TARGET) -> np.ndarray:
    """按亮度等比缩放的高光肩部：保色相/饱和比例，避免逐通道压缩把亮部拉灰。"""
    y = 0.2126 * rgb[:, 0] + 0.7152 * rgb[:, 1] + 0.0722 * rgb[:, 2]
    y2 = autograde._shoulder(y, knee, target)
    f = np.where(y > knee, y2 / np.maximum(y, 1e-6), 1.0)
    out = rgb * f[:, None]
    # 兜底：只对已贴近 255 的通道做窄肩部（0.95 起），不把正常亮部拉灰。
    return np.where(out >= _SAFE_KNEE, autograde._shoulder(out, _SAFE_KNEE, target), out)


def eval_mapping(m: Mapping, rgb01: np.ndarray, strength: float | None = None) -> np.ndarray:
    """把 Mapping 作用到 0~1 RGB 上；纯逐色，无任何空间运算。"""
    x = np.asarray(rgb01, dtype=np.float32)
    shape = x.shape
    flat = x.reshape(-1, 3)
    s = m.strength if strength is None else float(np.clip(strength, 0.0, 1.0))
    if s <= 0.0:
        return x.copy()

    lab = colortransfer.rgb_to_lab(np.clip(flat * 255.0 + 0.5, 0, 255).astype(np.uint8))
    L, a, b = lab[:, 0], lab[:, 1], lab[:, 2]
    c = np.hypot(a, b)

    # A1 影调
    dL = np.interp(L, m.tone_xp, m.tone_fp).astype(np.float32)

    # A2 分区色偏
    da = np.interp(L, _BAND_CENTERS, m.band_da).astype(np.float32)
    db = np.interp(L, _BAND_CENTERS, m.band_db).astype(np.float32)

    # A3 同色残差：色相双扇区线性插值 × 影调三角核（partition of unity）
    hue = np.degrees(np.arctan2(b, a)) % 360.0
    sec = np.floor(hue / 30.0).astype(np.int16) % _HUE_SECTORS
    frac = (hue / 30.0 - np.floor(hue / 30.0)).astype(np.float32)
    r = m.residual[sec] * (1.0 - frac)[:, None, None] + m.residual[(sec + 1) % _HUE_SECTORS] * frac[:, None, None]
    zw = np.maximum(0.0, 1.0 - np.abs(L[:, None] - _ZONE_CENTERS[None, :]) / _ZONE_HALF)
    zw /= np.maximum(zw.sum(axis=1, keepdims=True), 1e-6)
    d = (r * zw[:, :, None]).sum(axis=1)

    # 彩度门控：近中性少动（下限 0.3、C≥6 饱和），防止灰底被染色
    gate = np.clip(c / 6.0, 0.3, 1.0)
    # 肤色门控：只收 A2/A3 的色度，不动 A1 影调
    ab_gate = np.ones_like(gate)
    if m.protect_skin:
        ab_gate = 1.0 - (1.0 - _SKIN_KEEP) * _skin_membership(lab)

    tgt = np.stack(
        [
            np.clip(L + dL + d[:, 0] * gate, 0.0, 100.0),
            a + (da + d[:, 1] * gate) * ab_gate,
            b + (db + d[:, 2] * gate) * ab_gate,
        ],
        axis=-1,
    )

    # A5 色域回收：只缩彩度 C，不改色相角与明度
    cf = np.minimum(1.0, autograde._chroma_headroom(tgt))
    tgt[:, 1] *= cf
    tgt[:, 2] *= cf

    out_lab = lab * (1.0 - s) + tgt * s
    rgb = _lab_to_rgb01(out_lab)

    # A6 高光肩部
    if m.shoulder_mode == "channel":
        rgb = autograde._shoulder(rgb, _SHOULDER_KNEE, _SHOULDER_TARGET)
    else:
        rgb = _shoulder_ratio(rgb, _SHOULDER_KNEE, _SHOULDER_TARGET)
    return np.clip(rgb, 0.0, 1.0).reshape(shape)


def build_lut(m: Mapping, size: int = 33) -> tuple[np.ndarray, int]:
    """把逐色映射烘成 3D LUT，返回 (lut[n,n,n,3] 轴序 [b,g,r], n)。"""
    n = int(max(9, min(65, size)))
    line = np.linspace(0.0, 1.0, n, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")
    grid = np.stack([r, g, b], axis=-1).astype(np.float32)
    return eval_mapping(m, grid).astype(np.float32), n


def transfer(
    src_u8: np.ndarray,
    profile: RefProfile,
    *,
    strength: float = 1.0,
    **fit_kw,
) -> tuple[np.ndarray, np.ndarray, int]:
    """一步到位：拟合 → 烘 33³ LUT → 查表。返回 (成片, lut, n)。"""
    m = fit_mapping(src_u8, profile, strength=strength, **fit_kw)
    lut, n = build_lut(m, size=33)
    if float(np.clip(strength, 0.0, 1.0)) <= 0.0:
        return np.array(src_u8, dtype=np.uint8, copy=True), lut, n
    return lutcube.apply_cube(src_u8, (lut, n), strength=1.0), lut, n
