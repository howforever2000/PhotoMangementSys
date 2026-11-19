"""FEAT-198 模型风格提示词单测：载入 / 双通道 / 注入 / 边界。

钉死的语义：
  1. 6 条风格可载入，id 唯一、中文原文与英文要点都非空；
  2. 中文与英文走**两条通道**（中文给可见输入框、英文进提示词），不重复下发；
  3. 未知 id 不抛错、返回空（可选增强不该让整单失败）；
  4. 风格段经 `RunContext.apply_style` 追加，空风格原样透传；
  5. 规划提示词里含可选风格清单；选中某款时点名它。
"""
import os
import sys
import unittest

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "python"))

from ai_studio import modelstyle, presets  # noqa: E402
from ai_studio.renderopts import RunContext  # noqa: E402

WANT_IDS = (
    "hk90",
    "kodak_gold_200",
    "wong_kar_wai",
    "cinestill_800t",
    "portra_400",
    "fuji_pro_160ns",
)


class TestLoad(unittest.TestCase):
    def test_ids_and_order(self):
        self.assertEqual(modelstyle.style_ids(), WANT_IDS)

    def test_labels(self):
        self.assertEqual(modelstyle.label_of("hk90"), "90 年代港片")
        self.assertEqual(modelstyle.label_of("portra_400"), "柯达 Portra 400")
        self.assertEqual(modelstyle.label_of("nope"), "")

    def test_each_style_has_both_channels(self):
        for sid in WANT_IDS:
            self.assertGreaterEqual(len(modelstyle.compose_zh(sid)), 100, sid)
            self.assertGreaterEqual(len(modelstyle.compose_en(sid)), 80, sid)

    def test_unknown_id_is_empty_not_error(self):
        self.assertEqual(modelstyle.compose_zh("nope"), "")
        self.assertEqual(modelstyle.compose_en("nope"), "")
        self.assertEqual(modelstyle.en_block("nope"), "")
        self.assertEqual(modelstyle.compose("nope"), "")


class TestChannels(unittest.TestCase):
    def test_en_block_wraps_english_only(self):
        block = modelstyle.en_block("hk90")
        self.assertIn("Film/color style reference:", block)
        # 英文段里不该出现中文原文（否则等于重复下发）
        self.assertNotIn(modelstyle.compose_zh("hk90"), block)

    def test_compose_has_both(self):
        both = modelstyle.compose("hk90")
        self.assertIn(modelstyle.compose_zh("hk90"), both)
        self.assertIn(modelstyle.compose_en("hk90"), both)


class TestRunContext(unittest.TestCase):
    def test_apply_style_appends(self):
        ctx = RunContext(1, modelstyle.en_block("portra_400"))
        out = ctx.apply_style("Grade this photo.")
        self.assertTrue(out.startswith("Grade this photo."))
        self.assertTrue(out.endswith(modelstyle.en_block("portra_400")))

    def test_empty_style_passthrough(self):
        self.assertEqual(RunContext().apply_style("abc"), "abc")
        self.assertEqual(RunContext(1, "   ").apply_style("abc"), "abc")

    def test_render_steps_still_independent(self):
        ctx = RunContext(8, "style")
        self.assertEqual(ctx.gateway_steps(), 8)
        self.assertIsNone(RunContext(1, "style").gateway_steps())


class TestPlannerPrompt(unittest.TestCase):
    def test_lists_all_styles(self):
        text = presets.planner_prompt("调通透一点")
        for sid in WANT_IDS:
            self.assertIn(sid, text, f"规划提示词缺少风格 {sid}")

    def test_names_picked_style(self):
        text = presets.planner_prompt("随手拍", 0.6, "wong_kar_wai")
        self.assertIn("王家卫式", text)

    def test_no_hint_without_style(self):
        self.assertNotIn("挑了一款", presets.planner_prompt("随手拍"))

    def test_planner_still_cloud_only(self):
        text = presets.planner_prompt("随便", 0.5, "hk90")
        for bad in ("auto_grade", "local_film", "face_reshape", "skin_smooth"):
            self.assertNotIn(bad, text)


class TestDataGuard(unittest.TestCase):
    def test_every_zh_mentions_tone_vocabulary(self):
        keywords = ("高光", "暗部", "中间调", "影调", "肤色", "颗粒", "反差", "饱和")
        for sid in WANT_IDS:
            zh = modelstyle.compose_zh(sid)
            self.assertTrue(any(k in zh for k in keywords), f"{sid} 缺少影调描述")

    def test_brands_kept(self):
        raw = " ".join(modelstyle.compose_en(s) for s in WANT_IDS).lower()
        for brand in ("portra", "kodak", "cinestill", "fuji"):
            self.assertIn(brand, raw)

    def test_no_animation_studio_ip(self):
        raw = " ".join(
            modelstyle.compose_zh(s) + modelstyle.compose_en(s) for s in WANT_IDS
        ).lower()
        for bad in ("ghibli", "miyazaki", "disney", "pixar", "shinkai"):
            self.assertNotIn(bad, raw)


if __name__ == "__main__":
    unittest.main()
