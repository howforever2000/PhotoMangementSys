"""FEAT-197 计划层约束单测：重复算子 / 步数上限 / 超分两遍 / 云端白名单 / 上采样。

钉死的语义：
  1. 同一算子允许重复出现（创意组合支持多次插入），依赖校验按「首次出现位置」；
  2. 步数上限 10，超出**报错**而不是静默截断（截断会让用户排的后几步悄悄消失）；
  3. 超分同族最多 2 次（4× = 2× 跑两遍），3 次以上报错；
  4. 「模型理解」只允许云端算子，本机算子一律拒；
  5. 上采样改写计划：不勾时摘掉规划器自带的超分步，勾 4× 时补两步 2×。
"""
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import catalog, pipeline, presets, recipecatalog, upsample  # noqa: E402

CLOUD = catalog.cloud_ops()


class TestDuplicateOps(unittest.TestCase):
    def test_same_op_twice_allowed(self):
        p = pipeline.validate_plan(
            {"steps": [{"op": "beauty", "params": {}}, {"op": "beauty", "params": {}}]}
        )
        self.assertEqual([s["op"] for s in p["steps"]], ["beauty", "beauty"])

    def test_superres_twice_allowed(self):
        p = pipeline.validate_plan(
            {
                "steps": [
                    {"op": "upscale", "params": {"scale": 2}},
                    {"op": "upscale", "params": {"scale": 2}},
                ]
            }
        )
        self.assertEqual(len(p["steps"]), 2)

    def test_superres_three_times_rejected(self):
        with self.assertRaises(pipeline.PlanError):
            pipeline.validate_plan(
                {"steps": [{"op": "upscale", "params": {"scale": 2}} for _ in range(3)]}
            )

    def test_dependency_order_with_duplicates(self):
        # local_film 必须在 auto_grade 之后；重复插入也成立
        ok = pipeline.validate_plan(
            {
                "steps": [
                    {"op": "auto_grade", "params": {}},
                    {"op": "local_film", "params": {}},
                    {"op": "auto_grade", "params": {}},
                    {"op": "local_film", "params": {}},
                ]
            }
        )
        self.assertEqual(len(ok["steps"]), 4)

    def test_reverse_order_rejected(self):
        with self.assertRaises(pipeline.PlanError):
            pipeline.validate_plan(
                {"steps": [{"op": "local_film", "params": {}}, {"op": "auto_grade", "params": {}}]}
            )


class TestStepLimit(unittest.TestCase):
    def test_exactly_max_steps_ok(self):
        steps = [{"op": "beauty", "params": {}} for _ in range(pipeline.MAX_STEPS)]
        p = pipeline.validate_plan({"steps": steps})
        self.assertEqual(len(p["steps"]), pipeline.MAX_STEPS)

    def test_over_max_steps_raises(self):
        steps = [{"op": "beauty", "params": {}} for _ in range(pipeline.MAX_STEPS + 1)]
        with self.assertRaises(pipeline.PlanError):
            pipeline.validate_plan({"steps": steps})

    def test_frontend_limit_matches_backend(self):
        """前端 `MAX_CHAIN_STEPS` 必须与后端 `MAX_STEPS` 同口径（防两处漂移）。"""
        ts = os.path.join(ROOT, "src", "utils", "workbenchPlan.ts")
        with open(ts, "r", encoding="utf-8") as fh:
            src = fh.read()
        self.assertIn(f"MAX_CHAIN_STEPS = {pipeline.MAX_STEPS}", src)


class TestCloudOnlyPlanner(unittest.TestCase):
    def test_cloud_ops_excludes_local_and_internal(self):
        self.assertIn("beauty", CLOUD)
        self.assertIn("style_transfer", CLOUD)
        self.assertIn("upscale", CLOUD)
        self.assertNotIn("auto_grade", CLOUD)
        self.assertNotIn("local_upscale", CLOUD)
        self.assertNotIn("refine", CLOUD)  # internal

    def test_planner_rejects_local_op(self):
        with self.assertRaises(pipeline.PlanError):
            pipeline.validate_plan(
                {"steps": [{"op": "auto_grade", "params": {}}]}, allowed=CLOUD
            )

    def test_planner_prompt_has_no_local_ops(self):
        text = presets.planner_prompt("调通透一点")
        for op in ("auto_grade", "local_film", "face_reshape", "skin_smooth", "local_upscale"):
            self.assertNotIn(op, text)


class TestUpsample(unittest.TestCase):
    def test_scale_normalization(self):
        self.assertEqual(upsample.normalize_upsample_scale(4), 4)
        self.assertEqual(upsample.normalize_upsample_scale("2"), 2)
        self.assertEqual(upsample.normalize_upsample_scale(None), 2)
        self.assertEqual(upsample.normalize_upsample_scale(99), 2)
        self.assertEqual(upsample.passes_for(2), 1)
        self.assertEqual(upsample.passes_for(4), 2)

    def test_off_strips_planner_superres(self):
        plan = {"intent": "x", "steps": [{"op": "beauty", "params": {}}, {"op": "upscale", "params": {"scale": 4}}]}
        out = upsample.plan_with_upsample(plan, False, 2)
        self.assertEqual([s["op"] for s in out["steps"]], ["beauty"])

    def test_4x_appends_two_2x_passes(self):
        plan = {"intent": "x", "steps": [{"op": "beauty", "params": {}}]}
        out = upsample.plan_with_upsample(plan, True, 4)
        self.assertEqual([s["op"] for s in out["steps"]], ["beauty", "upscale", "upscale"])
        self.assertTrue(all(s["params"]["scale"] == 2 for s in out["steps"] if s["op"] == "upscale"))

    def test_2x_appends_one_pass(self):
        plan = {"intent": "x", "steps": [{"op": "beauty", "params": {}}]}
        out = upsample.plan_with_upsample(plan, True, 2)
        self.assertEqual([s["op"] for s in out["steps"]], ["beauty", "upscale"])

    def test_upsampled_plan_passes_cloud_validation(self):
        plan = {"intent": "x", "steps": [{"op": "style_transfer", "params": {}}]}
        out = upsample.plan_with_upsample(plan, True, 4)
        validated = pipeline.validate_plan(out, allowed=CLOUD)
        self.assertEqual(len(validated["steps"]), 3)

    def test_meta_shape(self):
        m = upsample.upsample_meta(True, 4)
        self.assertEqual(m, {"upsample": True, "upsampleScale": 4, "upsamplePasses": 2})
        self.assertEqual(upsample.upsample_meta(False, 4)["upsamplePasses"], 0)


class TestRenderSteps(unittest.TestCase):
    def test_default_is_one_and_not_sent(self):
        from ai_studio.renderopts import RunContext, clamp_render_steps

        self.assertEqual(clamp_render_steps(None), 1)
        self.assertEqual(clamp_render_steps(0), 1)
        self.assertEqual(clamp_render_steps(-3), 1)
        self.assertEqual(clamp_render_steps("abc"), 1)
        self.assertIsNone(RunContext(1).gateway_steps())
        self.assertEqual(RunContext(8).gateway_steps(), 8)

    def test_clamped_upper_bound(self):
        from ai_studio.renderopts import MAX_RENDER_STEPS, clamp_render_steps

        self.assertEqual(clamp_render_steps(9999), MAX_RENDER_STEPS)


class TestRecipesStillValid(unittest.TestCase):
    def test_all_recipes_pass_validation(self):
        for r in recipecatalog.recipes():
            plan = recipecatalog.plan_of(r["id"])
            self.assertTrue(plan["steps"], r["id"])


if __name__ == "__main__":
    unittest.main()
