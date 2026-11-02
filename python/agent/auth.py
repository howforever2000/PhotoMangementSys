"""一次性 token 校验（per-process，只守回环端口）。

为什么用环境变量而不是配置文件：token 由 Rust 在拉起子进程时通过 `AGENT_TOKEN`
注入，Python 侧不落盘、不缓存到下一次 job（docs/v5.md §8 第 2 条）。

为什么没注入就 503 而不是放行：放行等于把 `/agent/submit` 变成任何本机进程
都能调用、且请求体里带 provider 明文 key 的接口。宁可大声失败。
"""
from __future__ import annotations

import hmac
import os

from fastapi import Header, HTTPException

TOKEN_ENV = "AGENT_TOKEN"


def expected_token() -> str:
    """当前进程应有的 token（空串表示宿主没注入）。"""
    return (os.environ.get(TOKEN_ENV) or "").strip()


def require_token(x_agent_token: str | None = Header(default=None, alias="X-Agent-Token")) -> None:
    """FastAPI 依赖：`X-Agent-Token` 必须与 `AGENT_TOKEN` 常量时间相等。"""
    want = expected_token()
    if not want:
        raise HTTPException(status_code=503, detail="AGENT_TOKEN 未注入（该服务必须由宿主拉起）")
    if not x_agent_token or not hmac.compare_digest(x_agent_token, want):
        raise HTTPException(status_code=401, detail="X-Agent-Token 不匹配")
