"""3D LUT（.cube）加载、三线性套用与轻调色效果（纯 numpy / Pillow，无新依赖）。

只做「颜色查表 + 胶片辅助效果」这一件事：
  - load_cube      解析 Adobe .cube（3D）
  - apply_cube     查表套用，支持 strength 与原图混合
  - apply_grain    叠加胶片颗粒
  - apply_vignette 叠加暗角

两种查表实现并存，各有明确用途：
  - `_apply_nearest`（生产路径）：把源 LUT 先重采样成高密度常量表，再单次 gather。
    全程无浮点运算 —— 索引走 256 项预计算表、输出表已量化成 uint8，
    24MP 从 6.0s（逐像素三线性）降到 ~0.29s，与三线性的平均通道误差 <1.1。
  - `_apply_trilinear`（参考实现）：教科书三线性，作为精度基准与回归对照保留。

.cube 数据顺序遵循 Adobe 规范：红色(R)变化最快，其次绿(G)，再次蓝(B)，
即扁平索引 k = r + N*g + N^2*b，reshape 后数组轴序为 [b, g, r]。
本模块不依赖 cv2，可被 ai_studio 各工作流复用。
"""
from __future__ import annotations

import os
from typing import Tuple

import numpy as np

# 3D LUT 立方体缓存：路径 -> (lut, size)，避免同一批照片反复解析同一个 .cube
_CACHE: dict[str, Tuple[np.ndarray, int]] = {}

# 高密度查表缓存：(id(lut), n) -> (lut 引用, 扁平常量表, 边长)
# 存 lut 引用是为了校验身份：只按 id 做 key 时 GC 后 id 可能被复用，会命中别的表。
#
# 只缓存「路径 LUT」（内置 .cube，路径稳定、一批照片反复用）。运行时生成的 LUT
# （autograde 每张照片新建）只查一次就丢，缓存它们等于每次泄漏一张 11MB 表 ——
# 这是本模块唯一的内存无界点，必须靠 `apply_cube` 的 cacheable=False 关掉。
_DENSE_CACHE: dict[Tuple[int, int], Tuple[np.ndarray, np.ndarray, int]] = {}
# 缓存条数上限：内置 LUT 只有 7 个，正常永远到不了；纯粹防未来新增路径 LUT 无界。
_DENSE_CACHE_MAX = 24
# 恒等表缓存：边长 -> 扁平常量表
_IDENTITY_CACHE: dict[int, np.ndarray] = {}
# 量化后的高密度表缓存：(id(lut), n, strength) -> (lut 引用, uint8 扁平表, 边长)
_DENSE_U8_CACHE: dict[Tuple[int, int, float], Tuple[np.ndarray, np.ndarray, int]] = {}
# 索引表缓存：边长 -> 256 项 uint8→网格索引映射
_INDEX_CACHE: dict[int, np.ndarray] = {}


def load_cube(path: str) -> Tuple[np.ndarray, int]:
    """解析 .cube，返回 (lut[ n,n,n,3 ] 轴序 [b,g,r], n)。"""
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
            if head.startswith(
                ("TITLE", "DOMAIN_MIN", "DOMAIN_MAX", "LUT_1D_SIZE", "LUT_3D_DATA")
            ):
                continue
            parts = line.split()
            if len(parts) == 3:
                try:
                    rows.append((float(parts[0]), float(parts[1]), float(parts[2])))
                except ValueError:
                    pass
    data = np.asarray(rows, dtype=np.float32)
    if data.ndim != 2 or data.shape[0] == 0:
        raise ValueError(f"LUT 内容为空或格式不正确: {path}")
    n = size or int(round(len(data) ** (1.0 / 3.0)))
    if data.shape[0] != n ** 3:
        raise ValueError(f"LUT 数据量 {data.shape[0]} 与尺寸 {n}^3 不符: {path}")
    return data.reshape(n, n, n, 3), n


def get_cube(path: str) -> Tuple[np.ndarray, int]:
    """带缓存的 load_cube。"""
    lut = _CACHE.get(path)
    if lut is None:
        lut = load_cube(path)
        _CACHE[path] = lut
    return lut


def apply_cube(img_u8: np.ndarray, path_or_lut, strength: float = 1.0) -> np.ndarray:
    """把 sRGB 图片(uint8 HxWx3)套用 LUT，返回 uint8。strength∈[0,1] 与原图混合。

    快速路径：把源 LUT 重采样成 `_DENSE`³ 常量表后单次 gather（建表按 LUT 缓存）。
    strength<1 的混合在 LUT 域完成 —— 与像素域混合数学等价（三线性是线性的），
    精度差只来自离散化，实测真实照片平均通道误差 <1.1。
    需要 LUT 文件时传路径；需要运行时生成的 LUT 时传 (lut, n) 元组。

    缓存口径：只有「路径 LUT」进稠密表缓存；运行时生成的 LUT（autograde 逐图新建）
    不缓存 —— 它们只套一次，缓存等于每次泄漏一张 11MB 表。
    """
    if isinstance(path_or_lut, str):
        lut, n = get_cube(path_or_lut)
        cacheable = True
    else:
        lut, n = path_or_lut
        cacheable = False
    strength = float(max(0.0, min(1.0, strength)))
    return _apply_nearest(img_u8, lut, n, strength, cacheable)


# 高密度查表立方体的边长：源 LUT 多为 32³/33³，直接最近邻会在天空等平滑渐变上
# 出可见色带（实测最大通道误差 58~72）；先重采样到这个密度再查表。
# 实测（真实照片 night_city.jpg）：D=65 → 均值 1.0~1.6 / 最大 12~16；
# D=97 → 均值 0.87~1.04 / 最大 11~12；D=129 → 均值 0.5~0.76（收益递减、表涨到 26MB）。
# 取 97：11MB 表、每张 LUT 建表一次约 0.87s，之后 12MP 单张 ~0.54s（原三线性 6.0s）。
_DENSE = 97

# 量化表缓存条数上限。单条 97³×3 ≈ 2.7MB；胶片 LUT 的 strength 可连续变化，
# 不设上限会随滑动调参无限堆积。取 8 条（约 22MB）足够覆盖一批照片里出现的档位。
_DENSE_U8_CACHE_MAX = 8


def _sample_trilinear(lut: np.ndarray, n: int, coords: np.ndarray) -> np.ndarray:
    """在 LUT 上按归一化坐标 [0,1] 三线性取样；coords 形状 (...,3)，顺序 [r,g,b]。"""
    c = np.clip(coords, 0.0, 1.0) * (n - 1)
    i0 = np.clip(np.floor(c), 0, n - 1).astype(np.int32)
    i1 = np.minimum(i0 + 1, n - 1)
    f = (c - i0).astype(np.float32)
    r0, g0, b0 = i0[..., 0], i0[..., 1], i0[..., 2]
    r1, g1, b1 = i1[..., 0], i1[..., 1], i1[..., 2]
    fr, fg, fb = f[..., 0:1], f[..., 1:2], f[..., 2:3]

    c00 = lut[b0, g0, r0] * (1 - fr) + lut[b0, g0, r1] * fr
    c01 = lut[b0, g1, r0] * (1 - fr) + lut[b0, g1, r1] * fr
    c10 = lut[b1, g0, r0] * (1 - fr) + lut[b1, g0, r1] * fr
    c11 = lut[b1, g1, r0] * (1 - fr) + lut[b1, g1, r1] * fr
    c0 = c00 * (1 - fg) + c01 * fg
    c1 = c10 * (1 - fg) + c11 * fg
    return c0 * (1 - fb) + c1 * fb


def _dense_flat(lut: np.ndarray, n: int, cache: bool = True) -> tuple[np.ndarray, int]:
    """源 LUT → (高密度扁平常量表, 边长)，按 LUT 缓存，与 strength 无关。

    建表一次 ~0.9s（D=97），之后的强度混合在常量表上做（11MB，几毫秒），
    这样同一批照片里切强度也不必重建。`cache=False` 用于运行时生成、只套一次的 LUT
    （autograde 逐图新建）：不写缓存，避免每次泄漏一张 11MB 表。

    缓存条目同时持有 `lut` 本身：只按 `id(lut)` 做 key 时，LUT 被 GC 后新数组
    可能复用同一个 id，从而命中旧表、套用**别的照片的调色参数**（静默错色）。
    持有引用 + 身份比对后不可能发生；一条 LUT 仅 33³×3×4 ≈ 431KB。
    """
    key = (id(lut), n)
    if cache:
        hit = _DENSE_CACHE.get(key)
        if hit is not None and hit[0] is lut:
            return hit[1], hit[2]
    line = np.linspace(0.0, 1.0, _DENSE, dtype=np.float32)
    b, g, r = np.meshgrid(line, line, line, indexing="ij")
    coords = np.stack([r, g, b], axis=-1)                 # [b,g,r] 轴序，坐标顺序 [r,g,b]
    dense = _sample_trilinear(lut, n, coords)
    flat = np.ascontiguousarray(dense.reshape(-1, 3), dtype=np.float32)
    if cache:
        if len(_DENSE_CACHE) >= _DENSE_CACHE_MAX:
            _DENSE_CACHE.pop(next(iter(_DENSE_CACHE)))
        _DENSE_CACHE[key] = (lut, flat, _DENSE)
    return flat, _DENSE


def _identity_flat(d: int) -> np.ndarray:
    """恒等映射的扁平常量表（[d³,3]，坐标顺序 [r,g,b]），按边长缓存。"""
    hit = _IDENTITY_CACHE.get(d)
    if hit is None:
        line = np.linspace(0.0, 1.0, d, dtype=np.float32)
        b, g, r = np.meshgrid(line, line, line, indexing="ij")
        hit = np.ascontiguousarray(np.stack([r, g, b], axis=-1).reshape(-1, 3), dtype=np.float32)
        _IDENTITY_CACHE[d] = hit
    return hit


def _index_table(d: int) -> np.ndarray:
    """256 项 uint8→网格索引映射：`idx_tab[v]` = 像素值 v 在边长 d 的表里的坐标。

    为什么值得预计算：`v * scale + 0.5` 对 24MP 图要算 7200 万次，但 v 只有 256 种
    取值，结果表只有 1KB。查表替代浮点乘加后，索引阶段实测 1213ms → 473ms。
    """
    hit = _INDEX_CACHE.get(d)
    if hit is None:
        scale = (d - 1) / 255.0
        hit = np.clip(np.arange(256, dtype=np.float32) * scale + 0.5, 0, d - 1).astype(np.int32)
        _INDEX_CACHE[d] = hit
    return hit


def _dense_u8(
    lut: np.ndarray, n: int, strength: float, cache: bool = True
) -> tuple[np.ndarray, int]:
    """(高密度扁平表, 边长)，表项已量化为 uint8 —— gather 出来即成品，无需二次转换。

    量化放在**建表时**（97³×3 约 27 万次）而不是**套用时**（24MP×3 约 7200 万次），
    这是本函数存在的唯一理由。strength 混合仍在 float 域完成后再量化，
    与旧实现（float 混合 → 末端量化）逐位一致。`cache=False` 时只算不存。
    """
    key = (id(lut), n, round(float(strength), 4))
    if cache:
        hit = _DENSE_U8_CACHE.get(key)
        if hit is not None and hit[0] is lut:
            return hit[1], hit[2]
    flat, d = _dense_flat(lut, n, cache)
    if strength < 1.0:
        flat = flat * strength + _identity_flat(d) * (1.0 - strength)
    u8 = np.clip(flat * 255.0 + 0.5, 0, 255).astype(np.uint8)
    if cache:
        # 强度可连续变化，缓存按最近使用淘汰，避免 2.7MB/条无限堆积
        if len(_DENSE_U8_CACHE) >= _DENSE_U8_CACHE_MAX:
            _DENSE_U8_CACHE.pop(next(iter(_DENSE_U8_CACHE)))
        _DENSE_U8_CACHE[key] = (lut, u8, d)
    return u8, d


def _apply_nearest(
    img_u8: np.ndarray, lut: np.ndarray, n: int, strength: float, cache: bool = True
) -> np.ndarray:
    """查表：把三维索引压成一维后一次 np.take。

    轴序仍是 [b,g,r]（见模块头），所以索引 = (b*d + g)*d + r。

    全程无浮点：索引走 256 项预计算表，输出表已量化成 uint8，`np.take` 直接
    返回成品。实测 24MP 1213ms → 473ms，与旧实现逐位一致（最大差 0 级）。
    """
    flat, d = _dense_u8(lut, n, strength, cache)
    idx = _index_table(d)[img_u8]                      # (H,W,3) int32，三次整数查表
    flat_idx = (idx[..., 2] * d + idx[..., 1]) * d + idx[..., 0]
    return np.take(flat, flat_idx.ravel(), axis=0).reshape(img_u8.shape)


def _apply_trilinear(img_u8: np.ndarray, lut: np.ndarray, n: int, strength: float) -> np.ndarray:
    """逐像素三线性查表（参考实现/回归对照；生产路径走 `_apply_nearest`）。

    保留它是为了「改进后的快速路径 vs 教科书三线性」有可比对的基准，
    以及 LUT 尺寸极小、或需要最高保真时可直接调用。
    """
    src = img_u8.astype(np.float32) / 255.0
    out = _sample_trilinear(lut, n, src)
    if strength < 1.0:
        out = src * (1.0 - strength) + out * strength
    return np.clip(out * 255.0 + 0.5, 0, 255).astype(np.uint8)


def restore_black_white(img_u8: np.ndarray, amount: float) -> np.ndarray:
    """黑白场还原：把胶片 LUT 抬起的黑位 / 压低的白位按 amount 拉回 0/1，去「发灰」。

    为什么需要：负片类 LUT（classic_neg 等）刻意抬黑位造「哑光感」，但产品链路里
    auto_grade 刚把黑场拉伸到 0，LUT 再抬一次等于二次发灰（实测 p1 从 0.035 抬到
    0.07~0.08，用户感知为「胶片过后都发灰」）。amount∈[0,1] 为还原比例，0 关闭。

    实现：实测 LUT 后的 p1 分布在 [0, 0.10]、p99 在 [0.90, 1.0]，据此取固定锚点
    black=0.10 / white=0.90 做线性拉伸——只在灰感明显时才起作用（p1 越接近锚点
    拉伸越强），不影响本来就没抬黑位的 LUT。纯逐色映射，O(H·W) 一次查表代价。
    """
    amount = float(max(0.0, min(1.0, amount)))
    if amount <= 0:
        return img_u8
    x = img_u8.astype(np.float32) / 255.0
    black = 0.10 * amount
    white = 1.0 - 0.10 * amount
    span = max(white - black, 1e-4)
    out = (x - black) / span
    return np.clip(out * 255.0 + 0.5, 0, 255).astype(np.uint8)


def apply_grain(img_u8: np.ndarray, amount: float, seed: int | None = None) -> np.ndarray:
    """叠加胶片颗粒：amount∈[0,1]，0 表示不处理。"""
    amount = float(max(0.0, min(1.0, amount)))
    if amount <= 0:
        return img_u8
    rng = np.random.default_rng(seed)
    noise = rng.normal(0.0, 18.0 * amount, img_u8.shape)
    out = img_u8.astype(np.float32) + noise
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def apply_vignette(img_u8: np.ndarray, amount: float) -> np.ndarray:
    """叠加暗角：amount∈[0,1]，越大四角越暗；中心不变。"""
    amount = float(max(0.0, min(1.0, amount)))
    if amount <= 0:
        return img_u8
    h, w = img_u8.shape[:2]
    ys = np.linspace(-1.0, 1.0, h, dtype=np.float32)[:, None]
    xs = np.linspace(-1.0, 1.0, w, dtype=np.float32)[None, :]
    d = np.sqrt(xs * xs + ys * ys) / np.sqrt(2.0)  # 0(中心)~1(角)
    mask = 1.0 - amount * np.clip((d - 0.35) / 0.65, 0.0, 1.0) ** 1.4
    out = img_u8.astype(np.float32) * mask[..., None]
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def lut_path(luts_dir: str, file_name: str) -> str:
    return os.path.join(luts_dir, file_name)
