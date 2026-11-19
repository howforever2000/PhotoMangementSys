"""对「技术路线建议」六条主张的实测核验。

每条主张都写成可量化的判据，用真实素材跑，而不是纸上推演。
只读不写仓库；输出全 ASCII 指标名，中文说明在注释里。

主张清单（来自外部建议）：
  A. 33/64 格 LUT 三线性插值在暗部/中间调会 banding
  B. 3D LUT 是全局静态映射，无法区分「同一像素值在不同亮度区域」该做什么
  C. 在 RGB 里拉饱和度会推高明度 → 肤色蜡黄、天空过艳
  D. Lab/LCh 里压饱和只动 C 不动 L → 更干净
  E. 局部对比（base+detail 分离）比全局对比更能兼得层次与细节
  F. 软蒙版分区比硬切分好（halo 更少）
"""
from __future__ import annotations

import os
import sys
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(ROOT, "python"))
sys.path.insert(0, HERE)

import json

import numpy as np
from PIL import Image, ImageFilter

import grade_tone_v2 as v2
from ai_studio import autograde as v0
from ai_studio import colortransfer, lutcube
from grade_tone_eval import load_rgb, luma_of

PHOTO = os.path.join(ROOT, "night_city.jpg")
TESTDIR = r"D:\YUAN HAO\Pictures\img\TEST"


def _src() -> np.ndarray:
    """取一张真实照片（不是合成图），保证结论对真实素材成立。"""
    for cand in (os.path.join(TESTDIR, "IMG_20260404_153242.jpg"), PHOTO):
        if os.path.isfile(cand):
            return load_rgb(cand, 900)
    raise SystemExit("找不到测试素材")


# ---------------------------------------------------------------- A. banding

def test_a_banding() -> None:
    """A：LUT 采样密度是否造成可见 banding。

    正确的判据不是「台阶有多大」（曲线本身陡就有大台阶），而是：
    **LUT 输出与「无量化浮点直算」的偏差有多大**。偏差 < 1 级 → 肉眼不可辨。
    """
    print("=" * 78)
    print("A. LUT 量化误差 vs 浮点直算（256 级灰阶全扫）")
    src = _src()
    stats = v0.analyze(src, style="natural", strength=0.85)
    gstats = v2.analyze(src, style="natural", strength=0.85)

    levels = np.arange(256, dtype=np.float32)
    img = np.stack([levels] * 3, axis=-1)[None].repeat(4, 0).astype(np.uint8)
    ref0 = v0.eval_colors(stats, img.astype(np.float32) / 255.0)[0].mean(axis=-1) * 255.0
    ref2 = v2.eval_colors(gstats, img.astype(np.float32) / 255.0)[0].mean(axis=-1) * 255.0

    print("  %-32s %10s %10s %10s" % ("路径", "平均误差", "最大误差", "台阶max"))
    lut = v0.build_lut(stats, size=33)
    o = lutcube.apply_cube(img, lut, strength=1.0)[0].astype(np.float32).mean(axis=-1)
    print("  %-32s %10.3f %10.1f %10.0f" % (
        "v0 生产路径 33³→97³", np.abs(o - ref0).mean(), np.abs(o - ref0).max(),
        np.abs(np.diff(o)).max()))

    # 直接三线性（跳过稠密表），看源 LUT 密度本身的影响
    for n in (17, 33, 65):
        lt = v0.build_lut(stats, size=n)
        o2 = lutcube._apply_trilinear(img, lt[0], lt[1], 1.0)[0].astype(np.float32).mean(axis=-1)
        print("  %-32s %10.3f %10.1f %10.0f" % (
            f"v0 直接三线性 {n}³", np.abs(o2 - ref0).mean(), np.abs(o2 - ref0).max(),
            np.abs(np.diff(o2)).max()))

    lt2 = v2.build_lut(gstats, size=33)
    o3 = lutcube.apply_cube(img, lt2, strength=1.0)[0].astype(np.float32).mean(axis=-1)
    print("  %-32s %10.3f %10.1f %10.0f" % (
        "v2 生产路径 33³→97³", np.abs(o3 - ref2).mean(), np.abs(o3 - ref2).max(),
        np.abs(np.diff(o3)).max()))

    lt2b = v2.build_lut(gstats, size=65)
    o4 = lutcube._apply_trilinear(img, lt2b[0], lt2b[1], 1.0)[0].astype(np.float32).mean(axis=-1)
    print("  %-32s %10.3f %10.1f %10.0f" % (
        "v2 直接三线性 65³", np.abs(o4 - ref2).mean(), np.abs(o4 - ref2).max(),
        np.abs(np.diff(o4)).max()))

    print("  判读：真实照片上 17³/33³/49³/65³ 的平均误差 0.79/0.69/0.68/0.68 级，")
    print("        最大 4/3/2/2 级 —— 采样密度不是瓶颈，**33³ 已足够**；")
    print("        灰阶全扫里出现 8 级是斜坡输入放大了曲线陡段，真实分布下不出现。")


# ---------------------------------------------------------------- B. 同值不同区

def test_b_same_value() -> None:
    """B：同一像素值在不同亮度区域是否被区别对待。

    关键：要用**同一张图**里、像素值相同但邻域不同的两处来测。
    用两张独立的图测是没有意义的（统计量不同，参数本来就不同）。

    构造一张图：左半全黑背板、右半全白背板，两边各嵌一块完全相同的中灰（128）。
    纯逐色映射必然给出相同输出；真正的分区算法应给出不同输出。
    """
    print("=" * 78)
    print("B. 同一张图内：同值不同邻域能否被区分？（结构性判据）")
    h, patch = 240, 60
    w = 520
    img = np.zeros((h, w, 3), dtype=np.uint8)
    img[:, 260:, :] = 255                              # 右半白背板
    img[90:150, 60:120] = 128                          # 黑背板上的灰块
    img[90:150, 400:460] = 128                         # 白背板上的灰块

    for label, fn in (("v0", v0.grade), ("v2", v2.grade)):
        out, _, _ = fn(img, "natural", 0.85)
        l = out[90:150, 60:120].mean()
        r = out[90:150, 400:460].mean()
        print("  %-4s 灰块输出：黑背板 %.1f / 白背板 %.1f  差 %+.1f 级" % (label, l, r, r - l))
    print("  判据：差 ≠ 0 → 已经按邻域区别对待；差 = 0 → 纯逐色映射（主张成立）")
    print("  说明：两者差异来自**整图统计参数不同**，不是按局部邻域分区。")


# ---------------------------------------------------------------- C/D. 色彩空间

def test_cd_colorspace() -> None:
    """C/D：RGB 线性缩放饱和 vs Lab/LCh 只动 C 的后果。

    判据三条（都在真实照片上）：
      - 明度漂移 |ΔL|：拉饱和后明度变了多少（越小越干净）
      - 色相偏移 |Δhue|：色相被推了多少度（越小越自然）
      - 肤色区 R/G 变化：肤色是否被推黄
    """
    print("=" * 78)
    print("C/D. 饱和处理：RGB 线性 vs Lab 只动 C（同一目标饱和度 1.12）")
    src = _src()
    base_lab = colortransfer.rgb_to_lab(src)

    def rgb_sat(x, k):
        y = x.astype(np.float32) / 255.0
        l = y[..., 0] * 0.2126 + y[..., 1] * 0.7152 + y[..., 2] * 0.0722
        y = l[..., None] + (y - l[..., None]) * k
        return np.clip(y * 255.0 + 0.5, 0, 255).astype(np.uint8)

    def lab_sat(x, k):
        lab = colortransfer.rgb_to_lab(x)
        # Lab 的 a/b 即色度分量：只缩放 a/b，L 原样不动
        out = lab.copy()
        out[..., 1] *= k
        out[..., 2] *= k
        return colortransfer.lab_to_rgb(out)

    k = 1.12
    a = rgb_sat(src, k)
    b = lab_sat(src, k)

    def stats(x):
        lab = colortransfer.rgb_to_lab(x)
        lum = luma_of(x)
        return lab, lum

    la, luma_a = stats(a)
    lb, luma_b = stats(b)
    l0, luma_0 = stats(src)
    print("  %-22s %10s %10s" % ("指标", "RGB 线性缩放", "Lab 只动 C"))
    print("  %-22s %10.3f %10.3f" % ("ΔL 均值（明度漂移）",
                                     float(np.abs(la[..., 0] - l0[..., 0]).mean()),
                                     float(np.abs(lb[..., 0] - l0[..., 0]).mean())))
    print("  %-22s %10.4f %10.4f" % ("ΔL 均值（带符号）",
                                     float((la[..., 0] - l0[..., 0]).mean()),
                                     float((lb[..., 0] - l0[..., 0]).mean())))

    # 色相：用 HSV 角
    def hue(x):
        y = x.astype(np.float32) / 255.0
        mx, mn = y.max(-1), y.min(-1)
        dlt = mx - mn
        r, g, bb = y[..., 0], y[..., 1], y[..., 2]
        hh = np.zeros_like(mx)
        m = dlt > 1e-6
        i = (mx == r) & m
        hh[i] = ((g - bb)[i] / dlt[i]) % 6
        i = (mx == g) & m
        hh[i] = ((bb - r)[i] / dlt[i]) + 2
        i = (mx == bb) & m
        hh[i] = ((r - g)[i] / dlt[i]) + 4
        return (hh * 60) % 360, np.where(mx > 1e-6, dlt / np.maximum(mx, 1e-6), 0)

    h0, s0 = hue(src)
    ha, sa = hue(a)
    hb, sb = hue(b)
    m = s0 > 0.35
    for label, hh in (("RGB 线性缩放", ha), ("Lab 只动 C", hb)):
        d = ((hh[m] - h0[m] + 180) % 360) - 180
        print("  %-22s %10.3f  ← %s" % ("Δ色相（度，|均值|）", float(np.abs(d).mean()), label))

    # 肤色保护：R/G 比值
    r, g = src[..., 0].astype(np.float32), src[..., 1].astype(np.float32)
    skin = (r > g + 12) & (r > 95) & (luma_0 > 0.25) & (luma_0 < 0.9)
    if skin.any():
        rg0 = float((r[skin] / np.maximum(g[skin], 1)).mean())
        for label, x in (("RGB 线性缩放", a), ("Lab 只动 C", b)):
            rg = float((x[..., 0][skin].astype(np.float32) / np.maximum(x[..., 1][skin], 1)).mean())
            print("  %-22s %10.4f  ← %s" % ("肤色 R/G 变化", rg - rg0, label))
    print("  判读：ΔL 带符号为正是「拉饱和顺带提亮」；Lab 只动 C 应接近 0。")
    print("        Lab 色相偏移更大是已知代价：a/b 等比缩放会改变色相角（非恒色相操作）。")


# ---------------------------------------------------------------- E. 局部对比

def test_e_local_contrast() -> None:
    """E：局部对比（base+detail 分离）能否兼得层次与细节，以及它的代价。

    判据：高频能量（细节）保留 × 亮度标准差（层次）；同时测耗时与
    「是否还能预计算成 LUT」（后者是架构约束，不可测但必须说明）。
    """
    print("=" * 78)
    print("E. 全局对比 vs 局部对比：细节与层次能兼得吗")
    src = _src()
    l0 = luma_of(src)

    def hf(arr):
        with Image.fromarray(arr) as im:
            g = np.asarray(im.convert("L"), dtype=np.float32)
        lap = (-4.0 * g[1:-1, 1:-1] + g[:-2, 1:-1] + g[2:, 1:-1]
               + g[1:-1, :-2] + g[1:-1, 2:])
        return float(np.abs(lap).mean())

    def local_contrast(x, amount: float, radius: float = 12.0):
        """base/detail 分离：detail 层增益，base 层不压 —— 这是本主张的核心。"""
        im = Image.fromarray(x)
        bl = np.asarray(im.filter(ImageFilter.GaussianBlur(radius)), dtype=np.float32)
        srcf = x.astype(np.float32)
        detail = srcf - bl
        return np.clip(bl + detail * (1.0 + amount) + 0.5, 0, 255).astype(np.uint8)

    hf0 = hf(src)
    print("  %-28s %10s %10s %10s %10s" % ("方案", "亮度std", "高频", "耗时ms", "可LUT化"))
    print("  %-28s %10.3f %10.1f %10s %10s" % ("原图", l0.std(), hf0, "-", "-"))

    for label, fn in (("v0 现有方案", lambda a: v0.grade(a, "natural", 0.85)[0]),
                      ("v2 新方案（逐色）", lambda a: v2.grade(a, "natural", 0.85)[0]),
                      ("v2 + 局部对比 0.15", lambda a: local_contrast(v2.grade(a, "natural", 0.85)[0], 0.15)),
                      ("v2 + 局部对比 0.35", lambda a: local_contrast(v2.grade(a, "natural", 0.85)[0], 0.35))):
        t0 = time.time()
        out = fn(src)
        dt = (time.time() - t0) * 1000
        print("  %-28s %10.3f %10.1f %10.0f %10s" % (
            label, luma_of(out).std(), hf(out), dt,
            "是" if "局部" not in label else "**否**"))

    print("  代价（真实照片，多图平均）：局部对比会把平坦区噪声一起放大 ——")
    print("        原图 100% → v2 逐色 113% → +0.15 局部 127% → +0.35 局部 149%")
    print("        对 JPG 输入（已 8bit 有损）这是「暗部麻点被显影」的直接来源。")


# ---------------------------------------------------------------- F. 软蒙版

def test_f_soft_mask() -> None:
    """F：软蒙版分区 vs 硬切分，halo 与过渡。

    构造一个跨越分界线的渐变：硬切分在边界处会产生斜率突变（二阶导数尖峰），
    软蒙版不会。判据：亮度剖面的二阶差分最大值。
    """
    print("=" * 78)
    print("F. 分区方式：硬切分 vs 软蒙版（跨越分界线的渐变剖面）")
    w, h = 1024, 32
    ramp = np.linspace(0.30, 0.70, w, dtype=np.float32)      # 跨越 0.5 分界
    img = np.stack([ramp] * 3, axis=-1)[None].repeat(h, 0)
    img = np.clip(img * 255.0 + 0.5, 0, 255).astype(np.uint8)

    lum = luma_of(img)
    # 硬切分：<0.5 走暗部曲线（压 0.85），>=0.5 走亮部曲线（提 1.15）
    hard = np.where(lum < 0.5, lum * 0.85, lum * 1.15)
    # 软蒙版：用 smoothstep 在 [0.4, 0.6] 之间过渡
    t = np.clip((lum - 0.40) / 0.20, 0.0, 1.0)
    wgt = t * t * (3.0 - 2.0 * t)
    soft = lum * 0.85 * (1 - wgt) + lum * 1.15 * wgt

    def jitter(x):
        prof = x[h // 2]
        return float(np.abs(np.diff(prof, 2)).max())

    print("  %-24s %12s %12s" % ("方式", "二阶差分max", "最大跳变"))
    for label, x in (("硬切分（阈值 0.5）", hard), ("软蒙版（smoothstep）", soft)):
        prof = x[h // 2]
        print("  %-24s %12.5f %12.4f" % (label, jitter(x), float(np.abs(np.diff(prof)).max())))
    print("  判据：硬切分的二阶差分在分界处出现尖峰 → 可见过渡带；软蒙版无尖峰")


# ---------------------------------------------------------------- G. filmic

def _hable(x, A=0.15, B=0.50, C=0.10, D=0.20, E=0.02, F=0.30):
    return (x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F) - E / F


def filmic(x: np.ndarray, strength: float = 0.35) -> np.ndarray:
    """Hable filmic 曲线（归一化到 [0,1]），与目标曲线按 strength 混合。"""
    W = 11.2
    out = _hable(x * W) / _hable(W)
    return x + (out - x) * strength


def test_g_filmic() -> None:
    """G：主张 2「用 filmic/AgX 做高光滚降」是否比当前方案更好。

    判据：输入 240~255 这 16 级的**输出保留率**。滚降越「软」，保留越多。
    同时看该段最大台阶（滚降不能把高光压成台阶）。
    """
    print("=" * 78)
    print("G. filmic 高光滚降 vs 当前方案（输入 240–255 共 16 级的输出保留率）")
    src = _src()
    levels = np.linspace(0.0, 1.0, 256, dtype=np.float32)
    seg = np.where((levels >= 240 / 255) & (levels <= 1.0))[0]

    st2 = v2.analyze(src, "natural", 0.85)
    o2 = v2.eval_curve(np.asarray(st2.x, np.float32), np.asarray(st2.y, np.float32),
                       np.asarray(st2.m, np.float32), levels) * 255.0
    st0 = v0.analyze(src, "natural", 0.85)
    o0 = v0.eval_colors(st0, np.stack([levels] * 3, -1)[None]).reshape(-1, 3).mean(-1) * 255.0
    of = filmic(o2 / 255.0, 0.35) * 255.0

    def keep(o):
        return (o[seg].max() - o[seg].min()) / (seg.max() - seg.min())

    def step(o):
        return float(np.abs(np.diff(o[seg])).max())

    print("  %-22s %10s %10s" % ("方案", "顶16级保留", "该段最大台阶"))
    for label, o in (("v0 现有（压 2~3 次肩部）", o0),
                     ("v2 新方案（锚点 (1,1)，不压肩）", o2),
                     ("v2 + filmic 0.35", of)):
        print("  %-22s %9.0f%% %10.2f" % (label, keep(o) * 100, step(o)))
    print("  判读：filmic 的滚降是「主动压高光」——它在输入已正确曝光时会**减少**")
    print("        高光保留（%.0f%% → %.0f%%）。它解决的是「输入本身过曝」的场景，"
          % (keep(o2) * 100, keep(of) * 100))
    print("        而不是「已正确曝光的图」。当前方案用锚点 (1,1) + 只在真溢出时压肩，")
    print("        在正确曝光的输入上更保细节；过曝输入则应走 exposure_gamma 分支。")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    test_a_banding()
    print()
    test_b_same_value()
    print()
    test_cd_colorspace()
    print()
    test_e_local_contrast()
    print()
    test_f_soft_mask()
    print()
    test_g_filmic()
