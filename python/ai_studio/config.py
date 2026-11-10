"""AI 创作微服务配置（FEAT-152）

服务是**无状态**的：模型配置（baseUrl/apiKey/model）由 Rust 在每次作业请求里附带，
Python 侧绝不落盘 api_key。这里只放进程级常量与端口解析。
"""
import os

# /health 里回给 Rust 的协议版本：Rust 侧据此判定服务是否需要重启（与 VCR 的 api_version 同思路）
API_VERSION = 1
SERVICE_NAME = "ai-studio"

# 单作业并发上限：图像编辑多为网络等待，2 路并发足够，过多会触发服务商限流
JOB_WORKERS = 2

# 单次 provider 请求超时（秒）：图像编辑可能较慢，给足
REQUEST_TIMEOUT = 180
# 连通性探测超时（/models）
PROBE_TIMEOUT = 20


def port() -> int:
    """服务端口：由 Rust 拉进程时通过 AI_STUDIO_PORT 注入（动态分配，避免固定端口冲突）。"""
    raw = os.environ.get("AI_STUDIO_PORT", "0") or "0"
    try:
        return int(raw)
    except ValueError:
        return 0
