"""autograde 自然饱和度微调（FEAT-186）单测。

钉死的语义（这四条一旦破了，就是「改了但不生效」或「默认变了」）：
  1. **默认 0 逐字节等价**：`vibrance_extra=0` 与不传参的 `GradeStats` 完全相同，
     且成片像素完全一致 —— 四条配方不改一个字，出图不许有肉眼可见变化；
  2. **加法叠加**：`vibrance = (风格派生值 + extra) * strength`；
  3. **力度 0 = 什么都不做**：`strength=0` 时 extra 也被乘成 0；
  4. **上限仍由 `_clamp_output` 兜底**（0.45），extra 拉满也不会越界。
"""
import os
import sys
import unittest

import numpy as np

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import autograde  # noqa: E402


def _photo(seed: int = 7) -> np.ndarray:
    """一张有层次、有颜色的合成图：渐变 + 彩块，避免纯色图走「不拉伸」分支。"""
    rng = np.random.default_rng(seed)
    h, w = 96, 128
    yy, xx = np.mgrid[0:h, 0:w]
    base = (yy / float(h) * 0.8 + 0.1) * 255.0
    img = np.stack([base, base * 0.92, base * 0.85], axis=-1)
    img[h // 3 : 2 * h // 3, w // 4 : 3 * w // 4] = [210.0, 120.0, 90.0]
    img += rng.normal(0.0, 2.0, img.shape)
    return np.clip(img, 0, 255).astype(np.uint8)


class TestVibranceExtra(unittest.TestCase):
    def test_default_zero_is_identical(self):
        """不传 extra 与传 0 必须完全等价（默认行为不许漂移）。"""
        img = _photo()
        a = autograde.analyze(img, style="natural", strength=0.85)
        b = autograde.analyze(img, style="natural", strength=0.85, vibrance_extra=0.0)
        self.assertEqual(a, b)

        out_a, _, _ = autograde.grade(img, style="natural", strength=0.85)
        out_b, _, _ = autograde.grade(
            img, style="natural", strength=0.85, vibrance_extra=0.0
        )
        self.assertTrue(np.array_equal(out_a, out_b), "默认 0 时成片必须逐字节相同")

    def test_additive_over_style_value(self):
        """extra 是加法叠加在风格派生值上，再乘力度。"""
        img = _photo()
        s = 0.8
        base = autograde.analyze(img, style="natural", strength=s)
        more = autograde.analyze(img, style="natural", strength=s, vibrance_extra=0.2)
        # 上限内时差值应正好等于 extra * strength
        if (base.vibrance + 0.2 * s) <= 0.45:
            self.assertAlmostEqual(more.vibrance - base.vibrance, 0.2 * s, places=5)
        self.assertGreaterEqual(more.vibrance, base.vibrance)

    def test_strength_zero_means_noop(self):
        """力度 0 时额外量也被乘成 0 —— 与「滑杆拉到 0 就是不做」一致。"""
        img = _photo()
        a = autograde.analyze(img, style="natural", strength=0.0, vibrance_extra=0.25)
        self.assertAlmostEqual(a.vibrance, 0.0, places=6)
        self.assertEqual(a.saturation, 1.0)

    def test_clamped_by_output_bounds(self):
        """extra 拉满也过不了 _clamp_output 的 0.45 上限。"""
        img = _photo()
        stats = autograde.analyze(img, style="vivid", strength=1.0, vibrance_extra=0.25)
        self.assertLessEqual(stats.vibrance, 0.45 + 1e-9)

    def test_graded_output_changes_when_extra_given(self):
        """给了 extra 就得真的有像素变化，不能是死控件。"""
        img = _photo()
        out_a, _, _ = autograde.grade(img, style="natural", strength=0.85)
        out_b, _, _ = autograde.grade(
            img, style="natural", strength=0.85, vibrance_extra=0.25
        )
        self.assertFalse(np.array_equal(out_a, out_b), "加了自然饱和度但图没变")


if __name__ == "__main__":
    unittest.main()
