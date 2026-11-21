"""自然饱和度单测（FEAT-206/207 口径）。

钉死的语义：
  1. **独立算子 `local_vibrance`**：`vibrance_adjust` 复用调色管线第 6 步的
     `_vibrance_lch` 数学（淡色多动、艳色少动、headroom 回收）；
  2. **0 = 逐字节恒等**：中性值下原图原样返回，配方默认值不改出图；
  3. **双向**：正提升淡色、负收敛；权重语义成立（低饱和像素比高饱和动得多）；
  4. **内嵌链路已拆**：`analyze` / `grade` 不再收 `vibrance_extra`（FEAT-186 的
     配方级微调随内嵌滑条移除），catalog 里 auto_grade 不再声明 vibrance 参数。
"""
import inspect
import json
import os
import sys
import unittest
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from ai_studio import autograde, ops, ops_local  # noqa: E402


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


class TestStandaloneVibrance(unittest.TestCase):
    def test_zero_is_byte_identical(self):
        """中性 0 必须逐字节恒等（配方默认值不改出图）。"""
        img = _photo()
        out = autograde.vibrance_adjust(img, 0.0)
        self.assertTrue(np.array_equal(out, img))

        tiny = autograde.vibrance_adjust(img, 1e-4)  # 低于阈值也走恒等
        self.assertTrue(np.array_equal(tiny, img))

    def test_positive_changes_pixels(self):
        img = _photo()
        out = autograde.vibrance_adjust(img, 0.25)
        self.assertFalse(np.array_equal(out, img), "正向必须有像素变化")

    def test_negative_changes_pixels(self):
        img = _photo()
        out = autograde.vibrance_adjust(img, -0.25)
        self.assertFalse(np.array_equal(out, img), "负向必须有像素变化")

    def test_low_saturation_moves_more(self):
        """自然饱和度语义：淡色（低通道差）应比艳色动得更狠。"""
        img = _photo()
        base = img.astype(np.int16)
        up = autograde.vibrance_adjust(img, 0.25).astype(np.int16)
        spread = base.max(axis=-1) - base.min(axis=-1)
        low = np.abs(up - base)[spread < 30].mean()
        high = np.abs(up - base)[spread > 120].mean()
        self.assertGreater(low, 0.0)
        self.assertGreater(low, high * 0.5, "淡色像素的变化量应显著大于艳色像素")

    def test_op_registered_and_identity_via_dispatch(self):
        self.assertIn("local_vibrance", ops_local.OPS_LOCAL)
        self.assertIn("local_vibrance", ops.LOCAL_OPS)
        self.assertTrue(ops.local_op_ready("local_vibrance"))
        from PIL import Image

        img = Image.fromarray(_photo())
        out0 = ops.resolve("local_vibrance")(img, {"vibrance": 0.0}, None)
        self.assertEqual(list(out0.getdata()), list(img.getdata()))


class TestEmbeddedVibranceRemoved(unittest.TestCase):
    def test_analyze_grade_no_vibrance_extra(self):
        """FEAT-186 的 vibrance_extra 链路必须拆干净（独立算子替代）。"""
        self.assertNotIn("vibrance_extra", inspect.signature(autograde.analyze).parameters)
        self.assertNotIn("vibrance_extra", inspect.signature(autograde.grade).parameters)
        self.assertNotIn("vibrance_extra", inspect.signature(ops_local.grade_image).parameters)

    def test_auto_grade_schema_has_no_vibrance(self):
        cat = json.loads((ROOT / "python" / "ai_studio" / "op_catalog.json").read_text("utf-8"))
        ag = next(o for o in cat["ops"] if o["id"] == "auto_grade")
        self.assertNotIn("vibrance", ag["params"], "调色算子不得再内嵌 vibrance 滑条")
        lv = next(o for o in cat["ops"] if o["id"] == "local_vibrance")
        self.assertIn("vibrance", lv["params"], "独立自然饱和度算子必须声明 vibrance")


if __name__ == "__main__":
    unittest.main()
