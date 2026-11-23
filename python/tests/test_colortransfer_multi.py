"""参考图调色·多参考图共性单测（FEAT-211 口径）。

钉死的语义：
  1. **等权合并 = 混池统计**：`merge_reference_stats` 的合并方差用全方差公式
     `E[σ²] + Var[μ]`，数学上等价于把所有参考图像素混进一个样本池再算统计；
  2. **单张参考图 = 旧单图链路**：merge 单条目直返，迁移结果与
     `reinhard_transfer`（逐像素传图）完全一致，旧行为零回归；
  3. **直方图多图 = 像素拼接**：`histogram_ref_sorted` 接受列表时逐通道 concat，
     匹配结果与手动拼接参考像素一致；
  4. **l_weight=0 完全保留素材明暗**（只迁 a/b 色度）；
  5. **链条多路径参数解析**：`|` 分隔、去重保序（与后端 `_split_ref_paths` 同口径）。
"""
import sys
import unittest
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from ai_studio import colortransfer  # noqa: E402


def _photo(seed: int = 7) -> np.ndarray:
    """有层次、有颜色的合成素材图：渐变 + 彩块 + 轻噪声。"""
    rng = np.random.default_rng(seed)
    h, w = 96, 128
    yy, xx = np.mgrid[0:h, 0:w]
    base = (yy / float(h) * 0.8 + 0.1) * 255.0
    img = np.stack([base, base * 0.92, base * 0.85], axis=-1)
    img[h // 3 : 2 * h // 3, w // 4 : 3 * w // 4] = [210.0, 120.0, 90.0]
    img += rng.normal(0.0, 2.0, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


def _tint(seed: int, rgb: tuple[float, float, float]) -> np.ndarray:
    """一张指定主色调的合成参考图（有像素方差，统计量才有意义）。"""
    rng = np.random.default_rng(seed)
    img = np.empty((64, 64, 3), dtype=np.float64)
    img[:] = rgb
    img += rng.normal(0.0, 6.0, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


class TestMergeReferenceStats(unittest.TestCase):
    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            colortransfer.merge_reference_stats([])

    def test_single_entry_identity(self):
        s = colortransfer.reference_stats(_tint(1, (180, 120, 90)))
        self.assertIs(colortransfer.merge_reference_stats([s]), s, "单条目必须原样直返")

    def test_merge_equals_pooled_stats(self):
        """合并均值/方差 == 两张参考图像素直接拼接后的整体统计（全方差公式）。"""
        a = _tint(1, (180, 120, 90))
        b = _tint(2, (60, 90, 150))
        merged = colortransfer.merge_reference_stats(
            [colortransfer.reference_stats(a), colortransfer.reference_stats(b)]
        )
        pooled = colortransfer.reference_stats(np.concatenate([a.reshape(-1, 3), b.reshape(-1, 3)]))
        for i in range(3):
            self.assertAlmostEqual(merged["mean"][i], pooled["mean"][i], delta=0.05)
            # pooled 的 std 带 +1e-6 偏置，合并结果也带，允许同一量级的误差
            self.assertAlmostEqual(merged["std"][i], pooled["std"][i], delta=0.05)

    def test_merged_stats_transfer_moves_toward_common_tone(self):
        """合并参考图迁移后，素材色度均值应更接近两参考图的色度均值（取中）。"""
        src = _photo()
        a = _tint(1, (180, 120, 90))
        b = _tint(2, (60, 90, 150))
        merged = colortransfer.merge_reference_stats(
            [colortransfer.reference_stats(a), colortransfer.reference_stats(b)]
        )
        out = colortransfer.reinhard_transfer_stats(src, merged)
        # a 的 a* 通道偏高、b 偏低，合并均值落在两者之间；迁移后素材的 a 均值应向中间收
        target = merged["mean"][1]
        src_lab = colortransfer.rgb_to_lab(src)
        out_lab = colortransfer.rgb_to_lab(out)
        self.assertLess(abs(out_lab[..., 1].mean() - target), abs(src_lab[..., 1].mean() - target))

    def test_stats_divergence(self):
        a = colortransfer.reference_stats(_tint(1, (180, 120, 90)))
        b = colortransfer.reference_stats(_tint(2, (60, 90, 150)))
        c = colortransfer.reference_stats(_tint(3, (60, 90, 150)))
        self.assertEqual(colortransfer.stats_divergence([a]), 0.0)
        self.assertGreater(colortransfer.stats_divergence([a, b]), 20.0)
        self.assertLess(colortransfer.stats_divergence([b, c]), 5.0, "同色系参考图分歧度应很小")


class TestSingleRefEquivalence(unittest.TestCase):
    def test_stats_path_equals_pixel_path(self):
        """单张参考图：预提取统计迁移 == 旧版逐像素传图（逐字节一致）。"""
        src = _photo()
        ref = _tint(1, (180, 120, 90))
        via_stats = colortransfer.reinhard_transfer_stats(src, colortransfer.reference_stats(ref))
        via_pixel = colortransfer.reinhard_transfer(src, ref)
        self.assertTrue(np.array_equal(via_stats, via_pixel))

    def test_strength_zero_is_identity(self):
        src = _photo()
        ref = _tint(1, (180, 120, 90))
        out = colortransfer.reinhard_transfer_stats(src, colortransfer.reference_stats(ref), strength=0.0)
        self.assertTrue(np.array_equal(out, src), "strength=0 必须逐字节恒等")


class TestLWeight(unittest.TestCase):
    def test_zero_keeps_luminance(self):
        """l_weight=0：L 通道不被迁移，素材明暗结构保留（色度仍动）。"""
        src = _photo()
        ref = _tint(1, (180, 120, 90))
        out = colortransfer.reinhard_transfer_stats(src, colortransfer.reference_stats(ref), l_weight=0.0)
        src_l = colortransfer.rgb_to_lab(src)[..., 0]
        out_l = colortransfer.rgb_to_lab(out)[..., 0]
        self.assertLess(np.abs(out_l - src_l).mean(), 2.0, "L 通道应几乎不动")
        out_ab = colortransfer.rgb_to_lab(out)[..., 1]
        self.assertGreater(np.abs(out_ab - colortransfer.rgb_to_lab(src)[..., 1]).mean(), 1.0, "a 通道应被迁移")

    def test_one_matches_full_transfer(self):
        """l_weight=1 与不传 l_weight 的全迁移逐字节一致（默认行为零回归）。"""
        src = _photo()
        ref = _tint(1, (180, 120, 90))
        stats = colortransfer.reference_stats(ref)
        full = colortransfer.reinhard_transfer_stats(src, stats, l_weight=1.0)
        self.assertTrue(np.array_equal(full, colortransfer.reinhard_transfer(src, ref)))


class TestHistogramMulti(unittest.TestCase):
    def test_multi_ref_equals_concat(self):
        """多张参考图：排序列拼接 == 手动拼接像素后的单图匹配（逐字节一致）。"""
        src = _photo()
        a = _tint(1, (180, 120, 90))
        b = _tint(2, (60, 90, 150))
        via_multi = colortransfer.histogram_match_sorted(src, colortransfer.histogram_ref_sorted([a, b]))
        pooled = colortransfer.histogram_match(src, np.concatenate([a.reshape(-1, 3), b.reshape(-1, 3)]))
        self.assertTrue(np.array_equal(via_multi, pooled))

    def test_ref_sorted_accepts_single_array(self):
        a = _tint(1, (180, 120, 90))
        single = colortransfer.histogram_ref_sorted(a)
        listed = colortransfer.histogram_ref_sorted([a])
        for s, l in zip(single, listed):
            self.assertTrue(np.array_equal(s, l))

    def test_order_does_not_matter(self):
        """直方图匹配只看排序分位，参考图列表顺序不影响结果。"""
        src = _photo()
        a = _tint(1, (180, 120, 90))
        b = _tint(2, (60, 90, 150))
        ab = colortransfer.histogram_match_sorted(src, colortransfer.histogram_ref_sorted([a, b]))
        ba = colortransfer.histogram_match_sorted(src, colortransfer.histogram_ref_sorted([b, a]))
        self.assertTrue(np.array_equal(ab, ba))


class TestCubeFromStats(unittest.TestCase):
    def test_cube_from_stats_equals_pixel_cube(self):
        ref = _tint(1, (180, 120, 90))
        a = colortransfer.build_cube_text(ref)
        b = colortransfer.build_cube_text_from_stats(colortransfer.reference_stats(ref))
        self.assertEqual(a, b)

    def test_cube_size_and_header(self):
        text = colortransfer.build_cube_text_from_stats(colortransfer.reference_stats(_tint(1, (180, 120, 90))), size=33)
        self.assertTrue(text.startswith("TITLE") or "LUT_3D_SIZE" in text)
        self.assertIn("LUT_3D_SIZE 33", text)
        # 33^3 数据行 + 头部 4 行（TITLE / LUT_3D_SIZE / DOMAIN_MIN / DOMAIN_MAX）
        self.assertEqual(len([l for l in text.splitlines() if l.strip()]), 33 ** 3 + 4)


class TestSplitRefPaths(unittest.TestCase):
    """链条步参数的 `|` 分隔解析（与 ops_local._split_ref_paths 同口径）。"""

    def test_split_and_dedupe(self):
        from ai_studio.ops_local import _split_ref_paths

        self.assertEqual(_split_ref_paths("a.png|b.png"), ["a.png", "b.png"])
        self.assertEqual(_split_ref_paths("a.png|a.png"), ["a.png"], "重复路径去重保序")
        self.assertEqual(_split_ref_paths(" a.png | b.png "), ["a.png", "b.png"], "空白容忍")
        self.assertEqual(_split_ref_paths(""), [])
        self.assertEqual(_split_ref_paths("a.png"), ["a.png"], "单路径兼容旧链路")


if __name__ == "__main__":
    unittest.main()
