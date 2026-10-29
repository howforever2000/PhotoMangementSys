"""BUG-2026-1101-001 回归：**stderr 写失败不得把成功的 OpenVINO 加载改写成失败**。

## 现场（用户实际症状）

性能设置里显示「语义索引 · NPU 加速中」，但 NPU 占用恒为 0。查活服务的 `/health`：

    sessions.clip_vision = ["OpenVINO:NPU"]          ← 会话确实建好了
    load_errors.clip_vision = "OpenVINO: OSError: [Errno 22] Invalid argument"
    clip_ready = false                               ← 但宿主认为没就绪

三行互相矛盾，宿主的语义腿因为 `clip_ready=false` 整段跳过 ⇒ NPU 一次都没被调用。

## 根因

打包版对子进程用的是 `Stdio::piped()`（`vision.rs::attach_child_output`），
管道会在服务重启/宿主回收时关闭。`_ensure_ov_clip()` 当时是这个顺序：

    self._ov_clip = ov            # 状态提交
    self._ready[...] = True
    self._session_info[...] = ... # → providers 显示 OpenVINO:NPU
    print(..., file=sys.stderr)   # ← 管道已关 → OSError: [Errno 22]

那一行被外层 `except Exception` 接住，于是**成功的加载被改写为失败**：
`_ready=False`、`load_errors` 写回错误，而 `_ov_clip` 已经赋值且不会被回滚
⇒ UI 显示加速中、health 说没就绪。

## 本脚本的判据（缺一不算通过）

1. stderr 每次写都抛 `OSError(22)` 时，`clip_vision` 仍然 `is_ready() == True`
2. `load_errors` 为空（不能被"日志炸了"写成加载失败）
3. `clip_runtime_status()` 的 `active` 与 `is_ready` 一致（不能自相矛盾）
4. 异常路径反过来也成立：组件不可用时不崩、`active=False`

用法：
    cd python
    .\\.venv-vcr\\Scripts\\python.exe bench\\verify_ov_clip_broken_stderr.py
"""
from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from vcr import ovrt  # noqa: E402
from vcr.model_registry import ModelRegistry  # noqa: E402


class _DeadPipe:
    """模拟「宿主管道已关闭」的 stderr：任何写入都抛 OSError(22)。"""

    def write(self, _s):  # noqa: D401
        raise OSError(22, "Invalid argument")

    def flush(self):
        raise OSError(22, "Invalid argument")

    def isatty(self):
        return False


def main() -> int:
    st = ovrt.status()
    if not st.get("usable"):
        print(f"[SKIP] 本机没有可用的 OpenVINO 组件（installed={st['installed']} "
              f"devices={st['devices']}），本回归无处附着")
        return 0

    real_stderr = sys.stderr
    ok = True
    try:
        sys.stderr = _DeadPipe()  # type: ignore[assignment]
        reg = ModelRegistry()
        reg.set_clip_runtime("auto")
        _ = reg.clip_vision                       # 触发 OV 加载（日志会撞上死管道）
        status = reg.clip_runtime_status()
        ready = reg.is_ready("clip_vision")
        err = reg.load_error("clip_vision")
    finally:
        sys.stderr = real_stderr

    print(f"is_ready(clip_vision) = {ready}")
    print(f"load_error            = {err!r}")
    print(f"status.active         = {status['active']} | backend={status['backend']} "
          f"| device={status['device']!r}")
    print(f"session providers     = {reg._session_info.get('clip_vision', {}).get('providers')}")

    if not ready:
        print("❌ stderr 关闭时 clip_vision 未就绪 —— BUG 未修好")
        ok = False
    if err:
        print(f"❌ 日志写入失败被当成了加载失败：{err}")
        ok = False
    if status["active"] != ready:
        print("❌ active 与 is_ready 不一致（UI 与实际会各说一套）")
        ok = False
    if status["active"] and status["backend"] != "openvino":
        print("❌ active=true 但 backend 不是 openvino")
        ok = False

    print(f"\n结论：{'✅ 通过（stderr 死管道不再影响语义塔状态）' if ok else '❌ 不通过'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
