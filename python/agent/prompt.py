"""提示词编译器：把「配方 + 用户一句话 + 槽位」编译成可执行的执行规格与目标提示词。

这是整套交互能稳的唯一关键（docs/v5.md §12、§14）：

    RunSpec（执行规格）    ← 枚举/数值，确定性执行器消费，用户通过 UI 控件改
    PromptTemplate（模板）  ← 只读骨架，只有编译器读它，模型不许改写
    TargetPrompt（目标提示词）← 渲染产物，模型只填槽位

**模型只填槽位，不写骨架。** 这样做的好处是：同一预设批量 12 组时锚点段落
逐字一致（风格才一致）、换模板只需改数据、出问题能定位到"哪个槽位被填成了什么"。

本模块是纯函数（无网络、无磁盘 IO），因此可以高覆盖单测 —— 这是刻意的：
P1a「零出网」这条卖点必须能被断言，而不是靠"我们没写 http 调用"。
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from typing import Any

from . import validators as V
from .recipes.schema import PromptTemplate, Recipe, RunSpec

# P1a 未命中时给用户看的原话（§13.2）。措辞里必须给出"那该怎么办"，
# 否则用户只会觉得"我写的描述没被理睬"。
HINT_NO_RULE_MATCH = (
    "当前预设仅支持调整强度（更暖/更冷/更强/更弱），未识别到相关描述。"
    "如需 AI 重绘请切换至 AI 插画化或使用完全自定义。"
)

# §13.2 规则映射表。顺序即优先级：先判"更强/更弱"这类绝对档，再判相对档。
_RULE_TABLE: tuple[tuple[tuple[str, ...], str], ...] = (
    (("更强", "重一点", "浓一点", "浓一些", "猛一点", "最强", "强一点"), "strong"),
    (("更弱", "轻一点", "淡一点", "淡一些", "自然点", "自然一点", "轻一些"), "subtle"),
    (("再强", "加重", "加强"), "up"),
    (("再弱", "减弱", "轻些"), "down"),
    (("暖", "温暖", "夕阳", "黄昏", "金色"), "up"),
    (("冷", "清冷", "清爽", "青蓝"), "down"),
)


class CompileError(ValueError):
    """编译失败（带可直接展示给用户的中文文案）。"""


@dataclass
class CompileInput:
    recipe: Recipe
    user_text: str = ""
    tray_paths: list[str] = field(default_factory=list)
    thumbs: list[str] = field(default_factory=list)
    ref_image: str | None = None
    user_edited_prompt: str | None = None
    person_names: list[str] = field(default_factory=list)
    run_spec_override: dict | None = None
    # 端点是否已配置（决定“需要模型的预设”是直接拒还是继续走 P1b）
    provider_ready: bool = False


@dataclass
class CompileOutput:
    run_spec: RunSpec
    p1_path: str
    prompt: str | None
    prompt_source: str  # template | user_edited | rule_mapped | none
    slots: dict
    plan_patch: dict
    assumptions: list[str]
    conflicts: list[str]
    vision_used: int
    need_replan: dict | None
    hint: str = ""
    # P1b：编译到此为止，槽位要等规划模型填（runtime 随后调 planner.fill_slots）
    plan_pending: bool = False
    # 原样保存用户描述：runtime 传给规划模型（编译器本身不出网）
    user_text: str = ""
    # 供 UI「执行参数预览 / 这次会做什么」用的可序列化视图
    preview: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "p1Path": self.p1_path,
            "promptSource": self.prompt_source,
            "prompt": self.prompt,
            "slots": self.slots,
            "planPatch": self.plan_patch,
            "assumptions": self.assumptions,
            "conflicts": self.conflicts,
            "visionUsed": self.vision_used,
            "needReplan": self.need_replan,
            "hint": self.hint,
            "planPending": self.plan_pending,
            "runSpec": self.run_spec.to_dict(),
            "preview": self.preview,
        }


# ------------------------------------------------------------------ P1a 规则映射


def rule_map(text: str, intensity: float) -> tuple[float, list[str], str]:
    """纯本地关键词映射。**不产生任何出网请求**（这条被单测断言）。

    返回 (新 intensity, 命中词列表, UI 提示文案)。
    命中相对档（"再暖一点"）时按档位阶梯移动，不越界。
    """
    t = (text or "").strip()
    if not t:
        return intensity, [], ""
    for words, action in _RULE_TABLE:
        hit = next((w for w in words if w in t), None)
        if not hit:
            continue
        level = V.intensity_level(intensity)
        if action in ("subtle", "moderate", "strong"):
            return V.intensity_value(action), [hit], ""
        order = ["subtle", "moderate", "strong"]
        i = order.index(level)
        i = max(0, min(len(order) - 1, i + (1 if action == "up" else -1)))
        return V.intensity_value(order[i]), [hit], ""
    return intensity, [], HINT_NO_RULE_MATCH


# ------------------------------------------------------------------ 冲突表（§16.2）


def _negated(text: str) -> bool:
    """这句里有没有否定词（用于避免"不要 PDF"被当成"要 PDF"）。"""
    return any(w in text for w in ("不要", "不用", "不需", "无需", "别", "don't", "no "))


def _apply_conflicts(text: str, spec: RunSpec) -> tuple[RunSpec, list[str]]:
    """按冲突表调整执行规格。只处理「用户明确说了」的冲突。

    纪律：**只有明确冲突才允许改交付物**（写"不要 PDF"才改），
    其余一律只填参 —— 用户选了下拉又被模型改掉会困惑。
    """
    t = (text or "").strip()
    conflicts: list[str] = []
    if not t:
        return spec, conflicts

    # 不出 PDF
    if spec.deliverable == "card_pdf" and any(
        w in t for w in ("不要pdf", "不要 PDF", "不要pdf文件", "发朋友圈", "图片就行", "不要文件")
    ):
        spec.deliverable = "card_images"
        conflicts.append("已按你说的不出 PDF，改为导出图片")

    # 要文件 / 要打印
    # ⚠️ 必须排除否定语境："不要 PDF" 里包含子串 "要 PDF"，不排除的话两条规则
    #    会来回翻转，最终交付物变成用户明确不要的那个（实测踩过）。
    if spec.deliverable == "card_images" and not _negated(t) and any(
        w in t for w in ("打印", "给我一份文件", "要pdf", "要 PDF")
    ):
        spec.deliverable = "card_pdf"
        conflicts.append("已按你说的生成 PDF 文件")

    # AI 重绘：需要切 remote 引擎（本版本未接 provider 时明确告知，不静默降级）
    if spec.style and spec.style.engine == "local" and any(
        w in t for w in ("ai画", "AI 画", "AI画", "插画", "重绘", "画成")
    ):
        conflicts.append(
            "“AI 重绘”需要切换到模型引擎，当前未配置模型服务；本次仍按本地算法执行。"
        )

    # 人像强度
    if spec.retouch and any(w in t for w in ("再自然一点", "自然点", "自然一点", "别那么假")):
        spec.retouch.strength = "mild"
        conflicts.append("人像强度已调为更自然")

    # 别改人脸（任何配方）
    if spec.retouch and any(w in t for w in ("别改人脸", "不要改脸", "别动脸", "不要动脸", "别修脸")):
        spec.retouch.faces = False
        conflicts.append("已按你说的不改人脸")

    return spec, conflicts


# ------------------------------------------------------------------ 渲染（确定性）


def render(tpl: PromptTemplate, slots: dict) -> str:
    """骨架渲染。**anchor 永远在最前且不可省略**（§14.3 第 1 条）。"""
    try:
        body = tpl.skeleton.format(**slots)
    except KeyError as e:
        raise CompileError(f"提示词模板缺少槽位 {e}") from e
    return f"{tpl.anchor}\n\n{body}".strip()


def _default_slots(tpl: PromptTemplate, override: dict | None = None) -> dict:
    slots = {name: spec.default for name, spec in tpl.slots.items()}
    for k, v in (override or {}).items():
        if k in slots:
            slots[k] = str(v)
    return slots


def _validate_slots(tpl: PromptTemplate, slots: dict) -> tuple[dict, list[str]]:
    """槽位校验：enum 必须命中候选值，text 截断到 maxLen。"""
    assumptions: list[str] = []
    clean: dict[str, Any] = {}
    for name, spec in tpl.slots.items():
        raw = slots.get(name, spec.default)
        if spec.type == "enum":
            val = str(raw)
            if val not in spec.values:
                assumptions.append(f"槽位 {name} 的取值 {val!r} 不在候选内，已回落默认值")
                val = spec.default
            clean[name] = val
        else:
            val = str(raw or "")
            if spec.max_len and len(val) > spec.max_len:
                val = val[: spec.max_len]
                assumptions.append(f"槽位 {name} 超过 {spec.max_len} 字，已截断")
            clean[name] = val
    return clean, assumptions


def _apply_override(spec: RunSpec, override: dict | None) -> tuple[RunSpec, list[str]]:
    """把（已校验的）前端覆盖合进 RunSpec 副本。不改入参。"""
    notes: list[str] = []
    clean = V.check_run_spec_override(override)
    if not clean:
        return spec, notes
    if "style" in clean and spec.style:
        if "intensity" in clean["style"]:
            spec.style.intensity = float(clean["style"]["intensity"])
            notes.append(f"强度按你的设置调整为 {V.intensity_level(spec.style.intensity)}")
    if "retouch" in clean and spec.retouch:
        if "strength" in clean["retouch"]:
            spec.retouch.strength = str(clean["retouch"]["strength"])
        if "faces" in clean["retouch"]:
            spec.retouch.faces = bool(clean["retouch"]["faces"])
    if "export" in clean:
        exp = clean["export"]
        if "targetKb" in exp:
            spec.export.target_kb = int(exp["targetKb"])
            notes.append(f"目标体积按你的设置调整为 {spec.export.target_kb}KB")
        if "maxEdge" in exp:
            spec.export.max_edge = int(exp["maxEdge"])
            notes.append(f"长边上限按你的设置调整为 {spec.export.max_edge}px")
        if "crop" in exp:
            spec.export.crop = str(exp["crop"])
    if "layout" in clean:
        lay = clean["layout"]
        if spec.layout is None:
            if lay:
                raise CompileError("该配方没有版式设置，不能覆盖 layout")
        else:
            if "template" in lay:
                spec.layout.template = str(lay["template"])
            if "title" in lay:
                spec.layout.title = str(lay["title"])
            if "subtitle" in lay:
                spec.layout.subtitle = str(lay["subtitle"])
    return spec, notes


# ------------------------------------------------------------------ 预览（UI 用）


def build_preview(recipe: Recipe, spec: RunSpec, p1_path: str) -> dict:
    """「这次会做什么」的人话列表 + 执行参数预览（§17 第 3、4 条）。

    local 引擎**不显示目标提示词**，显示参数（色温/高光/饱和/强度）；
    不消费提示词的配方显示参数（强度档/版式/页数）。
    """
    steps: list[str] = []
    params: dict[str, str] = {}

    if spec.style:
        from .imaging.style_presets import preset_params  # 延迟导入，避免循环

        level = V.intensity_level(spec.style.intensity)
        p = preset_params(spec.style.preset, level)
        params = {
            "引擎": "本地滤镜" if spec.style.engine == "local" else "模型重绘",
            "色温": f"{p['temp']:+d}",
            "高光": f"{p['highlight']:+d}",
            "阴影": f"{p['shadow']:+d}",
            "饱和": f"{p['saturation']:+d}",
            "强度": level,
        }
        steps.append(f"本地滤镜 {spec.style.preset}（强度 {level}）")

    if spec.retouch:
        params["人像强度"] = spec.retouch.strength
        params["人脸区域"] = "是" if spec.retouch.faces else "否"
        steps.append(f"人像润色（{spec.retouch.strength}）")

    if spec.layout:
        params["版式"] = spec.layout.template
        steps.append(f"排版 {spec.layout.template}")

    if spec.export.target_kb:
        params["目标体积"] = f"{spec.export.target_kb}KB"
        steps.append(f"压缩到 {spec.export.target_kb}KB 以内")
    if spec.export.max_edge:
        params["长边上限"] = f"{spec.export.max_edge}px"
    if spec.export.max_pages and spec.deliverable in ("card_pdf", "card_images"):
        params["最多页数"] = str(spec.export.max_pages)

    return {
        "deliverable": spec.deliverable,
        "engine": (spec.style.engine if spec.style else "local"),
        "steps": steps,
        "params": params,
        "p1Path": p1_path,
        "needsProvider": recipe.needs_provider,
    }


# ------------------------------------------------------------------ 主编译入口


def compile_prompt(inp: CompileInput) -> CompileOutput:
    """把输入编译成 (RunSpec 副本, 目标提示词, 路径标签, 假设/冲突)。"""
    recipe = inp.recipe
    assumptions: list[str] = []
    conflicts: list[str] = []

    # 0) RunSpec 副本（绝不改入参 —— 配方是全局共享的只读数据）
    spec: RunSpec = copy.deepcopy(recipe.run_spec)
    spec, notes = _apply_override(spec, inp.run_spec_override)
    assumptions.extend(notes)

    # 1) 冲突裁决（先于路径选择：交付物可能被"不要 PDF"改掉）
    spec, conflict_notes = _apply_conflicts(inp.user_text, spec)
    conflicts.extend(conflict_notes)

    text = (inp.user_text or "").strip()

    # 2) 路径判定
    if inp.user_edited_prompt is not None and inp.user_edited_prompt.strip():
        p1_path = "P3"
    elif not text:
        p1_path = "P0"
    elif recipe.engine_behavior == "remote_prompt":
        # P1b：模型填参。具体槽位由 runtime 调 planner.fill_slots 后回填
        p1_path = "P1b"
    elif recipe.engine_behavior == "local_rule_mapped":
        p1_path = "P1a"
    else:  # no_prompt
        p1_path = "P1c"

    # 3) 出网需求闸：没配端点就在代码层拦住（不靠拒绝服务，而是“先告诉用户去哪配”）
    needs_model = recipe.needs_provider or bool(spec.subject and spec.subject.mode == "extract")
    if needs_model and not inp.provider_ready:
        raise CompileError(
            "该预设需要模型服务（AI 重绘 / 抠主体）。请先在设置里填写模型地址与 Key。"
        )
    if p1_path == "P1b" and not inp.provider_ready:
        raise CompileError(
            "该预设需要用模型理解你的描述（P1b），请先在设置里填写规划模型地址与 Key。"
        )

    slots: dict = {}
    prompt: str | None = None
    prompt_source = "none"
    hint = ""
    vision_used = 0
    plan_pending = False

    if p1_path == "P0":
        assumptions.append("未填写描述：按预设默认参数执行")
        if recipe.prompt_template and (spec.style and spec.style.engine == "remote"):
            slots = _default_slots(recipe.prompt_template)
            slots, slot_notes = _validate_slots(recipe.prompt_template, slots)
            assumptions.extend(slot_notes)
            prompt = _finalize_prompt(recipe.prompt_template, slots, assumptions)
            prompt_source = "template"

    elif p1_path == "P1a":
        base = spec.style.intensity if spec.style else 0.6
        new_val, hits, rule_hint = rule_map(text, base)
        if spec.style:
            spec.style.intensity = new_val
        if hits:
            level = V.intensity_level(new_val)
            prompt_source = "rule_mapped"
            assumptions.append(
                f"描述『{hits[0]}』已按本地规则映射为 intensity={level}（零成本，未出网）"
            )
        else:
            hint = rule_hint
            assumptions.append("描述未命中本地规则，参数保持不变（未出网）")

    elif p1_path == "P1c":
        # 不消费描述的配方：描述只进 metadata 供轨迹回看（§13.1）
        prompt_source = "none"
        assumptions.append("该预设不消费文字描述：描述仅记录在轨迹里，不改变执行参数")

    elif p1_path == "P1b":
        # 编译到此，槽位等 runtime 调规划模型填（不在这里出网：编译器是纯函数）
        plan_pending = True
        prompt_source = "template"
        assumptions.append("描述将交给规划模型填槽位（会产生一次很小的调用费）")

    else:  # P3
        prompt, assumptions2 = _finalize_user_prompt(inp, spec, assumptions)
        assumptions.extend(assumptions2)
        prompt_source = "user_edited"

    # 4) 交付物与路径一致性
    if spec.deliverable in ("card_pdf", "card_images") and not spec.layout:
        raise CompileError("卡片类配方缺少 layout 规格")

    out = CompileOutput(
        run_spec=spec,
        p1_path=p1_path,
        prompt=prompt,
        prompt_source=prompt_source,
        slots=slots,
        plan_patch={"slots": slots} if slots else {},
        assumptions=assumptions,
        conflicts=conflicts,
        vision_used=vision_used,
        need_replan=None,
        hint=hint,
        plan_pending=plan_pending,
        user_text=text,
        preview=build_preview(recipe, spec, p1_path),
    )
    return out


def finalize_filled(
    tpl: PromptTemplate, raw_slots: dict, assumptions: list[str]
) -> tuple[dict, str]:
    """把规划模型填的槽位“收口”成最终提示词。

    模型只能影响槽位值：枚举越界回落默认、超长截断、禁用词剔除、锚点强制前置。
    这四步全部在本地完成 —— 模型没有任何机会改写骨架。
    """
    slots, notes = _validate_slots(tpl, raw_slots)
    assumptions.extend(notes)
    text = _finalize_prompt(tpl, slots, assumptions)
    return slots, text


def _finalize_prompt(tpl: PromptTemplate, slots: dict, assumptions: list[str]) -> str:
    """模板渲染 + 长度降级 + 锚点补回 + 禁用词清理。纯粹函数，不返回“来源”。"""
    slots, trunc_notes = V.truncate_slots(slots, tpl.max_chars, lambda s: render(tpl, s))
    assumptions.extend(trunc_notes)
    text = render(tpl, slots)
    text, hits = V.strip_forbidden(text)
    if hits:
        assumptions.append(f"已剔除禁用词 {hits}")
    text, patched = V.ensure_anchor(text, tpl.anchor)
    if patched:
        assumptions.append("已把锚点补回提示词开头")
    return text


def _finalize_user_prompt(
    inp: CompileInput, spec: RunSpec, assumptions: list[str]
) -> tuple[str, list[str]]:
    """P3 手改提示词：批量拒绝、超长截断、锚点补回、禁用词剔除、隐私替换。"""
    tpl = inp.recipe.prompt_template
    # 先判批量：批量 + 手改是最典型的误用，且与"有没有模板"无关。
    if len(inp.tray_paths) > 1 or spec.deliverable != "single_image":
        raise CompileError(
            "手改提示词模式仅支持单图处理。批量任务需要为每张图独立填充槽位，"
            "手改会导致所有图使用同一份描述。请使用预设，或改用卡片预设的标题字段填统一文案。"
        )
    if tpl is None:
        raise CompileError(
            "该预设不消费提示词，没有可编辑的目标提示词。手改提示词只对会生成新图的预设有效。"
        )

    notes: list[str] = []
    text = (inp.user_edited_prompt or "").strip()

    # 超长：按模板 maxChars 硬截断（§16.4）
    limit = tpl.max_chars
    if len(text) > limit:
        text = text[:limit]
        notes.append(f"提示词超出 {limit} 字上限，超出部分已忽略")

    text, banned = V.strip_forbidden(text)
    if banned:
        notes.append(f"已剔除禁用词 {banned}")
    text, names = V.strip_person_names(text, inp.person_names)
    if names:
        notes.append(f"已把人物姓名 {names} 替换为 the person（隐私）")
    text, patched = V.ensure_anchor(text, tpl.anchor)
    if patched:
        notes.append("已把锚点补回提示词开头")
    return text, notes


__all__ = [
    "CompileInput",
    "CompileOutput",
    "CompileError",
    "compile_prompt",
    "render",
    "rule_map",
    "build_preview",
    "HINT_NO_RULE_MATCH",
]
