"""ai_studio GPU 串行锁的单测。

钉死的语义（对应 .dsh/knowledge/gpu-dml-constraints.md「DML 串行锁是设备级硬约束」）：
  1. 绑到非 CPU provider 的会话，`gpu_lock.run` 必须串行 ——
     并发 run 会踩同一 D3D12 设备上下文，表现是随机 ACCESS_VIOLATION / 进程消失；
  2. 纯 CPU 会话不过锁（不能把免费的并发布成串行）；
  3. provider 事实读不到时保守加锁（错锁只慢一档，漏锁是段错误）。
"""
import os
import sys
import threading
import time
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

import numpy as np  # noqa: E402

from ai_studio import gpu_lock  # noqa: E402


class _FakeSess:
    def __init__(self, providers, delay=0.0):
        self._providers = providers
        self._delay = delay
        self._state = threading.Lock()
        self.active = 0
        self.max_active = 0

    def get_providers(self):
        return self._providers

    def run(self, _outs, feed):
        with self._state:
            self.active += 1
            self.max_active = max(self.max_active, self.active)
        try:
            time.sleep(self._delay)
            return (np.zeros((1, 1), dtype=np.float32),)
        finally:
            with self._state:
                self.active -= 1


def _drive(sess, n=4):
    def body():
        gpu_lock.run(sess, {"x": np.zeros((1,), dtype=np.float32)})

    threads = [threading.Thread(target=body) for _ in range(n)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()


class TestGpuLock(unittest.TestCase):
    def test_gpu_session_is_serialized(self):
        sess = _FakeSess(["DmlExecutionProvider", "CPUExecutionProvider"], delay=0.02)
        _drive(sess)
        self.assertEqual(sess.max_active, 1, "DML 会话并发 run 会踩同一 D3D12 设备")

    def test_cpu_session_is_not_serialized(self):
        sess = _FakeSess(["CPUExecutionProvider"], delay=0.05)
        _drive(sess)
        self.assertGreater(sess.max_active, 1, "纯 CPU 会话不应白持锁")

    def test_unknown_providers_are_conservative(self):
        class _Broken:
            def get_providers(self):
                raise RuntimeError("boom")

            def run(self, _outs, feed):
                return (np.zeros((1, 1), dtype=np.float32),)

        self.assertTrue(gpu_lock.is_gpu(_Broken()), "拿不到 provider 事实时应保守加锁")


if __name__ == "__main__":
    unittest.main()
