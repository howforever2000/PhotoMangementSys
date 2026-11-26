"""肤色保真（FEAT-208）的守卫单测。

钉死的语义（破了就是「改了但没生效」或「悄悄改坏了别的」）：

  1. **已在目标处 = 恒等**：锚点与当前统计一致且参数为 0 时逐字节不变
     （不做无谓的 Lab 往返）。
  2. **还原收敛**：人为把色相角转偏后，校正能把中位 h 拉回锚点附近。
  3. **偏置方向**：hue_shift>0 → 结果比锚点更黄（h 增大）。
  4. **彩度加性平移保 C_sd**：这是「加性」而不是「比例」存在的理由
     （比例缩放会把面部彩度层次整体压扁）。
  5. **掩码外零改动**：头发/衣物/背景像素逐字节不变（硬约束）。
  6. **高光保护**：L ≥ L_SAFE 的反光像素明度不动（local-enhance §9 纪律）。
  7. **色相/彩度校正幂等**：校正到位后二次应用不再漂移（明度提亮是
     一次性手感量，不在此列 —— 与整条链路非幂等同一口径）。
  8. **低彩度像素不被染色**：C < C_LOW 的灰区不参与色相旋转与彩度平移。
  9. **参数夹取**：任意来源的越界值必须被夹回合法区间。
"""
import os
import sys
import unittest

import numpy as np

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import colortransfer, skintone  # noqa: E402


def _skin_rgb(h_deg: float, chroma: float, light: float,
              size: tuple[int, int] = (180, 220), seed: int = 7) -> np.ndarray:
    """造一张 L/C/h 已知的「皮肤」测试图：带渐变与噪声（不是均匀色块）。"""
    rng = np.random.default_rng(seed)
    h, w = size
    yy, xx = np.mgrid[0:h, 0:w]
    L = light + (xx / w - 0.5) * 6.0 + rng.normal(0.0, 1.2, size)
    C = chroma + rng.normal(0.0, 1.5, size)
    ang = h_deg + rng.normal(0.0, 4.0, size)
    lab = np.stack([
        np.clip(L, 20.0, 92.0),
        C * np.cos(np.radians(ang)),
        C * np.sin(np.radians(ang)),
    ], axis=-1)
    return colortransfer.lab_to_rgb(lab)


def _rotate_hue(rgb_u8: np.ndarray, delta_deg: float) -> np.ndarray:
    """把整图色相角转 delta 度（模拟滤镜把肤色推偏），L/C 保持。"""
    lab = colortransfer.rgb_to_lab(rgb_u8).astype(np.float32)
    C = np.hypot(lab[..., 1], lab[..., 2])
    ang = np.radians(np.degrees(np.arctan2(lab[..., 2], lab[..., 1])) + delta_deg)
    new = np.stack([lab[..., 0], C * np.cos(ang), C * np.sin(ang)], axis=-1)
    return colortransfer.lab_to_rgb(new)


def _median_stats(rgb_u8: np.ndarray, mask: np.ndarray,
                  cheek: np.ndarray | None = None) -> dict[str, float]:
    lab = colortransfer.rgb_to_lab(rgb_u8)
    sel = lab[mask > 0]
    mean_a = float(np.mean(sel[:, 1]))
    mean_b = float(np.mean(sel[:, 2]))
    out = {
        "a": mean_a, "b": mean_b,
        "hue": float(np.degrees(np.arctan2(mean_b, mean_a))),
        "chroma": float(np.hypot(mean_a, mean_b)),
    }
    if cheek is not None:
        cm = (mask > 0) & (cheek > 0)
        out["cheek_a"] = float(np.median(lab[cm][:, 1]))
    return out


def _wrap(d: float) -> float:
    return (d + 180.0) % 360.0 - 180.0


FULL = np.ones((180, 220), dtype=np.float32)
ZERO = np.zeros((180, 220), dtype=np.float32)
RIGHT_CHEEK = skintone.cheek_region((180, 220))
OFF_PARAMS = skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=0.0, blush=0.0)

# 评估口径与算法统计口径一致：排除羽化带（那里的像素是原值与校正值的混合，
# 混进统计会把收敛残差虚报成 ~1.8°）。脸的「本体」就是羽化带以内的部分。
CORE = skintone._erode(FULL, skintone.FEATHER_RADIUS) > 0.5


class TestIdentityAtTarget(unittest.TestCase):
    def test_already_at_target_is_byte_identical(self):
        """锚点=当前统计且参数全 0 → 早退，逐字节不变。"""
        img = _skin_rgb(50.0, 20.0, 65.0)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(img, FULL, RIGHT_CHEEK, anchor, OFF_PARAMS)
        self.assertTrue(np.array_equal(out, img))

class TestRestoreAndBias(unittest.TestCase):
    def test_restore_recovers_filtered_hue(self):
        """滤镜把 h 转偏 −10° 后，零参数校正应把中位 h 拉回锚点 ±0.6°。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=11)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        filtered = _rotate_hue(src, -10.0)
        out = skintone.correct_region(filtered, FULL, RIGHT_CHEEK, anchor, OFF_PARAMS)
        s = _median_stats(out, CORE)
        self.assertLess(abs(_wrap(s["hue"] - anchor.hue)), 0.6)

    def test_bias_pushes_toward_yellow(self):
        """hue_shift=+3 → 结果中位 h 应落在锚点+3 附近（方向性：更黄）。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=12)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        filtered = _rotate_hue(src, -10.0)
        out = skintone.correct_region(
            filtered, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=3.0, chroma=0.0, brighten=0.0, blush=0.0),
        )
        s = _median_stats(out, CORE)
        self.assertLess(abs(_wrap(s["hue"] - (anchor.hue + 3.0))), 0.8)

    def test_blush_raises_cheek_a(self):
        """blush>0 → 颊部 a* 中位应高于全脸 a* 中位（嫩红集中在苹果肌）。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=13)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            src, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=0.0, blush=1.5),
        )
        all_a = _median_stats(out, CORE)
        cheek_a = _median_stats(out, CORE, RIGHT_CHEEK)
        # 全脸 a 基本不变（只有颊部 +blush），颊部应高出 blush 的大部分
        self.assertGreater(cheek_a["cheek_a"] - all_a["a"], 0.6)


class TestChromaAdditive(unittest.TestCase):
    def test_additive_shift_preserves_sd(self):
        """加性平移（不是比例缩放）：校正前后彩度标准差几乎不变。"""
        src = _skin_rgb(50.0, 18.0, 65.0, seed=14)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        filtered = _rotate_hue(src, 0.0)  # 只走 Lab 往返
        before = colortransfer.rgb_to_lab(filtered)[FULL > 0][:, 1:3]
        sd_before = float(np.std(np.hypot(before[:, 0], before[:, 1])))
        out = skintone.correct_region(
            filtered, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=3.0, brighten=0.0, blush=0.0),
        )
        after = colortransfer.rgb_to_lab(out)[CORE][:, 1:3]
        sd_after = float(np.std(np.hypot(after[:, 0], after[:, 1])))
        self.assertLess(abs(sd_after - sd_before), 0.5)


class TestGenderTier(unittest.TestCase):
    """性别侧重（FEAT-209）：男性档只还原 + 轻提亮，不加白黄也不加嫩红。"""

    def test_female_keeps_user_params(self):
        p = skintone.SkinParams(hue_shift=4.0, chroma=2.0, brighten=3.0, blush=2.5)
        self.assertEqual(skintone.for_gender(p, True), p)

    def test_male_drops_bias_and_blush(self):
        p = skintone.SkinParams(hue_shift=4.0, chroma=2.0, brighten=3.0, blush=2.5)
        male = skintone.for_gender(p, False)
        self.assertEqual(male.hue_shift, 0.0)
        self.assertEqual(male.chroma, 0.0)
        self.assertEqual(male.blush, 0.0)
        self.assertAlmostEqual(male.brighten, 3.0 * skintone.MALE_BRIGHTEN_RATIO)

    def test_male_still_restores_original_hue(self):
        """男性档不做偏置，但「还原」由锚点差算出 —— 仍要把偏色拉回原图。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=21)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        filtered = _rotate_hue(src, -10.0)
        male = skintone.for_gender(skintone.SkinParams(), False)
        out = skintone.correct_region(filtered, FULL, RIGHT_CHEEK, anchor, male)
        s = _median_stats(out, CORE)
        self.assertLess(abs(_wrap(s["hue"] - anchor.hue)), 0.6)
        # 男性档不加嫩红：颊部与全脸的 a* 差值在校正前后基本不变
        before = _median_stats(filtered, CORE)
        before_cheek = _median_stats(filtered, CORE, RIGHT_CHEEK)["cheek_a"]
        after_cheek = _median_stats(out, CORE, RIGHT_CHEEK)["cheek_a"]
        gap_before = before_cheek - before["a"]
        gap_after = after_cheek - s["a"]
        self.assertLess(abs(gap_after - gap_before), 0.4)


class TestBrightenCurve(unittest.TestCase):
    """提亮曲线（FEAT-209）：中间调拿满权重，高光仍完全保护。"""

    def test_midtone_gets_full_amount(self):
        img = _skin_rgb(50.0, 20.0, 60.0, seed=22)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            img, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=2.0, blush=0.0),
        )
        before = colortransfer.rgb_to_lab(img)[..., 0][CORE].mean()
        after = colortransfer.rgb_to_lab(out)[..., 0][CORE].mean()
        # L≈60 落在全量带（≤L_FULL=70）：抬升量应接近标称 2.0，而不是旧的 ~1.0
        self.assertGreater(after - before, 1.6)


class TestDarken(unittest.TestCase):
    """压暗（brighten < 0，交互简化 2026-11-26）：中间调拿满权重，深阴影完全保护。"""

    def test_negative_brighten_is_not_noop(self):
        """负提亮必须被识别为有效操作（守卫 is_noop 的 abs 判断）。"""
        self.assertFalse(
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=-2.0, blush=0.0).is_noop()
        )

    def test_darken_midtone_gets_full_amount(self):
        img = _skin_rgb(50.0, 20.0, 60.0, seed=22)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            img, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=-2.0, blush=0.0),
        )
        before = colortransfer.rgb_to_lab(img)[..., 0][CORE].mean()
        after = colortransfer.rgb_to_lab(out)[..., 0][CORE].mean()
        # L≈60 在全量带（≥L_DARK_FULL=30）：压暗量应接近标称 -2.0（与提亮镜像）
        self.assertLess(after - before, -1.6)

    def test_shadow_protected(self):
        """L ≤ L_DARK_SAFE 的深阴影：brighten 不压暗（其余量不动时逐字节不变）。"""
        img = _skin_rgb(50.0, 20.0, 65.0, seed=24)
        img = img.astype(np.float32).copy()
        img[40:80, 40:120] = 30.0           # 一块深阴影（L*≈11，≤L_DARK_SAFE=22）
        img = np.clip(img + 0.5, 0, 255).astype(np.uint8)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            img, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=-2.0, blush=0.0),
        )
        diff = np.abs(out.astype(np.int16) - img.astype(np.int16)).max(axis=2)
        self.assertEqual(int(diff[40:80, 40:120].max()), 0)


class TestToneContract(unittest.TestCase):
    """双契约 from_raw（交互简化 2026-11-26 + 用户口径「自然白」）：

    - 单滑条契约（无 hue_shift/chroma 键）：偏置随 brighten 联动推导；
    - 旧四参契约（显式 hue_shift/chroma）：原样夹取，本地美颜档位不受影响。
    """

    def test_single_slider_neutral(self):
        p = skintone.SkinParams.from_raw({"brighten": 0, "blush": 0.15})
        self.assertEqual(p.hue_shift, 0.0)
        self.assertEqual(p.chroma, 0.0)
        self.assertEqual(p.brighten, 0.0)
        self.assertEqual(p.blush, 0.15)

    def test_single_slider_whiten_derives_natural_bias(self):
        # brighten=+4（最白）→ 白黄+透亮正好是实测人像档 3.0°/1.5C
        p = skintone.SkinParams.from_raw({"brighten": 4.0})
        self.assertAlmostEqual(p.hue_shift, skintone.WHITEN_HUE, places=6)
        self.assertAlmostEqual(p.chroma, skintone.WHITEN_CHROMA, places=6)
        # 半幅 → 线性减半：幅度小=偏置轻，保证「自然白」而非干提曝光
        half = skintone.SkinParams.from_raw({"brighten": 2.0})
        self.assertAlmostEqual(half.hue_shift, skintone.WHITEN_HUE * 0.5, places=6)
        self.assertAlmostEqual(half.chroma, skintone.WHITEN_CHROMA * 0.5, places=6)

    def test_single_slider_darken_derives_warm_tan(self):
        # brighten=-4（最黑）→ 反向收白黄（负偏红棕）+ 轻微增饱和（晒黑暖调，不偏灰）
        p = skintone.SkinParams.from_raw({"brighten": -4.0})
        self.assertAlmostEqual(p.hue_shift, -skintone.DARKEN_HUE, places=6)
        self.assertAlmostEqual(p.chroma, skintone.DARKEN_CHROMA, places=6)
        self.assertEqual(p.brighten, -4.0)

    def test_single_slider_blush_defaults_5_percent(self):
        p = skintone.SkinParams.from_raw({"brighten": 1.0})
        self.assertAlmostEqual(p.blush, 0.15, places=6)   # 0~3 量程的 5%

    def test_legacy_contract_unchanged(self):
        """显式四键（本地美颜档位 / 旧存档）不做联动，原样夹取。"""
        p = skintone.SkinParams.from_raw(
            {"hue_shift": 3.0, "chroma": 1.5, "brighten": 2.2, "blush": 1.8}
        )
        self.assertEqual((p.hue_shift, p.chroma, p.brighten, p.blush), (3.0, 1.5, 2.2, 1.8))


class TestCheekFeather(unittest.TestCase):
    """颊部硬边（FEAT-209）：嫩红必须软边，矩形边界两侧不能出现阶跃色块。"""

    def test_blush_boundary_is_soft(self):
        img = _skin_rgb(50.0, 20.0, 65.0, seed=23)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        p = skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=0.0, blush=3.0)
        out = skintone.correct_region(img, FULL, RIGHT_CHEEK, anchor, p)
        d = colortransfer.rgb_to_lab(out)[..., 1] - colortransfer.rgb_to_lab(img)[..., 1]
        h, w = d.shape
        col = int(0.46 * (w - 1))
        yy = np.arange(h)[:, None]
        band = yy > int(0.42 * h)
        inside = band & (np.arange(w)[None, :] >= col - 2) & (np.arange(w)[None, :] < col)
        outside = band & (np.arange(w)[None, :] >= col) & (np.arange(w)[None, :] < col + 2)
        step = float(d[inside].mean() - d[outside].mean())
        self.assertLess(abs(step), 0.6)


class TestGuards(unittest.TestCase):
    def test_outside_mask_byte_identical(self):
        """掩码外像素逐字节不变（头发/衣物/背景安全的硬约束）。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=15)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        mask = np.zeros(FULL.shape, dtype=np.float32)
        mask[:, : 110] = 1.0
        cheek = skintone.cheek_region(FULL.shape)
        out = skintone.correct_region(
            src, mask, cheek, anchor,
            skintone.SkinParams(hue_shift=5.0, chroma=2.0, brighten=2.0, blush=2.0),
        )
        diff = np.abs(out.astype(np.int16) - src.astype(np.int16)).max(axis=2)
        self.assertEqual(int(diff[mask <= 0].max()), 0)

    def test_highlight_protected(self):
        """L ≥ L_SAFE 的反光像素：brighten 不提亮（其余量不动时逐字节不变）。"""
        img = _skin_rgb(50.0, 20.0, 65.0, seed=16)
        img = img.astype(np.float32).copy()
        img[40:80, 40:120] = 245.0          # 一块高光斑
        img = np.clip(img + 0.5, 0, 255).astype(np.uint8)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            img, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=2.0, blush=0.0),
        )
        diff = np.abs(out.astype(np.int16) - img.astype(np.int16)).max(axis=2)
        self.assertEqual(int(diff[40:80, 40:120].max()), 0)

    def test_low_chroma_pixels_not_tinted(self):
        """C < C_LOW 的灰区不参与色相旋转与色度平移。"""
        img = _skin_rgb(50.0, 20.0, 65.0, seed=17)
        img = img.astype(np.float32).copy()
        img[40:80, 40:120] = 200.0          # 一块低彩度斑（反光）
        img = np.clip(img + 0.5, 0, 255).astype(np.uint8)
        anchor = skintone.anchor_from_pixels(img, FULL, RIGHT_CHEEK)
        out = skintone.correct_region(
            img, FULL, RIGHT_CHEEK, anchor,
            skintone.SkinParams(hue_shift=6.0, chroma=4.0, brighten=0.0, blush=0.0),
        )
        diff = np.abs(out.astype(np.int16) - img.astype(np.int16)).max(axis=2)
        self.assertLessEqual(int(diff[40:80, 40:120].max()), 1)

    def test_hue_chroma_correction_is_idempotent(self):
        """校正到位后二次应用（同锚点）不再漂移：中位 h/C 变化 < 0.3。"""
        src = _skin_rgb(50.0, 20.0, 65.0, seed=18)
        anchor = skintone.anchor_from_pixels(src, FULL, RIGHT_CHEEK)
        filtered = _rotate_hue(src, -10.0)
        p = skintone.SkinParams(hue_shift=3.0, chroma=1.5, brighten=0.0, blush=0.0)
        once = skintone.correct_region(filtered, FULL, RIGHT_CHEEK, anchor, p)
        twice = skintone.correct_region(once, FULL, RIGHT_CHEEK, anchor, p)
        s1 = _median_stats(once, CORE)
        s2 = _median_stats(twice, CORE)
        self.assertLess(abs(_wrap(s2["hue"] - s1["hue"])), 0.3)
        self.assertLess(abs(s2["chroma"] - s1["chroma"]), 0.3)

    def test_params_clamped(self):
        raw = {"hue_shift": 99, "chroma": -99, "brighten": 50, "blush": "abc"}
        p = skintone.SkinParams.from_raw(raw)
        self.assertEqual(p.hue_shift, 8.0)
        self.assertEqual(p.chroma, -6.0)
        self.assertEqual(p.brighten, 4.0)
        self.assertEqual(p.blush, 0.15)     # 非法回落默认（中性亮度 + 5% 红嫩）
        # 双向值域下界（压暗侧）同样要夹取
        neg = skintone.SkinParams.from_raw({"brighten": -50})
        self.assertEqual(neg.brighten, -4.0)

    def test_wrap_deg(self):
        self.assertAlmostEqual(skintone.wrap_deg(20.0), 20.0)
        self.assertAlmostEqual(skintone.wrap_deg(400.0), 40.0)
        self.assertAlmostEqual(skintone.wrap_deg(-350.0), 10.0)
        self.assertAlmostEqual(skintone.wrap_deg(-190.0), 170.0)

    def test_correction_amounts_wrap(self):
        """锚点 h=10、当前 h=350 → dh 应为 +20（最短弧），不是 -340。"""
        anchor = skintone.SkinAnchor(hue=10.0, chroma=20.0, cheek_a=14.0)
        cur = {
            "hue": 350.0, "chroma": 20.0, "cheek_a": 14.0,
            "mean_a": 20.0 * np.cos(np.radians(350.0)),
            "mean_b": 20.0 * np.sin(np.radians(350.0)),
        }
        p = skintone.SkinParams(hue_shift=0.0, chroma=0.0, brighten=0.0, blush=0.0)
        dh, dax, day = skintone.correction_amounts(anchor, cur, p)
        self.assertAlmostEqual(dh, 20.0, places=4)
        self.assertAlmostEqual(dax, 0.0, places=3)
        self.assertAlmostEqual(day, 0.0, places=3)


if __name__ == "__main__":
    unittest.main()
