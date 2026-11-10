"""极简日志（FEAT-152）

与 VCR 的 `[vcr.stat]` 一致：直接 print 到 stdout，由 Rust 侧把子进程输出转存到
app_data_dir/ai_studio_service.log。刻意不引入 logging 配置，保持零副作用。
"""
import time

_T0 = time.time()


def log(msg: str) -> None:
    try:
        print(f"[ai-studio] {msg}", flush=True)
    except Exception:
        pass


def elog(msg: str) -> None:
    """错误日志（同样走 stdout，前缀区分，便于 grep）。"""
    try:
        print(f"[ai-studio][ERR] {msg}", flush=True)
    except Exception:
        pass
