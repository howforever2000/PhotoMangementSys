"""校验器：任何 P1/P2/P3 输出在落地前都要过这里（docs/v5.md §16）。

这些规则不是"锦上添花"，每一条都对应过具体问题：
- **禁用词**：用户写「带上我的微信二维码」，模型会真的把二维码画进图里；
- **隐私**：人名只用于分组/挑图，写进提示词等于把身份信息发到第三方端点；
- **锚点**：模型改写过度会丢掉构图约束，同批次风格立刻不一致；
- **白名单**：模型输出 `steps: [...]` 想自己排管线 —— 直接丢弃，不执行；
- **scope 冻结**：P1 悄悄把 8 张扩成 24 张，账单从 ¥0.15 变 ¥0.45。
"""
from __future__ import annotations

import re

# 命中即剔除（中英都列全：用户可能中英混写，模型也可能回英文）
FORBIDDEN_TERMS = (
    "watermark",
    "logo",
    "signature",
    "qrcode",
    "qr code",
    "wechat",
    "水印",
    "签名",
    "微信号",
    "微信",
    "二维码",
    "联系方式",
    "电话号码",
    "公众号",
)

# PlanPatch 白名单（§14.2）：不在表里的一律丢弃
PLAN_PATCH_KEYS = {
    "deliverable",
    "scope",
    "style",
    "retouch",
    "subject",
    "layout",
    "export",
    "slots",
}

# runSpecOverride 允许前端覆盖的字段（用户通过 UI 控件改，不通过写文字）
OVERRIDE_PATHS = {
    "style": {"intensity"},
    "retouch": {"strength", "faces"},
    "export": {"targetKb", "maxEdge", "crop"},
    "layout": {"title", "subtitle", "template"},
}

INTENSITY_CANON = {"subtle": 0.3, "moderate": 0.6, "strong": 0.9}


class ValidationError(ValueError):
    """校验失败（调用方决定是回灌自修还是丢弃 patch）。"""


def intensity_level(value: float) -> str:
    """数值 → 三档。渲染与 UI 都按这三档说话（subtle/moderate/strong）。"""
    v = float(value or 0.0)
    if v < 0.45:
        return "subtle"
    if v < 0.78:
        return "moderate"
    return "strong"


def intensity_value(level: str) -> float:
    return INTENSITY_CANON.get(level, 0.6)


def strip_forbidden(text: str) -> tuple[str, list[str]]:
    """剔除禁用词（整词/子串都算），返回 (清理后文本, 命中列表)。"""
    if not text:
        return text, []
    hits: list[str] = []
    out = text
    for term in FORBIDDEN_TERMS:
        if term.lower() in out.lower():
            hits.append(term)
            # 大小写不敏感替换：用正则 + IGNORECASE，替换成空
            out = re.sub(re.escape(term), "", out, flags=re.IGNORECASE)
    if hits:
        # 清理掉因删除产生的多余空格与标点
        out = re.sub(r"\s{2,}", " ", out)
        out = re.sub(r"\s+([,.;:，。；：])", r"\1", out)
        out = out.strip()
    return out, hits


def strip_person_names(text: str, names: list[str]) -> tuple[str, list[str]]:
    """把人物真名换成 `the person`（人名只用于分组/挑图，不进提示词）。"""
    if not text or not names:
        return text, []
    hits: list[str] = []
    out = text
    for name in names:
        n = (name or "").strip()
        if len(n) < 2:  # 单字名替换风险太高（会误伤普通词）
            continue
        if n in out:
            hits.append(n)
            out = out.replace(n, "the person")
    return out, hits


def ensure_anchor(prompt: str, anchor: str) -> tuple[str, bool]:
    """锚点必须出现在最终文本开头。缺了就补回（返回 (文本, 是否补回))。"""
    a = (anchor or "").strip()
    p = (prompt or "").strip()
    if not a:
        return p, False
    if p.startswith(a):
        return p, False
    # 文本里已经含有锚点首句但不在开头：把整段锚点提到最前，去掉重复出现的那次
    first_sentence = a.split(".")[0].strip()
    body = p
    if first_sentence and first_sentence in body:
        body = body.replace(first_sentence, "", 1).strip()
        body = body.lstrip(".,;，。；:： ").strip()
    return (f"{a}\n\n{body}".strip() if body else a), True


def filter_plan_patch(patch: dict) -> tuple[dict, list[str]]:
    """白名单过滤 PlanPatch。返回 (过滤后 patch, 被丢弃的键)。"""
    if not isinstance(patch, dict):
        raise ValidationError("plan_patch 必须是对象")
    dropped = sorted(set(patch) - PLAN_PATCH_KEYS)
    return {k: v for k, v in patch.items() if k in PLAN_PATCH_KEYS}, dropped


def check_run_spec_override(override: dict | None) -> dict:
    """校验前端传入的 runSpec 覆盖：只允许白名单字段，值域也校验。

    为什么不让前端随便写 runSpec：这条通道等于绕过配方规格，
    一个笔误就能让"零成本路径"变成"远程出网"。
    """
    if not override:
        return {}
    if not isinstance(override, dict):
        raise ValidationError("runSpecOverride 必须是对象")
    clean: dict = {}
    for section, fields in override.items():
        allowed = OVERRIDE_PATHS.get(section)
        if allowed is None:
            raise ValidationError(f"runSpecOverride.{section} 不允许覆盖")
        if not isinstance(fields, dict):
            raise ValidationError(f"runSpecOverride.{section} 必须是对象")
        bad = set(fields) - allowed
        if bad:
            raise ValidationError(f"runSpecOverride.{section} 不允许改 {sorted(bad)}")
        clean[section] = dict(fields)

    # 值域
    if "style" in clean and "intensity" in clean["style"]:
        try:
            v = float(clean["style"]["intensity"])
        except (TypeError, ValueError) as e:
            raise ValidationError("intensity 需要数值") from e
        if not 0.0 <= v <= 1.0:
            raise ValidationError("intensity 应在 0~1")
        clean["style"]["intensity"] = v
    if "retouch" in clean and "strength" in clean["retouch"]:
        if clean["retouch"]["strength"] not in ("natural", "mild", "strong"):
            raise ValidationError("retouch.strength 只支持 natural/mild/strong")
    if "export" in clean:
        exp = clean["export"]
        if "targetKb" in exp:
            kb = int(exp["targetKb"])
            if not 20 <= kb <= 20000:
                raise ValidationError("targetKb 应在 20~20000")
            exp["targetKb"] = kb
        if "maxEdge" in exp:
            edge = int(exp["maxEdge"])
            if not 240 <= edge <= 20000:
                raise ValidationError("maxEdge 应在 240~20000")
            exp["maxEdge"] = edge
        if "crop" in exp and exp["crop"] not in ("", "1:1", "4:3", "3:2", "16:9", "3:4"):
            raise ValidationError("crop 只支持空串或 1:1 / 4:3 / 3:2 / 16:9 / 3:4")
    return clean


def truncate_slots(slots: dict, max_chars: int, render_fn) -> tuple[dict, list[str]]:
    """渲染超长时的降级链：先截 note，再降 intensity 一档（§14.3 第 3 条）。"""
    assumptions: list[str] = []
    work = dict(slots)

    note = work.get("note")
    if isinstance(note, str) and len(render_fn(work)) > max_chars and note:
        work["note"] = note[: max(0, len(note) // 2)].rstrip()
        assumptions.append("提示词超长：已截断补充描述")

    level = work.get("intensity")
    order = ["strong", "moderate", "subtle"]
    while len(render_fn(work)) > max_chars and level in order:
        idx = order.index(level)
        if idx + 1 >= len(order):
            break
        level = order[idx + 1]
        work["intensity"] = level
        assumptions.append(f"提示词超长：强度已降为 {level}")
    return work, assumptions
