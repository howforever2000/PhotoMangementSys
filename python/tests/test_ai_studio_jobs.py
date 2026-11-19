"""ai_studio 作业注册表（jobs）的清理语义。

钉死的语义：
  1. 终态作业超过 TTL 后必须被清掉（服务常驻，不清理就单调增长）；
  2. 运行中的作业永远不因「总量上限」被清掉（否则前端轮询直接 404）；
  3. 总量压到 `_JOBS_MAX` 以内。
"""
import os
import sys
import time
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import jobs  # noqa: E402


class TestJobsSweep(unittest.TestCase):
    def setUp(self):
        jobs._jobs.clear()

    def tearDown(self):
        jobs._jobs.clear()

    def test_expired_terminal_is_removed(self):
        now = time.time()
        jobs._jobs["old"] = {"status": "done", "_finished_at": now - jobs._JOB_TTL_SEC - 10}
        jobs._jobs["new"] = {"status": "done", "_finished_at": now}
        jobs._jobs["run"] = {"status": "running"}
        jobs._sweep_locked(now)
        self.assertEqual(sorted(jobs._jobs), ["new", "run"])

    def test_cap_never_drops_running(self):
        now = time.time()
        for i in range(jobs._JOBS_MAX + 5):
            jobs._jobs[f"j{i}"] = {"status": "done", "_finished_at": now - i}
        jobs._jobs["running"] = {"status": "running"}
        jobs._sweep_locked(now)
        self.assertLessEqual(len(jobs._jobs), jobs._JOBS_MAX)
        self.assertIn("running", jobs._jobs)


if __name__ == "__main__":
    unittest.main()
