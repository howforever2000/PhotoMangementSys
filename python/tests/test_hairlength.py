"""发长判定（FEAT-209）的守卫单测。

钉死的语义：

  1. **只有头顶短发** → 不是长发（男性侧重档）。
  2. **两缕垂到下颌线以下、并到底边** → 长发（女性侧重档）。
  3. **帽子/兜帽形态**（底边有一片、但不在脸侧）→ 不是长发：
     实测婴儿兜帽会被解析成 hair 类，只靠「底边有没有头发」会误判。
  4. **头发像素太少**（小脸 / 光头）→ 返回 None，调用方按非长发处理。

几何口径：`bbox` 是检测框，`box` 是外扩 30% 后的 crop 在原图上的位置
（因此下颌线落在 crop 高度的 ~0.81 处）。
"""
import os
import sys
import unittest

import numpy as np

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import hairlength  # noqa: E402

BOX = (2, 0, 98, 96)        # crop 在原图上的位置，96×96
BBOX = (20, 18, 80, 78)     # 检测框（下巴在 y=78 → crop 的 0.8125）
SHAPE = (96, 96)


def _mask() -> np.ndarray:
    return np.zeros(SHAPE, dtype=bool)


class TestHairStats(unittest.TestCase):
    def test_short_hair_is_not_long(self):
        m = _mask()
        m[0:30, :] = True               # 只有头顶
        stats = hairlength.hair_stats(m, BBOX, BOX)
        self.assertIsNotNone(stats)
        self.assertFalse(stats.is_long)

    def test_neck_length_hair_is_long(self):
        m = _mask()
        m[0:30, :] = True               # 头顶
        m[70:96, 0:30] = True           # 左侧垂到脖颈
        m[70:96, 66:96] = True          # 右侧垂到脖颈
        stats = hairlength.hair_stats(m, BBOX, BOX)
        self.assertIsNotNone(stats)
        self.assertGreater(stats.below_ratio, 0.12)
        self.assertTrue(stats.is_long)

    def test_hat_bottom_band_without_sides_is_not_long(self):
        """兜帽形态：底边中间有一片「头发」，但脸侧没有 → 不是长发。"""
        m = _mask()
        m[0:40, :] = True               # 兜帽包裹头顶
        m[92:96, 38:58] = True          # 底边中间的一小片
        stats = hairlength.hair_stats(m, BBOX, BOX)
        self.assertIsNotNone(stats)
        self.assertGreaterEqual(stats.edge_ratio, hairlength.MIN_EDGE_HAIR)
        self.assertLess(stats.side_ratio, hairlength.MIN_SIDE_HAIR)
        self.assertFalse(stats.is_long)

    def test_too_few_hair_pixels_returns_none(self):
        m = _mask()
        m[0:2, 0:5] = True              # 10 px，远低于下限
        self.assertIsNone(hairlength.hair_stats(m, BBOX, BOX))
        self.assertFalse(hairlength.looks_long_hair(m, BBOX, BOX))

    def test_chin_uses_bbox_geometry(self):
        """下颌线由 bbox/box 几何算出，不写死 0.81。"""
        m = _mask()
        m[0:56, :] = True               # 只到 bbox 中部，仍在下颌线以上
        stats = hairlength.hair_stats(m, BBOX, BOX)
        self.assertAlmostEqual(stats.chin_y, 78 / 96, places=6)
        self.assertFalse(stats.is_long)


if __name__ == "__main__":
    unittest.main()
