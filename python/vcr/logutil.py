"""永不抛异常的输出工具。

## 为什么需要它（BUG-2026-1101-001 的直接产物）

打包版由宿主用 `Stdio::piped()` 接管子进程的 stdout/stderr（`vision.rs::attach_child_output`），
再由 Rust 线程读走写进 app.log。这个管道**会在服务重启/宿主回收时关闭**，
而 Python 侧此时往 `sys.stderr` 写一行日志就会抛：

    OSError: [Errno 22] Invalid argument

问题不在"日志丢了"，而在**日志写在关键路径里**：FEAT-133 的
`model_registry._ensure_ov_clip()` 曾经在「会话已建好、状态已提交」之后才
`print(..., file=sys.stderr)`，那一行抛异常被外层 `except Exception` 接住，
于是成功的加载被改写成失败：

    active=true（_ov_clip 已赋值）  ←→  _ready=False / load_errors 有内容
    ⇒ /health.clip_ready=false ⇒ 宿主跳过语义入库 ⇒ NPU 全程 0 占用

**纪律**：凡「进度/诊断」类输出，一律走本模块的 `err()/out()`；
凡「状态提交」，一律在输出**之前**完成或与输出完全解耦。
绝不要写 `print(..., file=sys.stderr)` 然后指望它不抛。
"""
from __future__ import annotations

import sys


def _write(stream, msg: str) -> None:
    try:
        stream.write(msg + "\n")
        stream.flush()
    except Exception:  # noqa: BLE001  —— 管道关了、句柄失效、编码错误，都不该影响业务
        pass


def err(msg: str) -> None:
    """写 stderr（诊断日志）。永不抛。"""
    _write(sys.stderr, msg)


def out(msg: str) -> None:
    """写 stdout。永不抛。"""
    _write(sys.stdout, msg)


def exc(prefix: str, e: BaseException) -> None:
    """写「前缀 + 异常类型 + 内容 + 栈」。永不抛。

    用于把被接住的异常留下可排查的现场 —— FEAT-133 那次就是因为只留了
    `OSError: [Errno 22]` 一行、没有栈，才多花了一轮排查。
    """
    import traceback

    detail = "".join(traceback.format_exception(type(e), e, e.__traceback__))
    err(f"{prefix}: {type(e).__name__}: {e}\n{detail.rstrip()}")
