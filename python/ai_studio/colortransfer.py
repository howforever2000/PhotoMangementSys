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


def reinhard_transfer_stats(
    src_u8: np.ndarray,
    ref_stats: dict,
    l_weight: float = 1.0,
    strength: float = 1.0,
) -> np.ndarray:
    """用**预先提取的参考图 Lab 统计**做 Reinhard 迁移。

    与逐像素传 ref 的 `reinhard_transfer` 等价，但统计量可以来自多张参考图的
    `merge_reference_stats` 合并结果（多图共性调色的入口）。l_weight<1 时只迁移
    色度、少动亮度（0 = 只迁 a/b，完全保留素材自身明暗）；strength∈[0,1] 为与
    原图的最终混合比例（0 = 原图，1 = 完全迁移）。
    """
    src_lab = rgb_to_lab(src_u8)
    out = src_lab.copy()
    weights = (l_weight, 1.0, 1.0)
    for i in range(3):
        ms, ss = src_lab[..., i].mean(), src_lab[..., i].std() + 1e-6
        mr, sr = ref_stats["mean"][i], ref_stats["std"][i]
        target = (src_lab[..., i] - ms) * (sr / ss) + mr
        out[..., i] = src_lab[..., i] * (1 - weights[i]) + target * weights[i]
    out[..., 0] = np.clip(out[..., 0], 0.0, 100.0)
    transferred = lab_to_rgb(out)
    strength = float(max(0.0, min(1.0, strength)))
    if strength >= 1.0:
        return transferred
    blended = src_u8.astype(np.float32) * (1.0 - strength) + transferred.astype(np.float32) * strength
    return np.clip(blended + 0.5, 0, 255).astype(np.uint8)


def reinhard_transfer(
    src_u8: np.ndarray,
    ref_u8: np.ndarray,
    l_weight: float = 1.0,
    strength: float = 1.0,
) -> np.ndarray:
    """把 ref 的 Lab 均值/标准差迁到 src（多参考图请走 `reinhard_transfer_stats`）。"""
    return reinhard_transfer_stats(src_u8, reference_stats(ref_u8), l_weight=l_weight, strength=strength)


def merge_reference_stats(stats_list: list[dict]) -> dict:
    """把多张参考图的 Lab 统计等权合并成一份「共性」统计。

    合并均值 = 各参考图均值的等权平均；合并方差用全方差公式
    `E[σ²] + Var[μ]`（组内方差均值 + 组间均值偏差），数学上等价于把所有参考图
    的像素混进一个样本池再算统计（忽略各图像素数差异），不引入任何新依赖。
    """
    if not stats_list:
        raise ValueError("没有可合并的参考图统计")
    if len(stats_list) == 1:
        return stats_list[0]
    means = np.asarray([s["mean"] for s in stats_list], dtype=np.float64)
    stds = np.asarray([s["std"] for s in stats_list], dtype=np.float64)
    merged_mean = means.mean(axis=0)
    var_within = (stds.astype(np.float64) ** 2).mean(axis=0)
    var_between = ((means - merged_mean) ** 2).mean(axis=0)
    merged_std = np.sqrt(var_within + var_between)
    return {"mean": merged_mean.tolist(), "std": (merged_std + 1e-6).tolist()}


def stats_divergence(stats_list: list[dict]) -> float:
    """参考图之间的色偏分歧度：a/b 均值两两距离的最大值（Lab 单位，近似 ΔE 的色度分量）。

    供工作流判断「多张参考图色调差异是否过大」（差异过大时合并共性会得到
    不伦不类的中间态，需要提示用户）。
    """
    if len(stats_list) < 2:
        return 0.0
    chroma = np.asarray([s["mean"][1:3] for s in stats_list], dtype=np.float64)
    d = 0.0
    for i in range(len(chroma)):
        for j in range(i + 1, len(chroma)):
            d = max(d, float(np.linalg.norm(chroma[i] - chroma[j])))
    return d


def histogram_ref_sorted(refs) -> list[np.ndarray]:
    """把一张或多张参考图整理成直方图匹配用的**逐通道已排序像素列**。

    接受单个 `np.ndarray` 或 `list[np.ndarray]`；多张参考图直接把像素拼进同一条
    列（等价于混池采样）——直方图匹配只看排序后的分位分布，拼接顺序不影响结果。
    """
    if isinstance(refs, np.ndarray):
        refs = [refs]
    if not refs:
        raise ValueError("没有可用的参考图像素")
    # 先拼后排：多张参考图必须混进**同一个样本池**再排序（各图分别排序再拼接
    # 会破坏整体分位分布，且结果随列表顺序变化）。
    return [np.sort(np.concatenate([r[..., i].ravel() for r in refs])) for i in range(3)]


def histogram_match_sorted(src_u8: np.ndarray, ref_sorted: list[np.ndarray], strength: float = 1.0) -> np.ndarray:
    """逐 RGB 通道直方图匹配，参考侧给 `histogram_ref_sorted` 的已排序列。

    参考侧预排序一次即可整批复用（批量下省掉每张图重排参考像素的开销）。
    strength 与 reinhard 同义（与原图的最终混合比例）。
    """
    out = np.empty_like(src_u8)
    h, w = src_u8.shape[:2]
    for i in range(3):
        s = src_u8[..., i].ravel()
        r_sorted = ref_sorted[i]
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


def histogram_match(src_u8: np.ndarray, ref_u8, strength: float = 1.0) -> np.ndarray:
    """逐 RGB 通道直方图匹配（对照组）。`ref_u8` 可为单张图或图片列表（多图共性）。"""
    return histogram_match_sorted(src_u8, histogram_ref_sorted(ref_u8), strength=strength)


def reference_stats(ref_u8: np.ndarray) -> dict:
    """提取参考图 Lab 的均值/标准差，供「先描述后套用」或导出 .cube 复用。"""
    lab = rgb_to_lab(ref_u8)
    return {
        "mean": [float(lab[..., i].mean()) for i in range(3)],
        "std": [float(lab[..., i].std() + 1e-6) for i in range(3)],
    }


def build_cube_text(ref_u8: np.ndarray, size: int = 33, l_weight: float = 1.0) -> str:
    """把「参考图 Reinhard 迁移」固化成 .cube 文本：采样 size^3 个 sRGB 色点做迁移。"""
    return build_cube_text_from_stats(reference_stats(ref_u8), size=size, l_weight=l_weight)


def build_cube_text_from_stats(ref_stats: dict, size: int = 33, l_weight: float = 1.0) -> str:
    """把「按参考图统计的 Reinhard 迁移」固化成 .cube 文本。

    多参考图时传 `merge_reference_stats` 的合并统计即可导出「共性」滤镜。
    """
    line = np.linspace(0.0, 1.0, size, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")  # 轴序 [b,g,r]
    grid = np.stack([r, g, b], axis=-1)
    grid_u8 = np.clip(grid * 255.0 + 0.5, 0, 255).astype(np.uint8).reshape(-1, 3)
    # 逐色点迁移（全局统计，等价于对整幅图做同一变换）——按 256K 一批控制内存
    out = reinhard_transfer_stats(grid_u8.reshape(1, -1, 3), ref_stats, l_weight=l_weight, strength=1.0)
    out = out.reshape(-1, 3)
    # 迁移产物是 uint8，而 .cube 规范要 0~1 归一化浮点（cube_text_from_flat 不猜量纲）
    return cube_text_from_flat(out.astype(np.float32) / 255.0, size)


def cube_text_from_lut(lut: np.ndarray, size: int) -> str:
    """把已算好的 LUT 数组（[size,size,size,3]，轴序 [b,g,r]）写成 .cube 文本。

    导出时保持 **[b,g,r] 轴序**，并要求 `.cube` 的写出顺序与读取顺序严格互逆：
    读取侧 `lutcube.load_cube` 把扁平数据 reshape 成 [b,g,r]，所以这里按
    `lut.reshape(-1,3)` 原样逐行写出即可 —— 写反会让红蓝互换（见 local-enhance.md §1）。
    """
    flat = np.asarray(lut, dtype=np.float32).reshape(-1, 3)
    return cube_text_from_flat(flat, int(size))


def cube_text_from_flat(flat_rgb: np.ndarray, size: int) -> str:
    """把扁平色表（每行一个 rgb，顺序与 .cube 规范一致）写成 .cube 文本。

    只接受 **0~1 浮点**（.cube 规范就是归一化值）。调用方若是 uint8 请自行 /255，
    这里不做量纲猜测 —— 猜错会静默产出全白或全黑的 LUT。
    """
    scale = float(np.max(flat_rgb)) if flat_rgb.size else 0.0
    if scale > 1.5:
        raise ValueError("cube_text_from_flat 期望 0~1 的浮点色值；uint8 请先除以 255")
    body = "\n".join(f"{v[0]:.6f} {v[1]:.6f} {v[2]:.6f}" for v in flat_rgb)
    return (
        f"TITLE \"generated\"\nLUT_3D_SIZE {int(size)}\n"
        "DOMAIN_MIN 0.0 0.0 0.0\nDOMAIN_MAX 1.0 1.0 1.0\n" + body + "\n"
    )
