"""照片级并行执行器 + CPU 线程拓扑口径的守卫测试。

钉死的语义：
  1. `map_photos` **保序**（结果下标与 paths 对齐）—— 落盘与 `outputs[i]` 回填都靠它；
  2. 单张抛异常只影响自己，且原文保留（降级纪律：单张失败不拖垮整批）；
  3. 取消后未开工的不再处理，且不报失败；
  4. `cputopo.threads()` = min(P 核数, 逻辑核//2)，**不是** 逻辑核//2（大小核机器上会
     越过 P 核溢到 E 核，越加越慢，见 BUG-2026-1027-003）。
"""
import os
import sys
import threading
import time
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import cputopo, parallel  # noqa: E402


class TestMapPhotos(unittest.TestCase):
    def test_preserves_order_under_parallel(self):
        """故意让靠前的项慢、靠后的项快 —— 完成顺序乱序，结果顺序仍须与输入一致。"""
        paths = [f"p{i}" for i in range(12)]

        def fn(i, p):
            time.sleep(0.02 * (12 - i) / 12)  # 越靠后越快
            return p.upper()

        out = parallel.map_photos(paths, fn, workers=4)
        self.assertEqual(out, [p.upper() for p in paths])

    def test_single_item_failure_is_isolated(self):
        paths = ["a", "b", "c", "d"]

        def fn(i, p):
            if p == "b":
                raise ValueError("boom")
            return p

        out = parallel.map_photos(paths, fn, workers=3)
        self.assertEqual(out[0], "a")
        self.assertTrue(parallel.is_failed(out[1]))
        self.assertIn("boom", out[1].reason())
        self.assertEqual(out[2], "c")
        self.assertEqual(out[3], "d")

    def test_cancel_stops_remaining(self):
        """取消后：未开工的不执行；已完成的照常返回；未处理的不算失败。"""
        cancel = threading.Event()
        ran = []

        def fn(i, p):
            ran.append(p)
            if len(ran) >= 2:
                cancel.set()  # 跑到第 2 张就取消
            return p

        paths = [f"p{i}" for i in range(8)]
        out = parallel.map_photos(paths, fn, cancel=cancel, workers=1)
        self.assertLess(len(ran), len(paths), "取消后不应把 8 张全跑完")
        self.assertTrue(
            any(parallel.is_cancelled(o) for o in out),
            "未处理的项应标记为已取消",
        )

    def test_progress_callback_counts_every_item(self):
        seen = []
        paths = [f"p{i}" for i in range(5)]
        parallel.map_photos(
            paths, lambda i, p: p, workers=2,
            on_done=lambda done, total: seen.append((done, total)),
        )
        self.assertEqual(len(seen), 5, "每张都要回调一次（成功与失败都算）")
        self.assertEqual([d for d, _ in seen], [1, 2, 3, 4, 5], "完成计数必须递增")
        self.assertTrue(all(t == 5 for _, t in seen))

    def test_empty_input(self):
        self.assertEqual(parallel.map_photos([], lambda i, p: p), [])

    def test_worker_count_capped_by_items(self):
        """1 张图不该起 6 个线程（也不该出错）。"""
        out = parallel.map_photos(["只一张"], lambda i, p: p, workers=6)
        self.assertEqual(out, ["只一张"])


class TestWorkersFor(unittest.TestCase):
    def test_onnx_heavy_uses_fewer_threads(self):
        """ONNX 链路要折半：每个会话自己还有 intra_op 线程，两层叠满会超订。"""
        self.assertLess(parallel.workers_for(onnx_heavy=True), parallel.workers_for(False))

    def test_both_at_least_two(self):
        self.assertGreaterEqual(parallel.workers_for(onnx_heavy=True), 2)
        self.assertGreaterEqual(parallel.workers_for(onnx_heavy=False), 1)


class TestCpuTopology(unittest.TestCase):
    def test_threads_never_exceed_p_cores(self):
        """线程数不得超过 P 核数 —— 越过就溢到 E 核，实测越加越慢。"""
        self.assertLessEqual(cputopo.threads(), cputopo.p_cores())

    def test_threads_at_most_half_logical(self):
        """同时不超过逻辑核一半：给宿主 UI / 缩略图 / 另一个 Python 服务留余量。"""
        self.assertLessEqual(cputopo.threads(), max(1, cputopo.logical_cores() // 2))

    def test_threads_within_bounds(self):
        self.assertGreaterEqual(cputopo.threads(), cputopo.THREADS_MIN)
        self.assertLessEqual(cputopo.threads(), cputopo.THREADS_MAX)

    def test_small_model_threads_less_or_equal(self):
        self.assertLessEqual(cputopo.small_model_threads(), cputopo.threads())
        self.assertGreaterEqual(cputopo.small_model_threads(), 2)

    def test_topology_shape(self):
        physical, p, e = cputopo.cpu_topology()
        self.assertGreaterEqual(physical, 1)
        self.assertGreaterEqual(p, 1)
        self.assertGreaterEqual(e, 0)
        self.assertEqual(p + e, physical, "P 核 + E 核 必须等于物理核总数")

    def test_env_override_respected(self):
        """A/B 对照与排障用：AI_STUDIO_THREADS 应能覆盖（且被夹到合法区间）。"""
        old = os.environ.get("AI_STUDIO_THREADS")
        try:
            os.environ["AI_STUDIO_THREADS"] = "4"
            self.assertEqual(cputopo.threads(), 4)
            os.environ["AI_STUDIO_THREADS"] = "999"
            self.assertEqual(cputopo.threads(), cputopo.THREADS_MAX)
            os.environ["AI_STUDIO_THREADS"] = "0"
            self.assertGreaterEqual(cputopo.threads(), cputopo.THREADS_MIN)
        finally:
            if old is None:
                os.environ.pop("AI_STUDIO_THREADS", None)
            else:
                os.environ["AI_STUDIO_THREADS"] = old


if __name__ == "__main__":
    unittest.main()
