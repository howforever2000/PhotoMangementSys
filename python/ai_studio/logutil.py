"""极简日志（FEAT-152）

与 VCR 的 `[vcr.stat]` 一致：直接 print 到 stdout，由 Rust 侧把子进程输出转存到
app_data_dir/ai_studio_service.log。刻意不引入 logging 配置，保持零副作用。

FEAT-178 细粒度日志：新增 `call_model` 上下文管理器，统一记录「模型调用 + 耗时 +
成功/失败」，落点与本模块既有 log/elog 完全一致（`[ai-studio][MODEL]` 前缀）。
"""
import time
from contextlib import contextmanager

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


@contextmanager
def call_model(kind: str, name: str, detail: str = ""):
    """记录一次模型/工具调用的耗时与结果（FEAT-178）。

    - `kind`：模型类别（onnx / cloud / tool），便于 grep 分类；
    - `name`：模型或工具名（本地用文件名，云端用 provider.model）；
    - `detail`：可选上下文（如 tile 数、人脸数、输出尺寸）。

    用法::

        with call_model("onnx", "real_esrgan_x4.onnx", f"tile={t}"):
            out = sess.run(...)

    正常退出打印 `[ai-studio][MODEL] ok ... | 耗时 Xms`；抛异常时打印
    `[ai-studio][ERR][MODEL] fail ... | 耗时 Xms | 原因` 并继续抛出（不吞异常）。
    与 log/elog 一样零副作用：打印失败绝不影响业务。
    """
    start = time.perf_counter()
    tag = f"{name}{' | ' + detail if detail else ''}"
    ok = True
    err = ""
    try:
        yield
    except Exception as e:  # noqa: BLE001 —— 只记日志，异常原样抛给调用方
        ok = False
        err = str(e)
        raise
    finally:
        ms = int((time.perf_counter() - start) * 1000)
        if ok:
            log(f"[MODEL] ok kind={kind} name={tag} | 耗时 {ms}ms")
        else:
            elog(f"[MODEL] fail kind={kind} name={tag} | 耗时 {ms}ms | {err}")
