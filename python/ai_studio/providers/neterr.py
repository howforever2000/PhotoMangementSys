"""网络层失败 → 可操作的中文提示（BUG-2026-1122-007）

`requests` 会把 urllib3 的失败包成很长的一句 `ConnectionError`：:

    HTTPConnectionPool(host='127.0.0.1', port=8000): Max retries exceeded with url:
    /v1/images/generations (Caused by NewConnectionError(... [WinError 10061] ...))

原样回显只说明「失败了」，不说明「该怎么办」。本模块按 WinError / socket 关键字把
连接层失败分成用户能分别动手的几类：端口没人监听、连接被中途重置、超时、域名解析
失败、TLS 握手失败。

**为什么要区分 10061 与 10054**（这正是本模块存在的原因）：
  - 10061 = 连接被拒绝 = 端口上没有服务在监听 → 该做的是「启动服务 / 改 baseUrl」；
  - 10054 = 有服务在听但中途掐断 → 该做的是「减小体量 / 换端点」。
历史上两者被同一句「上传图片过大（10054）」概括，于是「本地网关没启动」被误报成
「图片太大」，用户按提示去压分辨率永远修不好。

纯字符串判定：不发请求、不依赖 `requests` 的异常类型，可直接单测。
"""
from __future__ import annotations

import re
from typing import Literal
from urllib.parse import urlsplit

NetKind = Literal["refused", "reset", "timeout", "dns", "tls", "other"]

# 顺序即优先级：先判「没人听」与「被掐断」，再判超时与地址问题。
# 注意 `Connection aborted`（http.client 对 RST 的另一种说法）要归到 reset，
# 别落到 fallback 里丢掉分类。
_REFUSED = re.compile(r"10061|actively refused|connection refused|ConnectionRefused|积极拒绝", re.I)
_RESET = re.compile(r"10054|reset by peer|connection aborted|RemoteDisconnected|强迫关闭|ConnectionReset", re.I)
_TIMEOUT = re.compile(r"10060|timed out|timeout|ReadTimeout|ConnectTimeout", re.I)
_DNS = re.compile(r"getaddrinfo|name or service not known|11001|nodename nor servname|name resolution", re.I)
_TLS = re.compile(r"SSLError|CERTIFICATE_VERIFY_FAILED|certificate", re.I)

_RAW_LIMIT = 200


def endpoint_of(base: str) -> str:
    """把 baseUrl 收成 `host:port`，用于文案里点名「到底连的哪儿」。

    只回显主机与端口（不回显路径与 key）；解析不出主机时原样回显去掉斜杠的地址，
    绝不返回空串 —— 文案里出现「无法连接 ：」比不显示更难排查。
    """
    raw = (base or "").strip()
    if not raw:
        return "<未填 baseUrl>"
    try:
        parts = urlsplit(raw)
    except ValueError:
        return raw
    host = parts.hostname
    if not host:
        return raw.rstrip("/")
    if parts.port:
        return f"{host}:{parts.port}"
    if parts.scheme == "https":
        return f"{host}:443"
    if parts.scheme == "http":
        return f"{host}:80"
    return host


def kind_of(exc: BaseException | None) -> NetKind:
    """把连接层异常归类（不抛异常，供调用方决定「继续」还是「立刻失败」）。"""
    if exc is None:
        return "other"
    text = f"{type(exc).__name__}: {exc}"
    if _REFUSED.search(text):
        return "refused"
    if _RESET.search(text):
        return "reset"
    if _TIMEOUT.search(text):
        return "timeout"
    if _DNS.search(text):
        return "dns"
    if _TLS.search(text):
        return "tls"
    return "other"


def describe(base: str, exc: BaseException) -> str:
    """一条「连不上 + 下一步做什么」的中文说明（同一原因永远同一句话）。"""
    ep = endpoint_of(base)
    kind = kind_of(exc)
    if kind == "refused":
        return (
            f"无法连接 {ep}：该端口没有服务在监听（WinError 10061 连接被拒绝）。"
            f"本地服务可能没启动，或 baseUrl 填错了 —— 先确认 {ep} 上有服务在跑。"
        )
    if kind == "reset":
        return (
            f"{ep} 的连接被中途重置（10054）：可能是上传体过大被网关 / 代理掐断，"
            f"也可能是该端点不接当前请求。可减小输入分辨率，或换一个 baseUrl。"
        )
    if kind == "timeout":
        return (
            f"访问 {ep} 超时：服务没有在限定时间内响应。"
            f"本地出图慢时属正常，长期超时请检查地址与网络。"
        )
    if kind == "dns":
        return f"域名解析失败（{ep}）：baseUrl 可能拼错，或当前网络需要代理。"
    if kind == "tls":
        return f"TLS 握手失败（{ep}）：代理证书或 https 配置有问题。"
    return f"访问 {ep} 失败：{str(exc)[:_RAW_LIMIT]}"


__all__ = ["NetKind", "endpoint_of", "kind_of", "describe"]
