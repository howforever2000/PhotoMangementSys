"""配方 + 提示词编译器：把 docs/v5.md 里最容易被破坏的规则钉成断言。

重点覆盖（每条都对应过一个具体坏结果）：
- P0 / P1a / P1c 三条路径的判定，以及 **P1a 零出网**；
- 锚点必须出现在最终提示词开头；
- 禁用词剔除、人物姓名替换（隐私）；
- 冲突表（"不要 PDF" 才改交付物）；
- scope 冻结（P1 不许改范围）；
- P3 批量拒稿、超长截断；
- 批量一致性（同 slots 12 组 → 锚点段落逐字相同）。
"""
from __future__ import annotations

import pytest

from agent.prompt import HINT_NO_RULE_MATCH, CompileError, CompileInput, compile_prompt, render, rule_map
from agent.recipes.schema import Recipe, RecipeError, load_registry
from agent.validators import intensity_level, strip_forbidden, strip_person_names

REG = load_registry()
BY_ID = {r.id: r for r in REG.all()}


def compile_for(recipe_id: str, text: str = "", paths=None, **kw):
    return compile_prompt(
        CompileInput(recipe=BY_ID[recipe_id], user_text=text, tray_paths=paths or ["a.jpg"], **kw)
    )


# ------------------------------------------------------------------ 注册表


def test_catalog_loads_and_ids_are_namespaced():
    ids = sorted(BY_ID)
    assert "style.warm_dusk" in ids
    assert "card.magazine_3up" in ids
    assert "compress.target_kb" in ids
    assert all(i.startswith(("style.", "retouch.", "card.", "batch.", "compress.")) for i in ids)


def test_unknown_recipe_fails_loudly():
    with pytest.raises(RecipeError):
        REG.get("style.nope")


def test_template_is_resolved_from_catalog_for_prompt_templates():
    tpl = BY_ID["style.warm_dusk"].prompt_template
    assert tpl is not None
    assert tpl.anchor.startswith("Preserve the original composition")
    assert set(tpl.slots) == {"look", "intensity", "note"}


# ------------------------------------------------------------------ 路径判定


def test_p0_when_no_text():
    out = compile_for("style.warm_dusk", "")
    assert out.p1_path == "P0"
    assert out.prompt is None  # local 引擎不产出 prompt
    assert out.prompt_source == "none"


def test_p1a_rule_mapping_is_local_and_changes_intensity():
    out = compile_for("style.warm_dusk", "再暖一点")
    assert out.p1_path == "P1a"
    assert out.prompt is None, "P1a 不产出目标提示词（本地引擎不吃 prompt）"
    assert out.prompt_source == "rule_mapped"
    assert intensity_level(out.run_spec.style.intensity) == "strong"
    assert any("intensity=strong" in a for a in out.assumptions)


def test_p1a_unmatched_keyword_keeps_params_and_hints():
    out = compile_for("style.warm_dusk", "像莫奈那样")
    assert out.p1_path == "P1a"
    assert intensity_level(out.run_spec.style.intensity) == "moderate", "未命中不得改参数"
    assert out.hint == HINT_NO_RULE_MATCH
    assert out.run_spec.style.intensity == pytest.approx(0.6)


def test_p1c_does_not_consume_text():
    out = compile_for("card.magazine_3up", "做成杂志风")
    assert out.p1_path == "P1c"
    assert out.prompt is None
    assert out.run_spec.layout.template == "magazine_3up"
    assert any("不消费文字描述" in a for a in out.assumptions)


@pytest.mark.parametrize(
    ("text", "expect"),
    [
        ("更强", "strong"),
        ("轻一点", "subtle"),
        ("再暖一点", "strong"),
        ("冷一些", "subtle"),
        ("浓一点", "strong"),
    ],
)
def test_rule_table(text, expect):
    level, hits, hint = rule_map(text, 0.6)
    assert hits, f"{text!r} 应该命中规则"
    assert intensity_level(level) == expect
    assert hint == ""


def test_rule_map_is_bounded():
    # 已在 strong 时再说"更暖"不该越界
    level, _, _ = rule_map("更暖", 0.9)
    assert intensity_level(level) == "strong"


# ------------------------------------------------------------------ 冲突表


def test_conflict_no_pdf_switches_deliverable():
    out = compile_for("card.magazine_3up", "不要 PDF，发朋友圈")
    assert out.run_spec.deliverable == "card_images"
    assert any("不出 PDF" in c for c in out.conflicts)


def test_conflict_ai_repaint_only_warns_when_no_provider():
    out = compile_for("style.warm_dusk", "AI 画一下")
    assert out.run_spec.style.engine == "local", "没有 provider 时不得静默切远程"
    assert any("AI 重绘" in c for c in out.conflicts)


def test_conflict_do_not_touch_faces():
    out = compile_for("retouch.portrait_natural", "别改人脸")
    assert out.run_spec.retouch.faces is False


def test_conflict_more_natural_downgrades_strength():
    out = compile_for("retouch.portrait_natural", "再自然一点")
    assert out.run_spec.retouch.strength == "mild"


# ------------------------------------------------------------------ 校验器


def test_forbidden_terms_are_stripped():
    text, hits = strip_forbidden("把这张修好看，带上我的微信二维码")
    assert "二维码" in hits
    assert "二维码" not in text


def test_person_names_are_replaced_with_the_person():
    text, hits = strip_person_names("把张三修好看点", ["张三"])
    assert hits == ["张三"]
    assert "张三" not in text
    assert "the person" in text


def test_anchor_always_rendered_first():
    tpl = BY_ID["style.warm_dusk"].prompt_template
    slots = {"look": "warm_dusk", "intensity": "moderate", "note": "夕阳逆光"}
    text = render(tpl, slots)
    assert text.startswith(tpl.anchor)


def test_batch_consistency_anchor_block_identical():
    # 同 slots、12 组输入 → 锚点段落必须逐字相同（这是"批量风格一致"的机制保证）
    tpl = BY_ID["style.warm_dusk"].prompt_template
    anchors = {
        render(tpl, {"look": "warm_dusk", "intensity": "moderate", "note": f"第{i}张"}).split("\n\n")[0]
        for i in range(12)
    }
    assert len(anchors) == 1


# ------------------------------------------------------------------ P3


def test_p3_rejects_batch():
    with pytest.raises(CompileError) as ei:
        compile_for(
            "card.magazine_3up",
            "",
            paths=["a.jpg", "b.jpg", "c.jpg"],
            user_edited_prompt="make it look like a magazine cover",
        )
    assert "仅支持单图" in str(ei.value)


def test_p3_truncates_and_restores_anchor():
    tpl = BY_ID["style.warm_dusk"].prompt_template
    long_text = "x" * (tpl.max_chars + 400)
    out = compile_for(
        "style.warm_dusk", "", paths=["a.jpg"], user_edited_prompt=long_text
    )
    assert out.p1_path == "P3"
    assert out.prompt.startswith(tpl.anchor)
    assert len(out.prompt) <= tpl.max_chars + len(tpl.anchor) + 4
    assert any("上限" in a for a in out.assumptions)


def test_p3_strips_person_names_and_forbidden_terms():
    out = compile_for(
        "style.warm_dusk",
        "",
        paths=["a.jpg"],
        user_edited_prompt="make 张三 look warm, add my 二维码",
        person_names=["张三"],
    )
    assert "张三" not in out.prompt
    assert "二维码" not in out.prompt
    assert "the person" in out.prompt


def test_p3_rejected_for_recipe_without_prompt_template():
    with pytest.raises(CompileError) as ei:
        compile_for("retouch.portrait_natural", "", user_edited_prompt="smooth more")
    assert "不消费提示词" in str(ei.value)


# ------------------------------------------------------------------ 覆盖与冻结


def test_run_spec_override_target_kb_is_applied():
    out = compile_for("compress.target_kb", "", run_spec_override={"export": {"targetKb": 300}})
    assert out.run_spec.export.target_kb == 300
    assert any("300" in a or "设置" in a for a in out.assumptions)


def test_run_spec_override_rejects_unknown_field():
    with pytest.raises(Exception) as ei:
        compile_for("compress.target_kb", "", run_spec_override={"export": {"engine": "remote"}})
    assert "不允许" in str(ei.value)


def test_scope_is_frozen_to_tray_in_p1():
    # 用户写"处理 2023 年在杭州的所有照片"也不会改 scope（P1 冻结，P2 才走 needReplan）
    out = compile_for("style.warm_dusk", "把 2023 年在杭州拍的全部照片都处理一下")
    assert out.run_spec.scope.mode == "tray"
    assert out.p1_path == "P1a"


def test_preview_never_exposes_prompt_for_local_engine():
    out = compile_for("style.warm_dusk", "")
    assert out.preview["engine"] == "local"
    assert out.preview["params"]["色温"].startswith("+")
    assert "prompt" not in out.preview
