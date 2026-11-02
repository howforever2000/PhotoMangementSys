"""永不抛异常的诊断输出 + 敏感信息脱敏。

与 `vcr/logutil.py` 同风格，但多一个 `redact()`：Agent 侧会经手 API Key 与
用户原始描述，写日志前必须先过一遍（docs/v5.md §8）。

**纪律**（来自 BUG-2026-1101-001 的血账）：
凡「进度/诊断」类输出一律走 `err()/out()/exc()`；
凡「状态提交」一律在输出之前完成，绝不指望日志不抛异常。
"""
from __future__ import annotations

import hashlib
import re
import sys

# 需要整段打码的键名（大小写不敏感，含嵌套）
_SECRET_KEYS = re.compile(
    r"(api[_-]?key|authorization|token|secret|password|bearer|access[_-]?key)",
    re.IGNORECASE,
)

# 形似密钥的字面量（兜底：键名没暴露但值里就是 sk-xxx）
_SECRET_VALUE = re.compile(r"\b(sk-[A-Za-z0-9_\-]{6,}|gh[opsu]_[A-Za-z0-9]{10,})")

# 打码后只留头尾，便于"是不是同一把 key"的判断，又不泄露内容
_KEEP_HEAD = 6
_KEEP_TAIL = 4


def _write(stream, msg: str) -> None:
    try:
        stream.write(msg + "\n")
        stream.flush()
    except Exception:  # noqa: BLE001 —— 管道关闭/句柄失效/编码错误都不该影响业务
        pass


def err(msg: str) -> None:
    """写 stderr（诊断日志）。永不抛。"""
    _write(sys.stderr, msg)


def out(msg: str) -> None:
    """写 stdout。永不抛。"""
    _write(sys.stdout, msg)


def exc(prefix: str, e: BaseException) -> None:
    """写「前缀 + 异常类型 + 内容 + 栈」。永不抛。

    带栈是有意的：只留一行 `ValueError: xxx` 的现场，排障时往往要重跑一遍。
    """
    import traceback

    detail = "".join(traceback.format_exception(type(e), e, e.__traceback__))
    err(f"{prefix}: {type(e).__name__}: {e}\n{detail.rstrip()}")


def mask_secret(value: str) -> str:
    """密钥打码：`sk-abcdef123456` → `sk-abc…3456`。"""
    if not isinstance(value, str):
        return "***"
    s = value.strip()
    if not s:
        return ""
    if len(s) <= _KEEP_HEAD + _KEEP_TAIL:
        return "*" * len(s)
    return f"{s[:_KEEP_HEAD]}…{s[-_KEEP_TAIL:]}"


def prompt_digest(text: str) -> str:
    """提示词摘要：只留长度 + 短哈希，正文不进日志。"""
    s = text or ""
    h = hashlib.sha256(s.encode("utf-8", "replace")).hexdigest()[:8]
    return f"<len={len(s)} sha={h}>"


def redact(obj, _depth: int = 0):
    """递归脱敏：密钥键整段打码，字符串里的 `sk-` 字面量兜底替换。

    不修改入参（返回新对象），避免调用方以为还拿着原文。
    """
    if _depth > 8:  # 防御异常深的结构
        return "…"
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            if isinstance(k, str) and _SECRET_KEYS.search(k):
                out[k] = mask_secret(v) if isinstance(v, str) else "***"
            else:
                out[k] = redact(v, _depth + 1)
        return out
    if isinstance(obj, (list, tuple)):
        return [redact(v, _depth + 1) for v in obj]
    if isinstance(obj, str):
        return _SECRET_VALUE.sub(lambda m: mask_secret(m.group(0)), obj)
    return obj


def log_request(route: str, body) -> None:
    """请求体落日志的唯一入口：先脱敏，再把超长的 prompt 换成摘要。"""
    safe = redact(body)
    if isinstance(safe, dict):
        for k in ("prompt", "userMsg", "user_msg", "text"):
            v = safe.get(k)
            if isinstance(v, str) and len(v) > 60:
                safe[k] = prompt_digest(v)
    err(f"[req] {route} {safe}")
