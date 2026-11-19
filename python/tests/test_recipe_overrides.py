"""recipecatalog.apply_overrides 单测：样片微调参数覆盖的边界。

钉死的语义：
  1. 白名单外的参数/算子被忽略（精修参数想传也传不进）；
  2. 越界值被 norm_params 夹取，不炸不静默出界；
  3. 覆盖后仍走 validate_plan，依赖顺序约束不失效。
"""
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import recipecatalog  # noqa: E402


class TestApplyOverrides(unittest.TestCase):
    def test_none_keeps_plan(self):
        p0 = recipecatalog.apply_overrides("portrait", None)
        p1 = recipecatalog.plan_of("portrait")
        self.assertEqual(p0, p1)

    def test_empty_keeps_plan(self):
        p0 = recipecatalog.apply_overrides("portrait", {})
        p1 = recipecatalog.plan_of("portrait")
        self.assertEqual(p0, p1)

    def test_override_whitelisted(self):
        p = recipecatalog.apply_overrides(
            "portrait", {"face_reshape": {"slim": 0.5, "eyes": 0.3}}
        )
        step = next(s for s in p["steps"] if s["op"] == "face_reshape")
        self.assertEqual(step["params"]["slim"], 0.5)
        self.assertEqual(step["params"]["eyes"], 0.3)

    def test_partial_merge(self):
        """只覆盖一个键，同步其他键保持配方值。"""
        p = recipecatalog.apply_overrides("portrait", {"skin_smooth": {"strength": 0.6}})
        step = next(s for s in p["steps"] if s["op"] == "skin_smooth")
        self.assertEqual(step["params"]["strength"], 0.6)
        self.assertEqual(step["params"]["texture"], 0.55)

    def test_out_of_whitelist_ignored(self):
        p = recipecatalog.apply_overrides(
            "portrait", {"face_reshape": {"evil": 1}, "nope": {"x": 1}}
        )
        step = next(s for s in p["steps"] if s["op"] == "face_reshape")
        self.assertNotIn("evil", step["params"])

    def test_out_of_range_clamped(self):
        p = recipecatalog.apply_overrides(
            "portrait", {"face_reshape": {"slim": 99, "eyes": -5}}
        )
        step = next(s for s in p["steps"] if s["op"] == "face_reshape")
        self.assertLessEqual(step["params"]["slim"], 0.8)
        self.assertGreaterEqual(step["params"]["eyes"], 0.0)

    def test_garbage_input_no_crash(self):
        p = recipecatalog.apply_overrides("portrait", "garbage")
        self.assertEqual(len(p["steps"]), 4)

    def test_vibrance_whitelisted_and_clamped(self):
        """FEAT-186：自然饱和度是可覆盖量，且越界值按 catalog 夹到 0~0.25。"""
        p = recipecatalog.apply_overrides("natural", {"auto_grade": {"vibrance": 0.2}})
        step = next(s for s in p["steps"] if s["op"] == "auto_grade")
        self.assertAlmostEqual(step["params"]["vibrance"], 0.2)

        p2 = recipecatalog.apply_overrides("natural", {"auto_grade": {"vibrance": 9.0}})
        step2 = next(s for s in p2["steps"] if s["op"] == "auto_grade")
        self.assertLessEqual(step2["params"]["vibrance"], 0.25)

    def test_vibrance_default_zero_keeps_recipe(self):
        """没传 vibrance 时由 catalog 补默认 0，配方出图不受影响。"""
        p = recipecatalog.apply_overrides("natural", {})
        step = next(s for s in p["steps"] if s["op"] == "auto_grade")
        self.assertEqual(step["params"]["vibrance"], 0.0)


if __name__ == "__main__":
    unittest.main()
