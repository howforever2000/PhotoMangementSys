"""眼睛提亮（FEAT-210）与性别瘦脸折算的守卫单测。

钉死的语义：

  1. `amount=0` 逐字节不变（关掉滑条 = 不处理）。
  2. 只作用在眼周多边形内：矩形外逐字节不变。
  3. 提亮按彩度加权：眼白（低彩度）明显提亮，虹膜（高彩度）几乎不动。
  4. 眼睛过小 / 点数不足 → 原样返回。
  5. 瘦脸性别折算：男性 = 女性档 × 2/3（15% → 10%），未知性别不折算。
"""
import os
import sys
import unittest

import numpy as np
from PIL import Image, ImageDraw

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import eyebright, ops_local  # noqa: E402

WHITE_EYE = (30, 40, 74, 58)     # 左眼外接框 (x1, y1, x2, y2)
WHITE_EYE2 = (86, 40, 130, 58)   # 右眼
SCLERA = (236, 234, 230)         # 低彩度亮色（眼白）
IRIS = (120, 40, 20)             # 高彩度暖色（虹膜）
SKIN = (205, 175, 152)


def _points() -> np.ndarray:
    """造 68 点：两只眼的轮廓贴合上面的矩形，其余点随便给（不影响眼睛区域）。"""
    pts = np.zeros((68, 2), dtype=np.float64)
    left = [(32, 49), (40, 42), (58, 42), (70, 46), (58, 56), (40, 56)]
    right = [(88, 49), (96, 42), (114, 42), (126, 46), (114, 56), (96, 56)]
    for i, p in enumerate(left):
        pts[36 + i] = p
    for i, p in enumerate(right):
        pts[42 + i] = p
    return pts


def _image() -> np.ndarray:
    """眼白铺满眼眶、虹膜只占中间一小块（贴近真实：眼白面积像两条月牙）。"""
    arr = np.zeros((130, 160, 3), dtype=np.uint8)
    arr[:, :] = SKIN
    mask = Image.new("L", (160, 130), 0)
    draw = ImageDraw.Draw(mask)
    for sl in (slice(36, 42), slice(42, 48)):
        poly = _points()[sl]
        draw.polygon([(float(x), float(y)) for x, y in poly], fill=255)
    open_mask = np.asarray(mask) > 0
    arr[open_mask] = SCLERA
    # 虹膜：每只眼中心 r=7 的圆
    for cx in (50, 106):
        yy, xx = np.mgrid[0:130, 0:160]
        iris = ((xx - cx) ** 2 + (yy - 49) ** 2 <= 49) & open_mask
        arr[iris] = IRIS
    return arr


def _mean_l(arr: np.ndarray, mask: np.ndarray) -> float:
    from ai_studio import colortransfer

    lab = colortransfer.rgb_to_lab(arr)
    return float(lab[..., 0][mask].mean())


def _iris_mask() -> np.ndarray:
    yy, xx = np.mgrid[0:130, 0:160]
    return ((xx - 50) ** 2 + (yy - 49) ** 2 <= 49) | ((xx - 106) ** 2 + (yy - 49) ** 2 <= 49)


def _sclera_mask() -> np.ndarray:
    """眼白 = 眼眶内、虹膜外。"""
    mask = Image.new("L", (160, 130), 0)
    draw = ImageDraw.Draw(mask)
    for sl in (slice(36, 42), slice(42, 48)):
        poly = _points()[sl]
        draw.polygon([(float(x), float(y)) for x, y in poly], fill=255)
    return (np.asarray(mask) > 0) & ~_iris_mask()


class TestEyebright(unittest.TestCase):
    def test_zero_amount_is_identity(self):
        img = Image.fromarray(_image())
        out = eyebright.brighten_eyes(img, _points(), 0.0)
        self.assertTrue(np.array_equal(np.asarray(out), np.asarray(img)))

    def test_outside_eye_rect_untouched(self):
        img = Image.fromarray(_image())
        out = np.asarray(eyebright.brighten_eyes(img, _points(), 0.5))
        rect = eyebright.eye_rect(_points())
        x1, y1, x2, y2 = rect
        mask = np.ones(out.shape[:2], dtype=bool)
        mask[y1:y2, x1:x2] = False
        diff = np.abs(out.astype(np.int16) - _image().astype(np.int16))
        self.assertEqual(int(diff[mask].max()), 0)

    def test_sclera_brightens_more_than_iris(self):
        """眼白（低彩度）明显提亮；虹膜只按保底比例提，不会发灰。"""
        img = Image.fromarray(_image())
        out = np.asarray(eyebright.brighten_eyes(img, _points(), 0.5))
        before = _image()
        d_sclera = _mean_l(out, _sclera_mask()) - _mean_l(before, _sclera_mask())
        d_iris = _mean_l(out, _iris_mask()) - _mean_l(before, _iris_mask())
        self.assertGreater(d_sclera, 1.5)
        self.assertGreater(d_iris, 0.0)          # 保底提亮：保留一点「眼神」
        self.assertLess(d_iris, d_sclera * 0.6)  # 但明显弱于眼白

    def test_more_amount_more_lift(self):
        img = Image.fromarray(_image())
        mask = _sclera_mask()
        low = _mean_l(np.asarray(eyebright.brighten_eyes(img, _points(), 0.1)), mask)
        high = _mean_l(np.asarray(eyebright.brighten_eyes(img, _points(), 1.0)), mask)
        base = _mean_l(_image(), mask)
        self.assertGreater(high, low)
        self.assertGreater(low, base)

    def test_degenerate_points_return_original(self):
        img = Image.fromarray(_image())
        out = eyebright.brighten_eyes(img, np.zeros((10, 2)), 0.5)
        self.assertTrue(np.array_equal(np.asarray(out), np.asarray(img)))

    def test_rect_none_for_tiny_eyes(self):
        pts = _points()
        pts[36:42] = [[40, 48], [40, 48], [40, 48], [41, 48], [41, 48], [41, 48]]
        pts[42:48] = [[80, 48], [80, 48], [80, 48], [81, 48], [81, 48], [81, 48]]
        self.assertIsNone(eyebright.eye_rect(pts))


class TestSlimGender(unittest.TestCase):
    def test_male_is_two_thirds(self):
        self.assertAlmostEqual(ops_local.slim_for_gender(0.15, False), 0.10, places=6)
        self.assertAlmostEqual(ops_local.slim_for_gender(0.30, False), 0.20, places=6)

    def test_female_uses_verbatim(self):
        self.assertAlmostEqual(ops_local.slim_for_gender(0.15, True), 0.15, places=6)

    def test_unknown_does_not_adjust(self):
        self.assertAlmostEqual(ops_local.slim_for_gender(0.15, None), 0.15, places=6)


if __name__ == "__main__":
    unittest.main()
