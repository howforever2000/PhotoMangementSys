"""Agent 侧车入口（uvicorn，端口与 token 由宿主注入）。

宿主（Rust `sidecar.rs`）拉起本进程时注入：
    AGENT_PORT   必填。宿主每次向 0 号端口要一个空闲端口后传入
    AGENT_TOKEN  必填。per-process 一次性 token，`X-Agent-Token` 校验用
    AGENT_LOG    可选。仅用于在宿主未接管管道时自证（正常由 Rust 落盘）

**没有 token 就不启动**：缺 token 时服务等于对全机开放、且请求体里带 provider
明文 key（docs/v5.md §8）。这里刻意不做"开发模式免 token"的后门 ——
多一条密钥路径就多一处会漏的地方。
"""
from __future__ import annotations

import os
import sys

import uvicorn

from agent import logutil


def _die(msg: str) -> None:
    logutil.err(f"[fatal] {msg}")
    raise SystemExit(2)


def main() -> None:
    token = (os.environ.get("AGENT_TOKEN") or "").strip()
    if not token:
        _die("AGENT_TOKEN 未注入：本服务必须由宿主拉起（见 agent/auth.py 注释）")

    raw_port = (os.environ.get("AGENT_PORT") or "").strip()
    if not raw_port:
        # 手动排障时允许不传端口，但默认端口固定，便于 `curl 127.0.0.1:8791/health`
        port = 8791
        logutil.err("[warn] AGENT_PORT 未注入，回落到 8791（仅用于手动排障）")
    else:
        try:
            port = int(raw_port)
        except ValueError:
            _die(f"AGENT_PORT 不是合法端口: {raw_port!r}")
            return
        if not (1 <= port <= 65535):
            _die(f"AGENT_PORT 越界: {port}")

    logutil.err(f"[boot] pm-agent pid={os.getpid()} port={port} py={sys.version.split()[0]}")

    # access_log=False：宿主每 400ms 轮询一次，access log 只会把日志刷成噪声
    uvicorn.run(
        "agent.app:app",
        host="127.0.0.1",
        port=port,
        log_level="warning",
        access_log=False,
    )


if __name__ == "__main__":
    main()
