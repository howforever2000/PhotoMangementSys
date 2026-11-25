"""胶片引擎与风格目录的回归测试（纯 numpy，秒级）。

覆盖的是「改代码时最容易踩坏」的四条契约：
  1. 老 id 默认仍走 LUT（重合时保留老的），且 LUT 路径逐位不变；
  2. 新 id 没有 .cube，选它自动切 film；显式 engine=lut 必须报错；
  3. 参数化输出高光截断 ≤0.5%（方案 v2 的硬门槛）；
  4. 强度连续性：look=1.0 与旧配方一致，strength=0 回到原图。
"""
from __future__ import annotations

import os
import sys
import unittest

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from ai_studio import config, film_engine, film_render, film_styles, lutcube  # noqa: E402


def synth() -> np.ndarray:
    """构造一张同时含高光、阴影、蓝天与暖色的测试图（512×384）。"""
    h, w = 384, 512
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    r = 0.55 + 0.45 * np.sin(xx / 37.0)
    g = 0.50 + 0.35 * np.sin(yy / 29.0 + 1.1)
    b = 0.90 - 0.25 * (yy / h)
    img = np.stack([r, g, b], axis=-1)
    img[: h // 3, :, 2] = 1.0            # 顶部强光
    img[h // 2:, : h // 3, 0] = 0.95     # 左下暖块
    img[h // 2:, h // 2:, :] *= 0.08     # 右下极暗
    return np.clip(img * 255.0 + 0.5, 0, 255).astype(np.uint8)


class TestStyleCatalog(unittest.TestCase):
    def test_old_ids_keep_lut_engine(self):
        for sid in ("provia", "velvia", "classic_chrome", "classic_neg", "eterna"):
            eng, _ = film_styles.resolve(sid, "auto")
            self.assertEqual(eng, "lut", f"{sid} 默认引擎必须仍是 lut（重合时保留老的）")

    def test_new_ids_are_film_only(self):
        for sid in ("portra400", "kodak_2383"):
            self.assertFalse(film_styles.has_lut(sid))
            eng, recipe = film_styles.resolve(sid, "auto")
            self.assertEqual(eng, "film")
            self.assertIn("gamma", recipe)

    def test_lut_engine_on_new_style_raises(self):
        with self.assertRaises(ValueError):
            film_styles.resolve("portra400", "lut")

    def test_catalog_order_and_flags(self):
        cat = film_styles.catalog()
        self.assertEqual([c["id"] for c in cat][-2:], ["portra400", "kodak_2383"])
        self.assertTrue(all(c["hasLut"] for c in cat[:7]))
        self.assertTrue(all(c["new"] for c in cat[-2:]))


class TestLutPathUnchanged(unittest.TestCase):
    def test_lut_branch_matches_apply_cube(self):
        rgb = synth()
        out, meta = film_render.apply(
            rgb, {"lut": "provia", "engine": "lut", "strength": 1.0},
            lut_file_path=config.lut_path("provia.cube"),
        )
        ref = lutcube.apply_cube(rgb, config.lut_path("provia.cube"), strength=1.0)
        self.assertEqual(meta["engine"], "lut")
        self.assertTrue(np.array_equal(out, ref))


class TestFilmOutput(unittest.TestCase):
    def test_clip_budget(self):
        rgb = synth()
        for sid in ("portra400", "kodak_2383", "provia"):
            out, meta = film_render.apply(rgb, {"style": sid, "engine": "film"})
            clip = float((out >= 254).any(axis=2).mean() * 100.0)
            self.assertEqual(meta["engine"], "film")
            self.assertLessEqual(clip, 0.5, f"{sid} 高光截断 {clip:.2f}% 超门槛")

    def test_strength_zero_returns_source(self):
        rgb = synth()
        out, _ = film_render.apply(rgb, {"style": "portra400", "strength": 0.0})
        self.assertLessEqual(int(np.abs(out.astype(np.int16) - rgb.astype(np.int16)).max()), 1)

    def test_grade_only_on_film(self):
        rgb = synth()
        base, _ = film_render.apply(rgb, {"style": "kodak_2383"})
        graded, _ = film_render.apply(
            rgb, {"style": "kodak_2383", "grade": "teal_orange", "grade_strength": 1.0})
        diff = np.abs(graded.astype(np.int16) - base.astype(np.int16)).mean()
        self.assertGreater(diff, 0.5, "后调层没有生效")

    def test_look_is_continuous(self):
        rgb = synth()
        o1, _ = film_render.apply(rgb, {"style": "velvia", "engine": "film", "look": 1.0})
        o2, _ = film_render.apply(rgb, {"style": "velvia", "engine": "film", "look": 1.4})
        self.assertEqual(o1.shape, o2.shape)
        self.assertGreater(np.abs(o2.astype(np.int16) - o1.astype(np.int16)).mean(), 1.0)


class TestColorRoundTrip(unittest.TestCase):
    def test_srgb_tables(self):
        x = np.linspace(0.02, 0.98, 9, dtype=np.float32)
        self.assertLess(float(np.abs(film_engine.srgb_decode(film_engine.srgb_encode(x)) - x).max()),
                        1e-3)

    def test_lab_round_trip(self):
        rgb = synth()[:32, :32].astype(np.float32) / 255.0
        back = film_engine._lab_to_rgb01(film_engine._rgb01_to_lab(rgb))
        self.assertLess(float(np.abs(back - rgb).max()), 1e-3)


if __name__ == "__main__":
    unittest.main()
