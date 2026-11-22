"""人像肤色保真（FEAT-208）：以原图皮肤色度为锚，抵消调色/滤镜偏色，再叠加白黄·嫩红偏置。

分层职责（与 `skinretouch` 同一纪律：本模块只做「给定掩码与锚点 → 怎么改像素」）：
  - **全部是纯函数**：不检测人脸、不读盘；皮肤掩码与锚点由调用方提供
    （`ops_local.skin_tone_image` / `workflows/local_beauty.py`），因此可离线单测；
  - 色度操作在 CIELAB 极坐标 (C, h) 里做，与 `autograde._saturate_lch` /
    `_vibrance_lch` 同一体系（`.dsh/knowledge/local-enhance.md` §25）；
  - 所有校正都是**逐像素增量**，不是全脸统一赋值：全脸统一转到同一色相角会把
    鼻尖/耳朵/颊部的自然色相差抹平（h_sd 压到 ~0），脸变成色卡 —— 「不自然」的
    技术来源。实测逐像素增量可把 h_sd 从 13.64° 保到 13.59°。

三条实测换来的设计定案（2026-11-21，12 张真实人像）：
  1. **锚点 = 原图皮肤中位色度 + 有界偏移**，不是固定「白黄」值。素材一半是室内
     暖光人像（原图 h≈23~36°，本就偏红），固定 58° 会把这类照片整个改写成另一种
     肤色 —— 违反「接近原图肤色」，也不自然。
  2. **彩度校正用加性平移（C + dc）而不是比例缩放（C × ratio）**：平移完整保留
     彩度离散度 C_sd；比例缩放会把面部彩度层次整体压扁（实测 7.12 → 4.70）。
     且平移形式天然幂等：校正到位后 dc → 0，二次应用不再漂移。
  3. **明度提亮必须带高光保护**：硬提 +2 无收口实测引入 40009px 通道截断；
     权重随 L 升高线性衰减，L ≥ L_SAFE 的高光（额头油光/鼻尖反光）完全不动
     （local-enhance §9：提亮会洗掉面部立体感）。

「嫩红」的实现：颊部 a* 的**加性增量**，同样朝锚点+偏置拉 —— 血色集中在苹果肌
而不是全脸泛红。颊部定位用检测框几何近似（零新增模型依赖），68 点留作后续增强；
颊部掩码必须**羽化**后再作用（硬边的矩形边界在强嫩红下会出现直线色块，实测
blush=3 时边界阶跃 3.5 个 Lab 单位）。

性别侧重（FEAT-209，2026-11-22）：「女性更白更嫩红、男性保留原肤色白一点」由
`for_gender` 表达 —— 它只改**偏置层**，不改锚点还原层，所以两档都会先把滤镜带来
的偏色拉回原图。判定的发长启发式在 `hairlength`（零新增模型）。
"""
from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from PIL import Image, ImageFilter

from . import colortransfer, skinretouch

# ---------------- 常量（全部有实测依据，改动前先跑 test_skintone.py） ----------------

# 色域判定的浮点容差：与 autograde._GAMUT_EPS 同口径。sRGB 原色恰在边界上，
# Lab 往返的浮点误差有 1e-7 量级越界，不留容差会把合法像素判为越界。
_GAMUT_EPS = 1e-5
# 色域回收的二分次数：区间压到 1/256，8bit 输出足够。
_GAMUT_ITERS = 8
# 低于此彩度不做色相旋转/彩度平移：C≈0 时色相角是噪声，动了只会给灰区染色。
C_LOW = 4.0
# 明度提亮的高光保护：L ≥ L_SAFE 视为反光/油光，权重衰减到 0；L ≤ L_FULL 全量提。
# L_FULL 从 40 提到 70（FEAT-209）：旧曲线在中间调肤色（L≈59）只给 0.5 权重，标称
# 1.5 的提亮实际只有 +0.75 L*，用户反馈「不够白」。现在中间调拿满权重，只在
# 70~78 这段收口，高光保护（L≥78 逐字节不动）不变。
L_SAFE = 78.0
L_FULL = 70.0
# 颊部掩码羽化半径（占脸短边的比例）：嫩红必须软边，否则矩形边界露出直线色块。
CHEEK_FEATHER_RATIO = 0.045
# 皮肤掩码羽化半径（px）：与验证脚本同口径；贴回无硬边即可，不需要跟脸宽走。
FEATHER_RADIUS = 3
# 统计可信下限：掩码像素低于此值时中位数不稳，该脸跳过（宁不动不动歪）。
MIN_SKIN_PX = 800

# 参数合法区间（与 op_catalog.json 的 schema 一致；workflow 直读 opts 时在这里兜底夹取）
PARAM_RANGES = {
    "hue_shift": (-8.0, 8.0),
    "chroma": (-6.0, 6.0),
    "brighten": (0.0, 4.0),
    "blush": (0.0, 3.0),
}

# 男性侧重档：只还原原图肤色 + 轻提亮（用户口径「保有原肤色、白一点」）。
# 不做白黄偏置、不加嫩红；提亮按这个比例打折（默认 2.2 → 男 0.88 L*）。
MALE_BRIGHTEN_RATIO = 0.4


@dataclass(frozen=True)
class SkinParams:
    """一次肤色校正的四个配方级量。默认值即「人像」配方档（女性侧重档）。"""

    hue_shift: float = 3.0    # 色相偏移（度，>0 往黄，「白黄」）
    chroma: float = 1.5       # 彩度平移（Lab C 单位，>0 更透亮）
    brighten: float = 2.2     # 提亮（L* 单位，带高光保护；中间调全额生效）
    blush: float = 1.8        # 颊部 a* 增量（嫩红）

    @classmethod
    def from_raw(cls, raw: dict | None) -> "SkinParams":
        """任意来源（配方 / 前端 options）→ 夹到合法区间。缺省用配方默认档。"""
        raw = raw if isinstance(raw, dict) else {}
        defaults = cls()

        def _one(key: str) -> float:
            fallback = getattr(defaults, key)
            try:
                v = float(raw.get(key, fallback))
            except (TypeError, ValueError):
                return fallback
            lo, hi = PARAM_RANGES[key]
            return max(lo, min(hi, v))

        return cls(
            hue_shift=_one("hue_shift"),
            chroma=_one("chroma"),
            brighten=_one("brighten"),
            blush=_one("blush"),
        )

    def is_noop(self) -> bool:
        """四项全 0 = 该脸不做任何事（仍会做锚点统计，用于跳过判断）。"""
        return (
            abs(self.hue_shift) < 1e-3
            and abs(self.chroma) < 1e-3
            and self.brighten < 1e-3
            and self.blush < 1e-3
        )


def for_gender(p: SkinParams, long_hair: bool) -> SkinParams:
    """这一张脸实际使用的四个量（性别侧重，FEAT-209）。

    - 长发（女性侧重）：用户档位原样生效 —— 更白（提亮）+ 更嫩红（颊部 a*）；
    - 非长发（男性侧重）：只保留「还原原肤色 + 轻提亮」，白黄偏置与嫩红归零。

    注意「还原」不靠参数：它由锚点差算出（`correction_amounts`），所以男性档
    依然会把滤镜带偏的肤色拉回原图，只是不再往上加黄和红。
    """
    if long_hair:
        return p
    return SkinParams(
        hue_shift=0.0,
        chroma=0.0,
        brighten=p.brighten * MALE_BRIGHTEN_RATIO,
        blush=0.0,
    )


@dataclass(frozen=True)
class SkinAnchor:
    """一张脸在**原图**上的皮肤色度基准（Lab 中位数）。

    全部来自原图：调色/滤镜对肤色的任何污染都体现在「当前值 − 锚点」的差里，
    校正就是把这个差（加上固定偏置）抵消掉。
    """

    hue: float       # 皮肤中位色相角（度）
    chroma: float    # 皮肤中位彩度（Lab C）
    cheek_a: float   # 颊部 a* 中位（嫩红基准）


def wrap_deg(d: float) -> float:
    """色相角差 wrap 到 [-180, 180)。"""
    return (float(d) + 180.0) % 360.0 - 180.0


def cheek_region(shape: tuple[int, int]) -> np.ndarray:
    """检测框裁切图内的颊部近似区域（bool [h,w]）。

    用框内相对坐标（左右两块 × 下半段），不依赖 68 点 —— 零新增模型依赖。
    侧脸/大角度下会偏，但颊部增量本身只有 ~1 个 Lab 单位，偏一点可接受；
    更精细的解剖分区留给 68 点增强版。
    """
    h, w = shape
    yy, xx = np.mgrid[0:h, 0:w]
    fx = xx / max(w - 1, 1)
    fy = yy / max(h - 1, 1)
    return (((fx > 0.18) & (fx < 0.46)) | ((fx > 0.54) & (fx < 0.82))) & (fy > 0.42)


def _cheek_alpha(cheek_mask: np.ndarray) -> np.ndarray:
    """颊部掩码 → 羽化 alpha（半径挂脸短边，复用磨皮的同一把羽化刀）。

    统计口径仍用二值掩码（`predicted_cheek_a`），只有**作用**用羽化后的 alpha：
    边界不再是一刀切的阶跃，强嫩红也不会在脸上留下矩形色块。
    """
    h, w = cheek_mask.shape
    radius = max(2, int(round(CHEEK_FEATHER_RATIO * min(h, w))))
    return skinretouch._feather(np.clip(cheek_mask, 0.0, 1.0), radius)


def _to_lab(rgb_u8: np.ndarray) -> np.ndarray:
    return colortransfer.rgb_to_lab(np.clip(rgb_u8, 0, 255).astype(np.uint8)).astype(np.float32)


def _lab_to_linear(lab: np.ndarray) -> np.ndarray:
    """Lab → 线性 sRGB，不裁剪（超色域判定必须看裁剪前的值）。

    常量复用 `colortransfer` 同一份（autograde 已踩过「手抄矩阵漏 .T」的坑）。
    """
    L, a, b = lab[..., 0], lab[..., 1], lab[..., 2]
    fy = (L + 16.0) / 116.0
    fx = fy + a / 500.0
    fz = fy - b / 200.0
    d = 6.0 / 29.0

    def finv(t):
        return np.where(t > d, t ** 3, 3.0 * d * d * (t - 4.0 / 29.0))

    xyz = np.stack([finv(fx), finv(fy), finv(fz)], axis=-1) * colortransfer._WHITE
    return xyz @ colortransfer._MINV.T


def _in_gamut(lab: np.ndarray) -> np.ndarray:
    lin = _lab_to_linear(lab)
    return np.all((lin >= -_GAMUT_EPS) & (lin <= 1.0 + _GAMUT_EPS), axis=-1)


def _fit_ab_scale(l_fixed: np.ndarray, a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """逐像素二分：保持 L 与 (a,b) 方向，求能把颜色收回 sRGB 的最大幅度 ∈ [0,1]。

    为什么不用 `autograde._chroma_headroom`：那是「固定 h 缩放 C」的场景，这里
    校正后的 (a,b) 方向逐像素不同（颊部还叠了 a* 增量），统一按向量幅度二分。

    性能：绝大多数像素根本不越界。先整查一次，只对越界像素的**子集**做 8 轮
    二分 —— 全量跑的实测代价是肤色步从 ~200ms 涨到 ~400ms（脸大时更高）。
    """
    ok = _in_gamut(np.stack([l_fixed, a, b], axis=-1))
    if bool(ok.all()):
        return np.ones_like(a)
    s = np.ones_like(a)
    idx = np.nonzero(~ok)
    lo = np.zeros(int(idx[0].size))
    hi = np.ones(int(idx[0].size))
    lf = l_fixed[idx]
    af = a[idx]
    bf = b[idx]
    for _ in range(_GAMUT_ITERS):
        mid = (lo + hi) * 0.5
        cand = np.stack([lf, af * mid, bf * mid], axis=-1)
        good = _in_gamut(cand)
        lo = np.where(good, mid, lo)
        hi = np.where(good, hi, mid)
    s[idx] = lo
    return s


def _erode(mask: np.ndarray, radius: int) -> np.ndarray:
    """把掩码向内腐蚀 radius 像素（羽化带不参与统计）。

    为什么：贴回时羽化带内的像素是「原值与校正值」的混合，若把它们算进统计，
    下一轮的 dh/平移量会被混合值稀释（实测残差 ~1.8°）；腐蚀后统计集合只含
    完全校正的内部像素，目标精确命中、幂等成立。
    """
    img = Image.fromarray((np.clip(mask, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8), "L")
    img = img.filter(ImageFilter.MinFilter(2 * radius + 1))
    return np.asarray(img, dtype=np.float32) / 255.0


def region_stats(
    lab: np.ndarray, skin_mask: np.ndarray, cheek_mask: np.ndarray | None = None
) -> dict[str, float] | None:
    """一张脸的皮肤色度统计（L 分位过滤后的**向量均值**口径）。样本不足返回 None。

    为什么必须是向量均值而不是中位数：校正把每个像素的 (a,b) 向量**旋转同一
    角度**再全脸统一平移 —— 只有均值满足「旋转/平移下线性」，目标才能精确命中，
    二次应用才真正幂等。中位数口径下 atan2(中位 a, 中位 b) 对旋转非线性，
    实测残差达 1.6~2.2°。
    离群值防护不靠中位数，改用两层过滤：彩度过低（C < C_LOW）的灰区不参与；
    明度取 8%~92% 分位带（滤高光/阴影，L 在旋转/平移下不变 → 集合稳定）。
    """
    m = skin_mask > 0
    if int(m.sum()) < MIN_SKIN_PX:
        return None
    a = lab[..., 1]
    b = lab[..., 2]
    L = lab[..., 0]
    conf = m & (np.hypot(a, b) >= C_LOW)
    if int(conf.sum()) < MIN_SKIN_PX // 2:
        conf = m                      # 全灰图退化：退回硬掩码集合
    inner = conf & (_erode(skin_mask, FEATHER_RADIUS) > 0.5)
    if int(inner.sum()) < MIN_SKIN_PX // 3:
        inner = conf                  # 小脸退化：不腐蚀
    # 明度分位带：滤掉高光（额头油光）与深阴影，它们的色度不代表皮肤本色。
    # L 在旋转/平移下不变 → 集合在两轮应用间稳定，线性关系不破坏。
    lo, hi = np.percentile(L[inner], (8.0, 92.0))
    core = inner & (L >= lo) & (L <= hi)
    if int(core.sum()) < MIN_SKIN_PX // 3:
        core = inner
    mean_a = float(np.mean(a[core]))
    mean_b = float(np.mean(b[core]))
    out = {
        "mean_a": mean_a,
        "mean_b": mean_b,
        "hue": float(np.degrees(np.arctan2(mean_b, mean_a))),
        "chroma": float(np.hypot(mean_a, mean_b)),
    }
    cheek_a = mean_a
    cheek_core = core & (cheek_mask > 0) if cheek_mask is not None else None
    if cheek_core is not None and int(cheek_core.sum()) >= max(MIN_SKIN_PX // 4, 200):
        cheek_a = float(np.median(a[cheek_core]))
    out["cheek_a"] = cheek_a
    out["core"] = core          # 供 correct_region 预测颊部校正后值（含 ndarray，仅内部使用）
    return out


def anchor_from_pixels(
    rgb_u8: np.ndarray, skin_mask: np.ndarray, cheek_mask: np.ndarray | None = None
) -> SkinAnchor | None:
    """从原图像素算锚点。掩码由调用方提供（BiSeNet 解析结果）。"""
    stats = region_stats(_to_lab(rgb_u8), skin_mask, cheek_mask)
    if stats is None:
        return None
    return SkinAnchor(hue=stats["hue"], chroma=stats["chroma"], cheek_a=stats["cheek_a"])


def correction_amounts(
    anchor: SkinAnchor, cur: dict[str, float], p: SkinParams
) -> tuple[float, float, float]:
    """整体色度校正量：(色相旋转 dh、色度向量平移 dax/day)。

    语义：先逐像素旋转把滤镜造成的色相偏移抵消，再全脸统一平移色度向量到
    「锚点 + 偏置」。旋转下均值线性、平移下均值线性 → 目标精确命中，二次
    应用时 dh/dax/day 全部归零，天然幂等。

    颊部增量 da 不在这里算：它与旋转/平移统计耦合（旋转本身就会改变颊部
    a* 的中位数，若按「旋转前统计」算 da 会把旋转副作用误判成红润变化，
    实测产生 −2.4 的伪校正把向量拽偏 ~2°）。da 改由 `predicted_cheek_a`
    在「旋转+平移后」的预测值上计算，彻底解耦。
    """
    t_h = anchor.hue + p.hue_shift
    t_c = max(anchor.chroma + p.chroma, 0.0)
    dh = wrap_deg(t_h - cur["hue"])
    rad = np.radians(dh)
    rot_a = cur["mean_a"] * np.cos(rad) - cur["mean_b"] * np.sin(rad)
    rot_b = cur["mean_a"] * np.sin(rad) + cur["mean_b"] * np.cos(rad)
    dax = t_c * np.cos(np.radians(t_h)) - rot_a
    day = t_c * np.sin(np.radians(t_h)) - rot_b
    return float(dh), float(dax), float(day)


def predicted_cheek_a(
    lab: np.ndarray, core_mask: np.ndarray, cheek_mask: np.ndarray,
    dh: float, dax: float,
) -> float | None:
    """预测「旋转 + 平移后」的颊部 a* 中位数（不真跑一遍校正）。

    嫩红基准的当前值必须取自校正**之后**的色度 —— 否则旋转副作用会混进来。
    样本不足返回 None（该脸不做颊部校正，宁缺勿歪）。
    """
    m = core_mask & (cheek_mask > 0)
    if int(m.sum()) < max(MIN_SKIN_PX // 4, 200):
        return None
    a = lab[..., 1]
    b = lab[..., 2]
    C = np.hypot(a, b)
    h2 = np.degrees(np.arctan2(b, a)) + np.where(C >= C_LOW, dh, 0.0)
    a2 = C * np.cos(np.radians(h2)) + np.where(C >= C_LOW, dax, 0.0)
    return float(np.median(a2[m]))


def correct_region(
    rgb_u8: np.ndarray,
    skin_mask: np.ndarray,
    cheek_mask: np.ndarray,
    anchor: SkinAnchor | None,
    p: SkinParams,
) -> np.ndarray:
    """对一张脸的 ROI 应用肤色校正，返回新的 uint8 RGB。

    `anchor` 缺省（拿不到原图）时从当前像素自估锚点 —— 还原项退化为 0，
    只剩白黄·嫩红偏置（单步「本地美颜」场景就是这条路径）。
    """
    roi = rgb_u8.astype(np.float32)
    lab = _to_lab(rgb_u8)
    m = skin_mask > 0
    if not m.any():
        return rgb_u8

    cur = region_stats(lab, skin_mask, cheek_mask)
    if cur is None:
        return rgb_u8
    if anchor is None:
        anchor = SkinAnchor(hue=cur["hue"], chroma=cur["chroma"], cheek_a=cur["cheek_a"])

    dh, dax, day = correction_amounts(anchor, cur, p)
    da = 0.0
    if p.blush > 1e-3 or anchor.cheek_a != cur["cheek_a"]:
        pred = predicted_cheek_a(lab, cur["core"], cheek_mask, dh, dax)
        if pred is not None:
            da = (anchor.cheek_a + p.blush) - pred
    # 参数全 0 ≠ 跳过：还原项（dh/向量平移/da）仍会执行 —— 那是本算子的存在意义；
    # 「完全不跑」由调用方用 SkinParams.is_noop() 判断后直接不进来。
    if (
        abs(dh) < 1e-3 and abs(dax) < 1e-3 and abs(day) < 1e-3
        and abs(da) < 1e-3 and p.brighten < 1e-3
    ):
        return rgb_u8

    L = lab[..., 0]
    a = lab[..., 1]
    b = lab[..., 2]
    C = np.hypot(a, b)
    confident = C >= C_LOW

    # 1) 色相：逐像素增量旋转（C 低的像素不转，那是灰区/噪声）
    h2 = np.degrees(np.arctan2(b, a)) + np.where(confident, dh, 0.0)
    hr = np.radians(h2)
    # 2) 色度向量：逐像素旋转后的 (a,b) 上叠加全脸统一平移（灰区不动）
    a2 = C * np.cos(hr) + np.where(confident, dax, 0.0)
    b2 = C * np.sin(hr) + np.where(confident, day, 0.0)
    # 3) 颊部嫩红：a* 加性增量（为负 = 滤镜把颊部推太红，往回拉）。
    #    掩码必须羽化：颊部区域是框内矩形，硬边在强嫩红下会露出直线色块。
    if abs(da) > 1e-3:
        alpha_c = _cheek_alpha(cheek_mask)
        a2 = a2 + da * alpha_c
    # 4) 明度：一次性提亮 + 高光保护（唯一不幂等的量，属「手感」范畴）
    if p.brighten > 1e-3:
        w = np.clip((L_SAFE - L) / max(L_SAFE - L_FULL, 1.0), 0.0, 1.0)
        L2 = np.clip(L + p.brighten * w, 0.0, 100.0)
    else:
        L2 = L

    # 5) 色域回收：逐像素沿 (a2,b2) 方向收回 sRGB
    s = _fit_ab_scale(L2, a2, b2)
    new_lab = np.stack([L2, a2 * s, b2 * s], axis=-1)
    new_rgb = colortransfer.lab_to_rgb(new_lab).astype(np.float32)

    # 6) 羽化贴回（复用磨皮的同一把羽化刀，皮肤/头发交界不出硬边）
    alpha = skinretouch._feather(skin_mask, FEATHER_RADIUS)
    alpha = (alpha * m)[..., None]
    out = roi * (1.0 - alpha) + new_rgb * alpha
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


__all__ = [
    "PARAM_RANGES",
    "MALE_BRIGHTEN_RATIO",
    "SkinAnchor",
    "SkinParams",
    "for_gender",
    "MIN_SKIN_PX",
    "C_LOW",
    "L_SAFE",
    "L_FULL",
    "wrap_deg",
    "cheek_region",
    "region_stats",
    "anchor_from_pixels",
    "correction_amounts",
    "predicted_cheek_a",
    "correct_region",
]
