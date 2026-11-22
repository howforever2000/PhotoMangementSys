"""跨算子上下文通道（FEAT-208/209）的守卫单测。

钉死的语义：

  1. 默认上下文 `EMPTY` 是**只读**的：直接调用算子（不带 ctx）不能把状态写到
     模块级单例上，否则两次无关调用会串味。
  2. 形变（`drop_face_analysis`）只作废掩码，不置 `geometry_dirty` ——
     局部液化后掩码仍能标回原图取锚点（实测偏差 ≤0.52°），这条决定省掉 375ms/张。
  3. 整幅重绘/缩放（`mark_geometry_changed`）同时作废掩码并置脏位。
  4. 锚点复用必须同时满足「几何一致 + 同尺寸」，否则回退到原图重新分析。
"""
import os
import sys
import unittest
from dataclasses import replace

from PIL import Image

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import ops_local  # noqa: E402
from ai_studio.renderopts import EMPTY, RunContext  # noqa: E402


def _img(size=(120, 90)) -> Image.Image:
    return Image.new("RGB", size, (200, 170, 150))


class TestScratchChannel(unittest.TestCase):
    def test_empty_context_is_not_mutated(self):
        EMPTY.drop_face_analysis()
        EMPTY.mark_geometry_changed()
        self.assertEqual(dict(EMPTY.scratch), {})
        self.assertFalse(EMPTY.geometry_dirty)

    def test_reshape_like_drop_keeps_anchor_aligned(self):
        ctx = replace(EMPTY, source=_img(), scratch={"face_analysis": ["stale"]})
        ctx.drop_face_analysis()
        self.assertNotIn("face_analysis", ctx.scratch)
        self.assertFalse(ctx.geometry_dirty)

    def test_redraw_marks_geometry_dirty(self):
        ctx = replace(EMPTY, source=_img(), scratch={"face_analysis": ["stale"]})
        ctx.mark_geometry_changed()
        self.assertNotIn("face_analysis", ctx.scratch)
        self.assertTrue(ctx.geometry_dirty)

    def test_plain_context_defaults_to_aligned(self):
        self.assertFalse(RunContext().geometry_dirty)


class TestAnchorReuse(unittest.TestCase):
    def test_same_size_aligned_reuses(self):
        img, src = _img(), _img()
        self.assertTrue(ops_local._can_reuse_anchor_masks(img, src, True))

    def test_unaligned_does_not_reuse(self):
        img, src = _img(), _img()
        self.assertFalse(ops_local._can_reuse_anchor_masks(img, src, False))

    def test_size_mismatch_does_not_reuse(self):
        img, src = _img((120, 90)), _img((240, 180))
        self.assertFalse(ops_local._can_reuse_anchor_masks(img, src, True))

    def test_no_source_does_not_reuse(self):
        self.assertFalse(ops_local._can_reuse_anchor_masks(_img(), None, True))


if __name__ == "__main__":
    unittest.main()
