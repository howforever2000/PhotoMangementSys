"""模型风格提示词（FEAT-198）：`model_styles.json` 的载入与拼接。

## 为什么单独一个模块

风格提示词是**数据**（用户提供的 6 段影调描述），不是流程；它的消费者有三个：
规划器提示词（让模型知道有哪些风格可选）、图像编辑提示词（真正生效的那一处）、
前端 chips（只镜像 id/label）。把「载入 + 拼接」收成纯函数，三个入口才不会
各自拼一份、慢慢漂移。

## 关键设计：风格段进「执行」而不是「规划」

「模型理解」链路的最终出图提示词由算子生成（`ops.op_style_transfer` →
`presets.auto_grade_prompt(extra)`），**用户意图只进了规划器**。所以风格段如果
只塞进 textarea 当意图，效果取决于规划器有没有把它抄进 `extra` —— 抄漏就静默
失效。因此：

  - 风格段由 `with_style()` 在**算子层确定性地追加**（`extra` 的尾部）；
  - 规划器提示词里也列一份可选风格清单，让模型知道「用户要的是这种影调」，
    便于它排出正确的步骤（例如该用 `style_transfer` 而不是 `beauty`）。

## 为什么中文原文与英文要点都发

用户要的是截图那条中文描述（Q2=c：中文原文 + 英文要点双段）。中文段保留原始
影调语汇（「透亮」「死黑」「光晕」这类词对中文多模态模型有效），英文段给网关
一个结构化的稳定锚点。两段由 `compose()` 拼接，顺序固定：中文在前、英文在后。

## 品牌名

胶片型号名（柯达 / 富士 / CineStill / Portra / Pro 160NS / 800T）按用户裁定保留
（Q3）。这与 FEAT-170「手账不点名第三方 IP」不冲突：那条例针对的是**动画工作室名**
（网关会随机拒答），胶片型号是真实产品规格，且这类影调描述普遍被接受。
"""
from __future__ import annotations

import json
import os
import sys
from functools import lru_cache
from typing import Any, Dict, List

_FILE = "model_styles.json"

# 上限：一段风格描述不该长到吞掉用户意图，也不该超出网关对 prompt 的容忍。
# 实测最长一条（柯达 Portra 400，中文原文 + 英文要点）约 1227 字，所以取 2000 ——
# 这是**防数据写错**的闸，不是「网关只收 500 字」那种硬限制（真正的 500 字闸在
# `catalog.norm_params` 的 string 类型上，也正是风格段不走 `params.extra` 的原因）。
MAX_STYLE_CHARS = 2000


def _path() -> str:
    """开发态 = 包目录；PyInstaller 单文件模式 = _MEIPASS（与 catalog / recipes 同口径）。"""
    here = os.path.dirname(os.path.abspath(__file__))
    candidates = [
        os.path.join(here, _FILE),
        os.path.join(getattr(sys, "_MEIPASS", ""), "ai_studio", _FILE),
    ]
    for p in candidates:
        if os.path.isfile(p):
            return p
    return candidates[0]


@lru_cache(maxsize=1)
def _load() -> Dict[str, Any]:
    with open(_path(), "r", encoding="utf-8") as fh:
        data = json.load(fh)
    styles = data.get("styles")
    if not isinstance(styles, list) or not styles:
        raise ValueError("model_styles.json 缺少 styles 数组")
    ids = [s.get("id") for s in styles]
    if any(not isinstance(x, str) or not x for x in ids):
        raise ValueError("风格存在空 id")
    if len(ids) != len(set(ids)):
        raise ValueError("风格 id 重复")
    for s in styles:
        for field in ("label", "zh", "en"):
            if not str(s.get(field) or "").strip():
                raise ValueError(f"风格 {s['id']} 的 {field} 为空")
        total = len(s["zh"]) + len(s["en"])
        if total > MAX_STYLE_CHARS:
            raise ValueError(f"风格 {s['id']} 描述过长（{total} > {MAX_STYLE_CHARS}）")
    return data


def styles() -> List[Dict[str, str]]:
    """全部风格（保持声明顺序，前端 chips 与规划提示词都用这个顺序）。"""
    return list(_load()["styles"])


def style_ids() -> tuple[str, ...]:
    return tuple(s["id"] for s in styles())


def find(style_id: str) -> Dict[str, str] | None:
    """按 id 找一条；未知 id 返回 None（由调用方决定是告警还是忽略）。"""
    key = str(style_id or "").strip()
    return next((s for s in styles() if s["id"] == key), None)


def compose(style_id: str) -> str:
    """风格 id → 完整段落（中文原文 + 英文要点）。未知 id 返回空串。

    返回空串而不是抛错：风格是**可选增强**，用户选了一个在新版本里被删掉的 id
    时，应该照常出图（没有风格），而不是整单失败。
    """
    zh = compose_zh(style_id)
    en = compose_en(style_id)
    if not zh:
        return ""
    return f"画面风格参考（保持构图与物体形状不变）：{zh}\nStyle reference: {en}"


def compose_zh(style_id: str) -> str:
    """中文原文（前端会把它追加进可见的输入框，用户可编辑）。"""
    s = find(style_id)
    return s["zh"] if s else ""


def compose_en(style_id: str) -> str:
    """英文要点（后端拼进真正发给网关的提示词，用户看不到也不需要看到）。

    为什么中文与英文分两条通道：中文原文按 Q4(a) 要**可见可编辑**（前端写进输入框），
    英文要点是给网关的结构化锚点（后端补）。若两边都发中文，同一段描述会进提示词
    两次 —— 长提示词里重复描述会把权重推高，出图比预期更过火。
    """
    s = find(style_id)
    return s["en"] if s else ""


def en_block(style_id: str) -> str:
    """要由 `RunContext` 追加的段落（只有英文要点）。"""
    en = compose_en(style_id)
    return f"Film/color style reference: {en}" if en else ""


def label_of(style_id: str) -> str:
    s = find(style_id)
    return s["label"] if s else ""


def prompt_lines() -> str:
    """规划提示词里的可选风格清单（一行一款，只给 id + 中文名）。

    为什么不把整段描述塞进规划提示词：那段描述是**给图像模型的影调指令**，规划器
    只需要知道「用户选了哪一款」，把 6 段全文塞进去只会占用上下文、削弱意图。
    """
    return "\n".join(f"- {s['id']}：{s['label']}" for s in styles())


__all__ = [
    "MAX_STYLE_CHARS",
    "styles",
    "style_ids",
    "find",
    "compose",
    "compose_zh",
    "compose_en",
    "en_block",
    "label_of",
    "prompt_lines",
]
