"""Recipe（配方）结构 + 校验。

命名纪律（docs/v5.md §11）：`src/utils/presets.ts` / `customPresets.ts` 已占用
「预设 = 外观主题」。**代码一律叫 recipe**，UI 上才叫「预设」。

三个词的分离是整套交互能稳的唯一原因（§12），所以这里的类型也是三个：
- `RunSpec`      执行规格：交付物/范围/强度/引擎/导出，全是枚举与数值，**无自由文本**，
                 消费方是确定性执行器。用户在 UI 上通过控件改它。
- `PromptTemplate` 提示词模板：带槽位的句子骨架，**不可被模型改写**，只有编译器读它。
- TargetPrompt   渲染后的最终文本（不落在本文件，由 `prompt.py` 产出）。

数据文件放 `recipes/data/*.json`（配方）与 `recipes/templates/*.json`（模板）。
加载时**严格校验**：出现未知键直接报错 —— 拼错字段名最坏的结果是"配方静默不生效"，
那比启动失败难查一个数量级。
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

DATA_DIR = Path(__file__).resolve().parent
CATALOG_FILE = DATA_DIR / "catalog.json"
TEMPLATE_DIR = DATA_DIR / "templates"

# ---- 枚举（与 docs/v5.md §11.2 / §11.4 / §15 一一对应） ----

DELIVERABLES = ("single_image", "card_images", "card_pdf", "bundle")
ENGINE_BEHAVIORS = ("local_rule_mapped", "remote_prompt", "no_prompt")
STATUSES = ("ready", "planned", "needs_key")
ENGINES = ("local", "remote")
INTENSITIES = ("subtle", "moderate", "strong")
# intensity 的数值区间：RunSpec 里存 0~1，渲染/渲染 UI 时换算成三档
INTENSITY_LEVELS = {"subtle": (0.2, 0.4), "moderate": (0.4, 0.75), "strong": (0.75, 1.0)}
SUBJECT_MODES = ("contain", "extract")
SCOPES = ("tray", "picked", "query", "custom")


class RecipeError(ValueError):
    """配方数据/规格不合法。"""


def _need(d: dict, key: str, where: str) -> Any:
    if key not in d:
        raise RecipeError(f"{where}: 缺少必填字段 {key!r}")
    return d[key]


def _enum(value: Any, allowed: tuple[str, ...], where: str) -> str:
    if value not in allowed:
        raise RecipeError(f"{where}: 取值 {value!r} 不在允许范围 {allowed}")
    return str(value)


def _num(value: Any, where: str, lo: float = 0.0, hi: float = 1.0) -> float:
    try:
        v = float(value)
    except (TypeError, ValueError) as e:
        raise RecipeError(f"{where}: 需要数值，得到 {value!r}") from e
    if not (lo <= v <= hi):
        raise RecipeError(f"{where}: 数值 {v} 越界（应在 {lo}~{hi}）")
    return v


def _only_keys(d: dict, allowed: set[str], where: str) -> None:
    """未知键直接报错（防拼错字段名导致静默不生效）。"""
    extra = set(d) - allowed
    if extra:
        raise RecipeError(f"{where}: 出现未知字段 {sorted(extra)}（允许 {sorted(allowed)}）")


# ------------------------------------------------------------------ RunSpec


@dataclass
class Scope:
    mode: str = "tray"

    @staticmethod
    def from_dict(d: dict, where: str) -> "Scope":
        _only_keys(d, {"mode"}, where)
        return Scope(_enum(_need(d, "mode", where), SCOPES, f"{where}.mode"))


@dataclass
class StyleSpec:
    preset: str = ""
    intensity: float = 0.6
    engine: str = "local"

    @staticmethod
    def from_dict(d: dict, where: str) -> "StyleSpec":
        _only_keys(d, {"preset", "intensity", "engine"}, where)
        return StyleSpec(
            preset=str(_need(d, "preset", where)),
            intensity=_num(d.get("intensity", 0.6), f"{where}.intensity"),
            engine=_enum(d.get("engine", "local"), ENGINES, f"{where}.engine"),
        )


@dataclass
class RetouchSpec:
    faces: bool = False
    strength: str = "natural"

    @staticmethod
    def from_dict(d: dict, where: str) -> "RetouchSpec":
        _only_keys(d, {"faces", "strength"}, where)
        strength = str(d.get("strength", "natural"))
        if strength not in ("natural", "mild", "strong"):
            raise RecipeError(f"{where}.strength: 取值 {strength!r} 不在 (natural, mild, strong)")
        return RetouchSpec(faces=bool(d.get("faces", False)), strength=strength)


@dataclass
class SubjectSpec:
    mode: str = "contain"
    background: str = "#f4efe6"

    @staticmethod
    def from_dict(d: dict, where: str) -> "SubjectSpec":
        _only_keys(d, {"mode", "background"}, where)
        return SubjectSpec(
            mode=_enum(d.get("mode", "contain"), SUBJECT_MODES, f"{where}.mode"),
            background=str(d.get("background", "#f4efe6")),
        )


@dataclass
class LayoutSpec:
    template: str = "magazine_3up"
    title: str = ""
    subtitle: str = ""

    @staticmethod
    def from_dict(d: dict, where: str) -> "LayoutSpec":
        _only_keys(d, {"template", "title", "subtitle"}, where)
        return LayoutSpec(
            template=str(d.get("template", "magazine_3up")),
            title=str(d.get("title", "")),
            subtitle=str(d.get("subtitle", "")),
        )


@dataclass
class ExportSpec:
    formats: list[str] = field(default_factory=lambda: ["jpg"])
    max_edge: int = 0
    target_kb: int = 0
    crop: str = ""  # 如 "4:3"
    page_size: str = "a4"
    max_pages: int = 12
    overwrite: bool = False

    @staticmethod
    def from_dict(d: dict, where: str) -> "ExportSpec":
        _only_keys(
            d,
            {"formats", "maxEdge", "targetKb", "crop", "pageSize", "maxPages", "overwrite"},
            where,
        )
        formats = d.get("formats", ["jpg"])
        if not isinstance(formats, list) or not formats:
            raise RecipeError(f"{where}.formats: 需要非空数组")
        for f in formats:
            if f not in ("jpg", "png", "pdf"):
                raise RecipeError(f"{where}.formats: 不支持 {f!r}")
        return ExportSpec(
            formats=[str(f) for f in formats],
            max_edge=int(d.get("maxEdge", 0) or 0),
            target_kb=int(d.get("targetKb", 0) or 0),
            crop=str(d.get("crop", "")),
            page_size=str(d.get("pageSize", "a4")),
            max_pages=int(d.get("maxPages", 12)),
            overwrite=bool(d.get("overwrite", False)),
        )


@dataclass
class BudgetSpec:
    max_cost: float = 0.0
    max_calls: int = 0

    @staticmethod
    def from_dict(d: dict, where: str) -> "BudgetSpec":
        _only_keys(d, {"maxCost", "maxCalls"}, where)
        return BudgetSpec(
            max_cost=float(d.get("maxCost", 0.0)),
            max_calls=int(d.get("maxCalls", 0)),
        )


@dataclass
class RunSpec:
    deliverable: str
    scope: Scope
    style: StyleSpec | None
    retouch: RetouchSpec | None
    subject: SubjectSpec | None
    layout: LayoutSpec | None
    export: ExportSpec
    budget: BudgetSpec

    @staticmethod
    def from_dict(d: dict, where: str = "runSpec") -> "RunSpec":
        _only_keys(
            d, {"deliverable", "scope", "style", "retouch", "subject", "layout", "export", "budget"}, where
        )
        return RunSpec(
            deliverable=_enum(_need(d, "deliverable", where), DELIVERABLES, f"{where}.deliverable"),
            scope=Scope.from_dict(d.get("scope", {"mode": "tray"}), f"{where}.scope"),
            style=StyleSpec.from_dict(d["style"], f"{where}.style") if "style" in d else None,
            retouch=RetouchSpec.from_dict(d["retouch"], f"{where}.retouch") if "retouch" in d else None,
            subject=SubjectSpec.from_dict(d["subject"], f"{where}.subject") if "subject" in d else None,
            layout=LayoutSpec.from_dict(d["layout"], f"{where}.layout") if "layout" in d else None,
            export=ExportSpec.from_dict(d.get("export", {}), f"{where}.export"),
            budget=BudgetSpec.from_dict(d.get("budget", {}), f"{where}.budget"),
        )

    def to_dict(self) -> dict:
        """给前端「这次会做什么 / 执行参数预览」用的可序列化视图。"""
        out: dict[str, Any] = {
            "deliverable": self.deliverable,
            "scope": {"mode": self.scope.mode},
            "export": {
                "formats": list(self.export.formats),
                "maxEdge": self.export.max_edge,
                "targetKb": self.export.target_kb,
                "crop": self.export.crop,
                "pageSize": self.export.page_size,
                "maxPages": self.export.max_pages,
                "overwrite": self.export.overwrite,
            },
            "budget": {"maxCost": self.budget.max_cost, "maxCalls": self.budget.max_calls},
        }
        if self.style:
            out["style"] = {
                "preset": self.style.preset,
                "intensity": self.style.intensity,
                "engine": self.style.engine,
            }
        if self.retouch:
            out["retouch"] = {"faces": self.retouch.faces, "strength": self.retouch.strength}
        if self.subject:
            out["subject"] = {"mode": self.subject.mode, "background": self.subject.background}
        if self.layout:
            out["layout"] = {
                "template": self.layout.template,
                "title": self.layout.title,
                "subtitle": self.layout.subtitle,
            }
        return out


# ------------------------------------------------------------------ PromptTemplate


@dataclass
class SlotSpec:
    type: str
    values: list[str] = field(default_factory=list)
    default: str = ""
    max_len: int = 0
    hint: str = ""

    @staticmethod
    def from_dict(d: dict, where: str) -> "SlotSpec":
        _only_keys(d, {"type", "values", "default", "maxLen", "hint"}, where)
        t = str(_need(d, "type", where))
        if t not in ("enum", "text"):
            raise RecipeError(f"{where}.type: 只支持 enum/text，得到 {t!r}")
        values = [str(v) for v in d.get("values", [])]
        if t == "enum" and not values:
            raise RecipeError(f"{where}.values: enum 槽位必须有候选值")
        return SlotSpec(
            type=t,
            values=values,
            default=str(d.get("default", values[0] if values else "")),
            max_len=int(d.get("maxLen", 0) or 0),
            hint=str(d.get("hint", "")),
        )


@dataclass
class PromptTemplate:
    id: str
    version: int
    engine: str
    anchor: str
    slots: dict[str, SlotSpec]
    skeleton: str
    negative: str = ""
    max_chars: int = 1200

    @staticmethod
    def from_dict(d: dict) -> "PromptTemplate":
        where = "promptTemplate"
        _only_keys(
            d, {"id", "version", "engine", "anchor", "slots", "skeleton", "negative", "maxChars"}, where
        )
        anchor = str(_need(d, "anchor", where)).strip()
        if not anchor:
            raise RecipeError(f"{where}.anchor: 不能为空（锚点缺失会让模型随意改构图）")
        slots_raw = _need(d, "slots", where)
        if not isinstance(slots_raw, dict) or not slots_raw:
            raise RecipeError(f"{where}.slots: 需要非空对象")
        skeleton = str(_need(d, "skeleton", where))
        slots = {k: SlotSpec.from_dict(v, f"{where}.slots.{k}") for k, v in slots_raw.items()}
        # 骨架里出现的占位符必须在 slots 里定义，反之亦然（否则渲染直接 KeyError）
        used = {name for name in _placeholders(skeleton)}
        defined = set(slots)
        if used - defined:
            raise RecipeError(f"{where}.skeleton: 占位符 {sorted(used - defined)} 未在 slots 定义")
        if defined - used:
            raise RecipeError(f"{where}.slots: {sorted(defined - used)} 未在 skeleton 中使用")
        return PromptTemplate(
            id=str(_need(d, "id", where)),
            version=int(_need(d, "version", where)),
            engine=_enum(_need(d, "engine", where), ENGINES, f"{where}.engine"),
            anchor=anchor,
            slots=slots,
            skeleton=skeleton,
            negative=str(d.get("negative", "")),
            max_chars=int(d.get("maxChars", 1200)),
        )


def _placeholders(skeleton: str) -> list[str]:
    """取出 `{xxx}` 形式的占位符名（跳过 `{{` 转义）。"""
    out: list[str] = []
    i = 0
    while i < len(skeleton):
        ch = skeleton[i]
        if ch == "{" and skeleton[i : i + 2] != "{{":
            end = skeleton.find("}", i)
            if end < 0:
                break
            name = skeleton[i + 1 : end].strip()
            if name:
                out.append(name)
            i = end + 1
            continue
        i += 1
    return out


# ------------------------------------------------------------------ Recipe


@dataclass
class Recipe:
    id: str
    group: str
    icon: str
    label: str
    desc: str
    status: str
    engine_behavior: str
    p1_path: str
    run_spec: RunSpec
    prompt_template: PromptTemplate | None

    @staticmethod
    def from_dict(d: dict) -> "Recipe":
        where = f"recipe[{d.get('id', '?')}]"
        _only_keys(
            d,
            {
                "id",
                "group",
                "icon",
                "label",
                "desc",
                "status",
                "engine_behavior",
                "p1_path",
                "runSpec",
                "promptTemplate",
            },
            where,
        )
        rid = str(_need(d, "id", where))
        if not rid.startswith(("style.", "retouch.", "card.", "batch.", "compress.", "custom")):
            raise RecipeError(
                f"{where}.id: 必须落在 recipe 命名空间（style./retouch./card./batch./compress./custom），得到 {rid!r}"
            )
        status = _enum(_need(d, "status", where), STATUSES, f"{where}.status")
        behavior = _enum(
            _need(d, "engine_behavior", where), ENGINE_BEHAVIORS, f"{where}.engine_behavior"
        )
        p1 = str(_need(d, "p1_path", where))
        if p1 not in ("P0", "P1a", "P1b", "P1c", "P2", "P3"):
            raise RecipeError(f"{where}.p1_path: 非法路径 {p1!r}")
        run_spec = RunSpec.from_dict(_need(d, "runSpec", where), f"{where}.runSpec")
        # promptTemplate 可以是内联对象（catalog 内联）或已解析好的 PromptTemplate（模板表解析后）
        tpl_raw = d.get("promptTemplate")
        if isinstance(tpl_raw, PromptTemplate):
            tpl: PromptTemplate | None = tpl_raw
        elif isinstance(tpl_raw, dict):
            tpl = PromptTemplate.from_dict(tpl_raw)
        else:
            tpl = None
        # 一致性：local_rule_mapped 的配方必须有 style（否则描述映射无处落地）
        if behavior == "local_rule_mapped" and not run_spec.style:
            raise RecipeError(f"{where}: engine_behavior=local_rule_mapped 但没有 runSpec.style")
        # 不消费提示词的配方，模板只能是备用规格（P1c 不该渲染 prompt）
        if behavior == "no_prompt" and tpl is not None:
            raise RecipeError(f"{where}: engine_behavior=no_prompt 不应携带 promptTemplate")
        return Recipe(
            id=rid,
            group=str(_need(d, "group", where)),
            icon=str(d.get("icon", "sparkle")),
            label=str(_need(d, "label", where)),
            desc=str(d.get("desc", "")),
            status=status,
            engine_behavior=behavior,
            p1_path=p1,
            run_spec=run_spec,
            prompt_template=tpl,
        )

    @property
    def needs_provider(self) -> bool:
        """跑这一份配方是否需要出网（决定 UI 是否能给出「开始」）。"""
        if self.run_spec.style and self.run_spec.style.engine == "remote":
            return True
        if self.run_spec.subject and self.run_spec.subject.mode == "extract":
            return True
        return False


# ------------------------------------------------------------------ 加载


def load_template(path: Path) -> PromptTemplate:
    return PromptTemplate.from_dict(json.loads(path.read_text(encoding="utf-8")))


def load_recipe(path: Path) -> Recipe:
    """单份配方文件（开发时可用；正式加载走 `load_registry`）。"""
    return _recipe_from_obj(json.loads(path.read_text(encoding="utf-8")), {})


def _recipe_from_obj(raw: dict, templates: dict[str, PromptTemplate]) -> Recipe:
    """把一份配方对象转成 Recipe。`promptTemplate` 可以是内联对象，也可以是模板 id。"""
    obj = dict(raw)
    tpl_ref = obj.get("promptTemplate")
    if isinstance(tpl_ref, str):
        tpl = templates.get(tpl_ref)
        if tpl is None:
            tpl_path = TEMPLATE_DIR / f"{tpl_ref}.json"
            if not tpl_path.is_file():
                raise RecipeError(f"recipe[{obj.get('id')}]: 引用不存在的模板 {tpl_ref!r}")
            tpl = load_template(tpl_path)
        obj["promptTemplate"] = tpl
    elif isinstance(tpl_ref, dict):
        obj["promptTemplate"] = PromptTemplate.from_dict(tpl_ref)
    return Recipe.from_dict(obj)


class RecipeRegistry:
    """全部配方的只读注册表。加载失败**大声失败**（启动即炸，不要静默少一个配方）。"""

    def __init__(self, recipes: list[Recipe]):
        self._by_id = {r.id: r for r in recipes}
        if len(self._by_id) != len(recipes):
            raise RecipeError("配方 id 重复")

    def get(self, recipe_id: str) -> Recipe:
        r = self._by_id.get(recipe_id)
        if r is None:
            raise RecipeError(f"未知配方 {recipe_id!r}（可用：{sorted(self._by_id)}）")
        return r

    def all(self) -> list[Recipe]:
        return list(self._by_id.values())


def load_registry(catalog_path: Path | None = None) -> RecipeRegistry:
    """从单一 catalog.json 加载全部配方。

    **为什么是单一文件而不是一个目录**：Rust 侧要能把同一份数据编进二进制
    （`include_str!`）直接给前端渲染下拉，而不需要先拉起侧车。
    单一文件 = 一份真相；拆成目录就必须靠 glob，Rust 那边又会多一张手维护的清单。
    """
    p = catalog_path or CATALOG_FILE
    if not p.is_file():
        raise RecipeError(f"配方目录文件不存在: {p}")
    raw = json.loads(p.read_text(encoding="utf-8"))
    templates = {
        name: PromptTemplate.from_dict(obj) for name, obj in (raw.get("templates") or {}).items()
    }
    recipes_raw = raw.get("recipes") or []
    if not recipes_raw:
        raise RecipeError(f"{p}: recipes 为空")
    return RecipeRegistry([_recipe_from_obj(obj, templates) for obj in recipes_raw])


_REGISTRY: RecipeRegistry | None = None


def registry() -> RecipeRegistry:
    """进程级单例（配方是只读数据，加载一次就够）。"""
    global _REGISTRY
    if _REGISTRY is None:
        _REGISTRY = load_registry()
    return _REGISTRY


def catalog_json() -> str:
    """原始 catalog JSON 文本（给 Rust/前端透传，不在 Python 侧重新序列化）。"""
    return CATALOG_FILE.read_text(encoding="utf-8")
