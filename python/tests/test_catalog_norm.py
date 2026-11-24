"""catalog.norm_params 的归一行为（FEAT-220 修 bool 缺省值）。

`norm_params` 是所有来源（LLM / 配方 / 前端 / 已保存的计划）参数的唯一入口，
它静默改义比报错更危险：调用方全拿不到提示。这里钉死三类语义：

  1. **缺省 → schema default**，尤其 `bool`：`bool(None)` 会得到 False，
     曾把 `color_transfer.keep_exposure`（默认 true）静默关掉；
  2. **显式值优先**：旧配方里写死的 `method=reinhard` / `keep_exposure=false` 必须原样保留；
  3. **越界/非法 → 夹取或回落 default**，且只保留 schema 里存在的键。
"""
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

from ai_studio import catalog  # noqa: E402

_OP = "color_transfer"


class TestNormParams(unittest.TestCase):
    def test_bool_falls_back_to_schema_default(self):
        got = catalog.norm_params(_OP, {})
        self.assertIs(got["keep_exposure"], True, "bool 缺省必须回 schema default（true）")
        self.assertIs(got["protect_skin"], False, "bool 缺省必须回 schema default（false）")

    def test_explicit_values_are_preserved(self):
        got = catalog.norm_params(
            _OP, {"method": "reinhard", "tone_weight": 0.3, "keep_exposure": False}
        )
        self.assertEqual(got["method"], "reinhard", "旧配方的方法值不得被静默改成 adaptive")
        self.assertAlmostEqual(got["tone_weight"], 0.3)
        self.assertIs(got["keep_exposure"], False)

    def test_string_bools_are_parsed_not_truthy(self):
        self.assertIs(catalog.norm_params(_OP, {"keep_exposure": "false"})["keep_exposure"], False)
        self.assertIs(catalog.norm_params(_OP, {"keep_exposure": "true"})["keep_exposure"], True)

    def test_unknown_enum_falls_back_to_default(self):
        self.assertEqual(catalog.norm_params(_OP, {"method": "nope"})["method"], "adaptive")

    def test_range_is_clamped(self):
        got = catalog.norm_params(_OP, {"strength": 9.0})
        self.assertEqual(float(got["strength"]), 1.0)

    def test_unknown_keys_are_dropped(self):
        got = catalog.norm_params(_OP, {"not_a_param": 1})
        self.assertNotIn("not_a_param", got)
        self.assertEqual(set(got), set(catalog.op_spec(_OP)["params"]))


if __name__ == "__main__":
    unittest.main(verbosity=2)
