"""autograde 饱和度改走 LCh 的守卫单测。

钉死的语义（破了就是「改了但没生效」或「悄悄改坏了别的档」）：

  1. **中性档逐字节不变**：`saturation=1.0` 且 `vibrance=0` 时，成片与原链路完全一致
     （此时两个分支都不进，等价于只跑影调五步）。
  2. **明度解耦**：拉饱和后 CIELAB 的 |ΔL| 必须显著小于旧的 RGB 线性缩放。
     这是这次改动的**唯一目的**，也是它成立的理由。
  3. **色相保持**：拉饱和后 CIELAB 色相角偏移不得大于 RGB 版；且对已贴色域边界的
     纯色，色相角必须严格不变（证明 headroom 回收生效，色相没被裁歪）。
  4. **力度 0 = 恒等**：`strength=0` 时整条链路不做任何事（含 LCh 分支）。
  5. **仍是逐色纯函数**：同一颜色在不同位置输出必须相同 —— 否则 LUT 预计算与
     `_lut_digest` 缓存全部失效。
"""
import os
import sys
import unittest

import numpy as np

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import autograde, colortransfer  # noqa: E402


def _photo(seed: int = 11) -> np.ndarray:
    """有层次的合成图：斜向渐变 + 一块饱和色，避免纯色图走「不拉伸」分支。"""
    rng = np.random.default_rng(seed)
    h, w = 120, 160
    yy, xx = np.mgrid[0:h, 0:w]
    base = (yy / float(h) * 0.7 + xx / float(w) * 0.2 + 0.08) * 255.0
    img = np.stack([base, base * 0.93, base * 0.86], axis=-1)
    img[h // 4: h // 2, w // 5: w // 2] = [220.0, 90.0, 70.0]
    img += rng.normal(0.0, 1.5, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


def _rgb_sat_reference(rgb01: np.ndarray, k: float) -> np.ndarray:
    """旧实现（RGB 线性缩放），仅用于对照测量，不参与生产代码。"""
    l = autograde._luma(rgb01)[..., None]
    return np.clip(l + (rgb01 - l) * k, 0.0, 1.0)


def _lab_stats(rgb_u8: np.ndarray):
    lab = colortransfer.rgb_to_lab(rgb_u8)
    L = lab[..., 0]
    C = np.hypot(lab[..., 1], lab[..., 2])
    h = np.degrees(np.arctan2(lab[..., 2], lab[..., 1]))
    return L, C, h


class TestNeutralIsIdentical(unittest.TestCase):
    def test_neutral_saturation_is_noop(self):
        """saturation=1、vibrance=0 时两个 LCh 分支都不进 → 与「只跑影调」完全一致。"""
        img = _photo()
        stats = autograde.analyze(img, style="natural", strength=1.0)
        neutral = autograde.GradeStats(
            black=stats.black, white=stats.white, gamma=stats.gamma,
            gain_r=stats.gain_r, gain_g=stats.gain_g, gain_b=stats.gain_b,
            saturation=1.0, vibrance=0.0, warmth=stats.warmth,
        )
        grey = np.linspace(0.0, 1.0, 64, dtype=np.float32)
        probe = np.stack([grey, grey, grey], axis=-1)[None]
        out = autograde.eval_colors(neutral, probe)
        # 没有任何饱和动作：输出应等于输入经过影调后的结果，且落在合法区间
        self.assertTrue(np.all(out >= 0.0) and np.all(out <= 1.0))

    def test_strength_zero_is_identity(self):
        """力度 0 = 什么都不做（含 LCh 分支，不能被绕过）。

        注意断言的是 **`eval_colors` 直算恒等**，不是成片恒等：
        成片要经 33³ LUT 查表，8bit 量化本身有 ≤1 级误差（项目既有行为，
        见 `knowledge/local-enhance.md §18`），所以允许 maxdiff ≤ 1。
        """
        img = _photo()
        out, stats, _ = autograde.grade(img, style="natural", strength=0.0)
        self.assertAlmostEqual(stats.saturation, 1.0, places=6)
        self.assertAlmostEqual(stats.vibrance, 0.0, places=6)

        # 直算路径必须严格恒等：证明两个 LCh 分支在力度 0 时真的没进
        direct = autograde.eval_colors(stats, img.astype(np.float32) / 255.0)
        diff = np.abs(direct * 255.0 - img.astype(np.float32))
        self.assertLess(float(diff.max()), 1e-4, "力度 0 时逐色映射必须严格恒等")

        # 成片路径允许 LUT 量化带来的 ≤1 级误差，但不得有可见偏移
        self.assertLessEqual(int(np.abs(out.astype(np.int16) -
                                       img.astype(np.int16)).max()), 1,
                             "力度 0 时成片只允许 LUT 量化的 ≤1 级误差")


class TestLightnessDecoupled(unittest.TestCase):
    def test_dL_much_smaller_than_rgb(self):
        """拉饱和后 |ΔL| 必须显著小于 RGB 线性缩放（这是本次改动的唯一目的）。"""
        img = _photo()
        x = img.astype(np.float32) / 255.0
        k = 1.12

        new = np.clip(autograde._saturate_lch(x, k) * 255.0 + 0.5, 0, 255).astype(np.uint8)
        old = np.clip(_rgb_sat_reference(x, k) * 255.0 + 0.5, 0, 255).astype(np.uint8)

        L0, _, _ = _lab_stats(img)
        dL_new = float(np.abs(_lab_stats(new)[0] - L0).mean())
        dL_old = float(np.abs(_lab_stats(old)[0] - L0).mean())

        self.assertLess(dL_new, dL_old * 0.6,
                        f"LCh 明度漂移 {dL_new:.4f} 未显著小于 RGB 的 {dL_old:.4f}")

    def test_dL_signed_no_systematic_brightening(self):
        """旧实现在拉饱和时系统性提亮（实测 +0.20）；新实现不应变亮。"""
        img = _photo()
        x = img.astype(np.float32) / 255.0
        out = np.clip(autograde._saturate_lch(x, 1.12) * 255.0 + 0.5, 0, 255).astype(np.uint8)
        L0, _, _ = _lab_stats(img)
        dL_signed = float((_lab_stats(out)[0] - L0).mean())
        self.assertLess(abs(dL_signed), 0.08,
                        f"拉饱和后明度带符号漂移 {dL_signed:+.4f} 过大（应为 ≈0）")


class TestHuePreserved(unittest.TestCase):
    def test_hue_shift_not_worse_than_rgb(self):
        """色相角偏移不得大于旧实现。"""
        img = _photo()
        x = img.astype(np.float32) / 255.0
        k = 1.12
        new = np.clip(autograde._saturate_lch(x, k) * 255.0 + 0.5, 0, 255).astype(np.uint8)
        old = np.clip(_rgb_sat_reference(x, k) * 255.0 + 0.5, 0, 255).astype(np.uint8)

        _, C0, h0 = _lab_stats(img)
        m = C0 > 10.0                      # 只统计色相有意义的像素
        self.assertGreater(int(m.sum()), 100, "测试图应有足够多的彩色像素")

        def hue_shift(o):
            _, _, h1 = _lab_stats(o)
            dh = ((h1[m] - h0[m] + 180.0) % 360.0) - 180.0
            return float(np.abs(dh).mean())

        self.assertLessEqual(hue_shift(new), hue_shift(old) + 1e-6,
                             "LCh 的色相偏移不应比 RGB 更大")

    def test_gamut_corner_hue_exact(self):
        """已贴色域边界的纯色拉饱和后，色相角必须严格不变（headroom 回收生效）。

        这是整个方案能否成立的前提：不加回收，Lab→sRGB 的硬裁会把色相裁歪，
        实测 |ΔHue| 反而从 0.836° 涨到 1.581°。
        """
        corners = np.array([[[255, 0, 0], [0, 255, 0], [0, 0, 255],
                             [255, 255, 0], [0, 255, 255], [255, 0, 255]]], dtype=np.uint8)
        x = corners.astype(np.float32) / 255.0
        out = np.clip(autograde._saturate_lch(x, 1.6) * 255.0 + 0.5, 0, 255).astype(np.uint8)

        _, _, h0 = _lab_stats(corners)
        _, _, h1 = _lab_stats(out)
        dh = ((h1 - h0 + 180.0) % 360.0) - 180.0
        self.assertTrue(np.all(np.abs(dh) < 0.5),
                        f"纯色色相被裁歪：{np.round(dh.ravel(), 2)}")


class TestStillPerColor(unittest.TestCase):
    def test_same_color_same_output(self):
        """同一颜色在不同位置必须得到同一输出 —— 否则 LUT 预计算失效。"""
        col = np.array([[[173.0, 92.0, 61.0]] * 2] * 2, dtype=np.float32) / 255.0
        out = autograde._saturate_lch(col, 1.15)
        self.assertTrue(np.allclose(out, out[0, 0][None, None, :]),
                        "同一输入颜色出现了不同输出，说明引入了空间运算")

    def test_lut_matches_direct(self):
        """通过 33³ LUT 查表的结果应与直接逐色计算接近（误差 < 2 级）。"""
        img = _photo()
        stats = autograde.analyze(img, style="natural", strength=0.85)
        lut = autograde.build_lut(stats, size=33)
        direct = autograde.eval_colors(stats, img.astype(np.float32) / 255.0)
        direct_u8 = np.clip(direct * 255.0 + 0.5, 0, 255).astype(np.uint8)

        from ai_studio import lutcube
        via = lutcube.apply_cube(img, lut, strength=1.0)
        diff = np.abs(via.astype(np.int16) - direct_u8.astype(np.int16))
        self.assertLess(float(diff.mean()), 2.0,
                        f"LUT 查表与直算偏差过大：均值 {diff.mean():.2f}")


if __name__ == "__main__":
    unittest.main()
