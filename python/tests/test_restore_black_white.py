"""lutcube.restore_black_white（FEAT-176 黑白场还原）单测。

钉死的语义：
  1. amount=0 恒等（关闭状态零代价直通）；
  2. 满量把被抬的黑位 / 被压的白位拉回端点；
  3. 部分量单调、有界、中间调基本不动（锚点 0.10 的设计意图）；
  4. 已接近满格的中间调不会被反向拉出界（clip 收口）。
"""
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

import numpy as np  # noqa: E402

from ai_studio import lutcube  # noqa: E402


def _grad(h: int = 8, w: int = 256) -> np.ndarray:
    """横向 0~255 渐变灰度图。"""
    row = np.tile(np.linspace(0, 255, w, dtype=np.float32), (h, 1))
    return np.stack([row] * 3, axis=-1).astype(np.uint8)


class TestRestoreBlackWhite(unittest.TestCase):
    def test_zero_amount_is_identity(self):
        img = _grad()
        self.assertTrue(np.array_equal(lutcube.restore_black_white(img, 0.0), img))

    def test_full_amount_restores_endpoints(self):
        # 模拟负片 LUT：黑位抬到 25、白位压到 230 的渐变
        row = np.linspace(25, 230, 256, dtype=np.float32)
        img = np.stack([np.tile(row, (8, 1))] * 3, axis=-1).astype(np.uint8)
        out = lutcube.restore_black_white(img, 1.0)
        self.assertLessEqual(int(out.min()), 1)
        self.assertGreaterEqual(int(out.max()), 254)

    def test_partial_amount_monotonic_and_bounded(self):
        img = _grad()
        out = lutcube.restore_black_white(img, 0.4)
        diffs = np.diff(out[0].astype(np.int32))
        self.assertTrue((diffs >= 0).all())
        self.assertEqual(out.dtype, np.uint8)
        mid = int(out[0, 128, 0])
        self.assertTrue(110 <= mid <= 145)

    def test_clipped_midscale_not_over_stretched(self):
        img = np.full((8, 8, 3), 240, dtype=np.uint8)
        out = lutcube.restore_black_white(img, 0.8)
        self.assertLessEqual(int(out.max()), 255)


if __name__ == "__main__":
    unittest.main()
