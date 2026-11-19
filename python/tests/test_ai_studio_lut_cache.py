"""ai_studio LUT 稠密表缓存的驻留口径。

钉死的语义（对应 .dsh/knowledge/local-enhance.md §19/§20/§22）：
  1. 运行时生成的 LUT（autograde 逐图新建，只套一次）不得进稠密表缓存 ——
     否则每张图泄漏一张 11MB 表；
  2. 路径 LUT（内置 .cube，可复用）仍然进缓存并可按同一对象命中。
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

    def test_runtime_lut_is_not_cached(self):
        for _ in range(3):
            lut = np.random.default_rng(1).random((33, 33, 33, 3), dtype=np.float32)
            flat, d = lutcube._dense_flat(lut, 33, cache=False)
            self.assertEqual(flat.shape, (d ** 3, 3))
        self.assertEqual(lutcube._DENSE_CACHE, {}, "运行时 LUT 每次只套一次，缓存即泄漏 11MB/张")

    def test_path_lut_is_cached(self):
        lut = np.random.default_rng(2).random((33, 33, 33, 3), dtype=np.float32)
        lutcube._dense_flat(lut, 33, cache=True)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1, "可复用的路径 LUT 应缓存稠密表")
        lutcube._dense_flat(lut, 33, cache=True)
        self.assertEqual(len(lutcube._DENSE_CACHE), 1, "同一 LUT 命中缓存，不重复建表")


if __name__ == "__main__":
    unittest.main()
