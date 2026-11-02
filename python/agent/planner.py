"""规划层：让模型只做两件事 —— **填槽位**、**选下一步工具**。

## 三条硬边界（docs/v5.md §12、§14.2）

1. **模型只填槽位，不写骨架**。模板渲染是确定性的，锚点永远在最前；
   否则"用户改一个字，批量 12 张风格全变"。
2. **模型改不了执行规格**。它能提交的只有 `slots` 与工具参数，
   白名单之外的字段（`steps`、`scope`、`engine`…）一律丢弃。
3. **每次模型调用前过预算闸**。`maxCalls=0` 时这里根本进不来（budget.before 抛）。

## 两种用法

- `fill_slots()`：P1b —— 一次调用、强制走 `fill_slots` 工具，只填参不排管线；
- `run_loop()`：P2 —— 最多 5 步的工具循环，适合"完全自定义"这种意图不定的场景。
"""
from __future__ import annotations

import json

from . import logutil
from .budget import BudgetGuard
from .providers.base import ProviderError
from .providers.openai_compat import OpenAICompat, image_content_parts
from .recipes.schema import PromptTemplate, Recipe

MAX_STEPS = 5
MAX_VISION_IMAGES = 4  # 多模态额度合并计数（tray 4 张 + 参考图也只发 4 张）

SYSTEM_PROMPT = (
    "你是本地相册应用里的图像创作助手。你能看到用户手选的照片缩略图。\n"
    "规则：\n"
    "1. 只输出工具调用，不要解释过程。\n"
    "2. 不要自己编排执行管线；步骤由应用执行。\n"
    "3. 不要捏造照片内容：看不清就说看不清。\n"
    "4. 不要把人物姓名、联系方式、二维码写进任何参数。\n"
    "5. 图片里不要出现文字（应用会自己排版文字）。\n"
    "6. 拿不准时选更保守的参数（强度更低、页数更少）。"
)


class PlannerError(RuntimeError):
    """规划层失败（带可展示文案）。"""


# ------------------------------------------------------------------ 工具 schema


def fill_slots_schema(tpl: PromptTemplate) -> dict:
    """按模板的槽位**生成** schema：枚举值直接来自模板，模型无从"创造性扩展"。"""
    props: dict = {}
    required: list[str] = []
    for name, spec in tpl.slots.items():
        if spec.type == "enum":
            props[name] = {
                "type": "string",
                "enum": list(spec.values),
                "description": f"必须是这些值之一，默认 {spec.default}",
            }
        else:
            props[name] = {
                "type": "string",
                "description": (spec.hint or "") + (f"（最多 {spec.max_len} 字）" if spec.max_len else ""),
            }
        required.append(name)
    return {
        "type": "function",
        "function": {
            "name": "fill_slots",
            "description": "根据用户描述填写提示词模板的槽位。不要改写模板本身。",
            "parameters": {
                "type": "object",
                "properties": props,
                "required": required,
            },
        },
    }


TOOL_SCHEMAS: list[dict] = [
    {
        "type": "function",
        "function": {
            "name": "style_transfer",
            "description": "对选中的照片按一段目标提示词做重绘/风格化（会调用图像模型，按张计费）。",
            "parameters": {
                "type": "object",
                "properties": {
                    "look": {"type": "string", "description": "风格代号，如 warm_dusk / film_kodak / clean_air"},
                    "intensity": {"type": "string", "enum": ["subtle", "moderate", "strong"]},
                    "note": {"type": "string", "description": "补充描述，最多 120 字"},
                },
                "required": ["look"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "compress",
            "description": "把照片压到目标体积（不调用模型，零成本）。",
            "parameters": {
                "type": "object",
                "properties": {
                    "targetKb": {"type": "integer", "description": "目标体积 KB，20~20000"},
                    "maxEdge": {"type": "integer", "description": "长边上限像素"},
                },
                "required": ["targetKb"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "card_render",
            "description": "把照片排成卡片并导出 PDF（不调用模型，零成本）。",
            "parameters": {
                "type": "object",
                "properties": {
                    "template": {"type": "string", "enum": ["magazine_3up", "grid_2up"]},
                    "title": {"type": "string"},
                    "subtitle": {"type": "string"},
                },
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "finish",
            "description": "任务完成，返回一句给用户看的结果说明。",
            "parameters": {
                "type": "object",
                "properties": {"message": {"type": "string", "description": "一句话总结，别超过 60 字"}},
                "required": ["message"],
            },
        },
    },
]


# ------------------------------------------------------------------ 规划器


class Planner:
    """把"模型能力"收在这里：调用次数、预算、图片额度都从这一个口过。"""

    def __init__(self, client: OpenAICompat, guard: BudgetGuard):
        self.client = client
        self.guard = guard

    # ---- P1b：只填参 ----

    async def fill_slots(
        self,
        recipe: Recipe,
        tpl: PromptTemplate,
        user_text: str,
        thumbs: list[bytes],
    ) -> tuple[dict, list[str]]:
        """一次调用填完槽位。返回 (slots, 假设)。"""
        schema = fill_slots_schema(tpl)
        content: list[dict] = [{"type": "text", "text": user_text.strip() or "（未填写描述）"}]
        content.extend(image_content_parts(thumbs[:MAX_VISION_IMAGES]))

        messages = [
            {"role": "system", "content": SYSTEM_PROMPT},
            {
                "role": "user",
                "content": content
                if thumbs
                else user_text.strip() or "（未填写描述）",
            },
        ]
        reserved = self.guard.before("planner")
        try:
            resp = await self.client.chat(
                messages,
                tools=[schema],
                tool_choice={"type": "function", "function": {"name": "fill_slots"}},
                max_tokens=400,
            )
        except ProviderError as e:
            raise PlannerError(f"规划模型调用失败：{e}") from e
        self.guard.settle(reserved, _usage_cost(resp))
        logutil.err(
            f"[planner] fill_slots recipe={recipe.id} 图片={min(len(thumbs), MAX_VISION_IMAGES)}"
        )

        args = _first_tool_args(resp, "fill_slots")
        if args is None:
            raise PlannerError("规划模型没有返回 fill_slots 调用（可能是端点不支持工具调用）")
        assumptions = [f"槽位由模型填写（{self.client.ep.model}）"]
        return args, assumptions

    # ---- P2：工具循环 ----

    async def run_loop(
        self,
        user_text: str,
        thumbs: list[bytes],
        dispatch,
        *,
        max_steps: int = MAX_STEPS,
    ) -> dict:
        """最多 max_steps 步。`dispatch(name, args)` 由调用方提供（runtime）。"""
        content: list[dict] = [{"type": "text", "text": user_text.strip() or "（未填写描述）"}]
        content.extend(image_content_parts(thumbs[:MAX_VISION_IMAGES]))
        messages: list[dict] = [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": content},
        ]

        used: list[dict] = []
        for step in range(max_steps):
            reserved = self.guard.before("planner")
            try:
                resp = await self.client.chat(messages, tools=TOOL_SCHEMAS, tool_choice="auto")
            except ProviderError as e:
                raise PlannerError(f"规划模型调用失败：{e}") from e
            self.guard.settle(reserved, _usage_cost(resp))

            msg = _first_message(resp)
            calls = msg.get("tool_calls") or []
            if not calls:
                text = (msg.get("content") or "").strip()
                return {"status": "done", "text": text or "完成", "steps": used}

            messages.append({"role": "assistant", "tool_calls": calls, "content": msg.get("content")})

            for call in calls:
                fn = (call.get("function") or {})
                name = str(fn.get("name") or "")
                args = _json_or_empty(fn.get("arguments"))
                if name == "finish":
                    return {
                        "status": "done",
                        "text": str(args.get("message") or "完成"),
                        "steps": used,
                    }
                result = await dispatch(name, args)
                used.append({"tool": name, "args": args, "ok": bool(result.get("ok"))})
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": str(call.get("id") or ""),
                        "content": json.dumps(_shrink(result), ensure_ascii=False, default=str),
                    }
                )
        return {"status": "max_steps", "text": "任务达到最大步数", "steps": used}


# ------------------------------------------------------------------ 工具函数


def _first_message(resp: dict) -> dict:
    try:
        return resp["choices"][0]["message"]
    except (KeyError, IndexError, TypeError) as e:
        raise PlannerError(f"规划模型返回体不符合 chat.completions 契约：{str(resp)[:200]}") from e


def _first_tool_args(resp: dict, name: str) -> dict | None:
    msg = _first_message(resp)
    for call in msg.get("tool_calls") or []:
        fn = call.get("function") or {}
        if fn.get("name") == name:
            return _json_or_empty(fn.get("arguments"))
    return None


def _json_or_empty(raw) -> dict:
    if isinstance(raw, dict):
        return raw
    if not raw:
        return {}
    try:
        v = json.loads(raw)
        return v if isinstance(v, dict) else {}
    except (ValueError, TypeError):
        return {}


def _usage_cost(resp: dict) -> float | None:
    """有些端点会在 usage 之外给 cost；没有就返回 None（保留预占价）。"""
    for key in ("cost", "total_cost"):
        if key in resp:
            try:
                return float(resp[key])
            except (TypeError, ValueError):
                return None
    usage = resp.get("usage") or {}
    if isinstance(usage, dict) and "cost" in usage:
        try:
            return float(usage["cost"])
        except (TypeError, ValueError):
            return None
    return None


def _shrink(result: dict) -> dict:
    """工具结果回灌给模型前裁掉大字段：`out` 路径、缩略图路径对它没有意义，
    只会白烧 token。"""
    keep = {"ok", "error", "failures", "note", "count", "pageCount", "pdf", "cost"}
    small = {k: v for k, v in result.items() if k in keep}
    items = result.get("items")
    if isinstance(items, list):
        small["items"] = [
            {k: it.get(k) for k in ("src", "out", "bytes", "kb", "w", "h", "quality", "ok") if k in it}
            for it in items[:3]
        ]
        small["itemCount"] = len(items)
    return small
