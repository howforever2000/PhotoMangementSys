"""`workflows/base.unique_path` 的并发安全守卫。

为什么钉这条：照片级并行后，`unique_path` 的「查存在 → 写盘」两步之间是真竞态。
两个线程同批处理 `IMG_1.jpg` 与 `IMG_1.png`（归一成同一个 stem）时会拿到
同一个目标路径、互相覆盖，且**不会报错** —— 症状是「导出的图少了一张」。
现在改成 `O_CREAT|O_EXCL` 原子占位。
"""
import os
import sys
import tempfile
import threading
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio.workflows.base import unique_path  # noqa: E402


class TestUniquePath(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="pms-unique-")

    def tearDown(self):
        for root, _dirs, files in os.walk(self.dir, topdown=False):
            for f in files:
                os.remove(os.path.join(root, f))
        os.rmdir(self.dir)

    def test_first_call_uses_plain_name(self):
        p = unique_path(self.dir, "a", ".jpg")
        self.assertEqual(os.path.basename(p), "a.jpg")
        self.assertTrue(os.path.exists(p), "应当原子占位，文件此刻已存在")

    def test_existing_file_gets_suffix(self):
        first = unique_path(self.dir, "a", ".jpg")
        second = unique_path(self.dir, "a", ".jpg")
        self.assertNotEqual(first, second)
        self.assertEqual(os.path.basename(second), "a_1.jpg")

    def test_concurrent_calls_never_collide(self):
        """并发 16 次取同名路径，必须拿到 16 个互不相同的路径。

        Barrier 的参与方数量必须**与线程数一致**：写成 `Barrier(16)` 却起 40 个线程
        时，后 24 个到达的线程会永久阻塞（第一批已经跨过栅栏了）。
        """
        n = 16
        results = []
        lock = threading.Lock()
        barrier = threading.Barrier(n)

        def worker():
            barrier.wait()
            p = unique_path(self.dir, "same", ".jpg")
            with lock:
                results.append(p)

        threads = [threading.Thread(target=worker) for _ in range(n)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=30)

        self.assertEqual(len(results), n)
        self.assertEqual(len(set(results)), n, "并发下不得出现重复路径（会互相覆盖）")
        for p in results:
            self.assertTrue(os.path.exists(p), "每个路径都应被占位")

    def test_different_extensions_get_distinct_names(self):
        a = unique_path(self.dir, "photo", ".jpg")
        b = unique_path(self.dir, "photo", ".png")
        self.assertNotEqual(a, b, "不同扩展名不能互相覆盖")


if __name__ == "__main__":
    unittest.main()
