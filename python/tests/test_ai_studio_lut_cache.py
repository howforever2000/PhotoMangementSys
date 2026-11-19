"""ai_studio LUT 稠密表缓存的驻留口径。

钉死的语义（对应 `.dsh/knowledge/local-enhance.md` §19/§20/§22）：

  1. **路径 LUT**（内置 .cube，可复用）进缓存，同一 LUT 命中不重复建表；
  2. **运行时 LUT**（autograde 逐图新建）也进缓存，但键是**内容摘要**而不是对象 id。

第 2 条是 2026-11-19 的性能修订，与旧语义相反，原因（实测）：
  旧实现把运行时 LUT 排除在缓存外，本意是「只套一次，缓存等于泄漏一张 11MB 表」。
  但实际链路里 autograde 每张图都新建 LUT，而**滑杆级预览**会对同一组参数
  反复重建：建 97³ 稠密表实测 **330~578ms**，而真正的查表只要 **3.8ms**（1280px）。
  按内容摘要做键后，同参数命中原表；LRU 仍压到 8 条（约 22MB），
  条目数**不随照片数增长**（旧担心的泄漏由 LRU 兜住，不需要靠「不缓存」实现）。
"""
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

import numpy as np  # noqa: E402

from ai_studio import lutcube  # noqa: E402


class TestDenseCache(unittest.TestCase):
    def setUp(self):
        lutcube._DENSE_CACHE.clear()
        lutcube._DENSE_U8_CACHE.clear()

    def test_path_lut_is_cached(self):
        lut = np.random.default_rng(2).random((33, 33, 33, 3), dtype=np.float32)
        key = ("p", "some/path.cube", 33)
        lutcube._dense_flat(lut, 33, key)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1, "可复用的路径 LUT 应缓存稠密表")
        lutcube._dense_flat(lut, 33, key)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1, "同一 LUT 命中缓存，不重复建表")

    def test_runtime_lut_cached_by_content_digest(self):
        """同一组参数的新建 LUT（内容相同、对象不同）应命中同一张稠密表。"""
        rng = np.random.default_rng(1)
        lut_a = rng.random((33, 33, 33, 3), dtype=np.float32)
        lut_b = lut_a.copy()  # 新对象、同内容 —— 模拟同一参数再算一次
        self.assertIsNot(lut_a, lut_b)

        key_a = ("h", lutcube._lut_digest(lut_a), 33)
        key_b = ("h", lutcube._lut_digest(lut_b), 33)
        self.assertEqual(key_a, key_b, "内容相同必须得到同一个摘要")

        lutcube._dense_flat(lut_a, 33, key_a)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1)
        lutcube._dense_flat(lut_b, 33, key_b)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1, "同参数应命中，不重复建表")

    def test_different_content_does_not_share_table(self):
        """不同参数必须得到不同摘要 —— 否则会把别的照片的调色参数套上来（静默错色）。"""
        rng = np.random.default_rng(3)
        a = rng.random((33, 33, 33, 3), dtype=np.float32)
        b = rng.random((33, 33, 33, 3), dtype=np.float32)
        self.assertNotEqual(lutcube._lut_digest(a), lutcube._lut_digest(b))

    def test_runtime_cache_is_bounded(self):
        """运行时 LUT 不能无界增长：LRU 上限兜住，条目数不随照片数增长。"""
        rng = np.random.default_rng(4)
        for i in range(lutcube._DENSE_U8_CACHE_MAX + 6):
            lut = rng.random((33, 33, 33, 3), dtype=np.float32)
            key = ("h", lutcube._lut_digest(lut), 33)
            lutcube._dense_u8(lut, 33, 1.0, key)
        self.assertLessEqual(
            len(lutcube._DENSE_U8_CACHE),
            lutcube._DENSE_U8_CACHE_MAX,
            "量化表缓存必须被 LRU 压在上限内",
        )


if __name__ == "__main__":
    unittest.main()
