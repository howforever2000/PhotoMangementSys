"""OpenVINO 后端：把选定的 ONNX 通道交给 Intel NPU 执行（FEAT-135）。

为什么存在
----------
FEAT-133 只把 `clip_vision` 搬上 NPU，实测快 2.9× 但那条腿只占扫描 0.2% ⇒ 回退。
2026-11-02 在 53 张真实相册上的端到端实测发现**真正的杠杆不是「谁的 NPU 加速比高」**：

    DML 单队列才是 AI 腿的瓶颈（持锁占墙钟 98%，每张 116 次加锁；
    det_10g 单张 16ms，在 P=6 下次持锁实测 68~190ms）
    ⇒ 收益 ≈ 从 DML 队列里**拿走多少次加锁**，而不是单模型快多少毫秒。

所以本模块的服务对象是「加锁次数最多的那几条通道」：det（每张 1 次）、
ocr（约 72% 的图 1 次）、face_rec（有脸时每脸 1 次）。实测（53 张，P=6，threads=8）：

    cpu 145.2 / dml 85.7 / 三通道上 NPU 且不开 DML 43.0 / 三通道上 NPU + face_det 留 DML 38.4

三条硬约束（都是踩过的，不要「优化」掉）
----------------------------------------
1. **静态 shape 是硬前置条件，不是优化项**。NPU 编译器遇到动态维会
   `LLVM ERROR: Failed to infer result type(s)` 直接把**整个进程**带走，
   Python 侧 catch 不住（FEAT-133 实测触发两次）。⇒ 编译前显式 reshape，
   并且**先断言目标 shape 全是正整型**，让将来换模型时在 Python 层就炸，
   而不是在 NPU 驱动里静默 abort。
2. **通道形状必须从 `preprocess.py` 取，不能抄外部性能报告**。本仓库的 OCR
   喂的是 **640×640**（与 det 共用同一份 letterbox 缩放），公开报告普遍按 960 测。
   第一版按 960 写死 → 每张抛 shape 异常 → 被 `ocr_service` 的
   `except Exception` 吞成 `OcrOutcome(error=...)` → 通道根本没跑，
   探针却读到「ocr 变快了」，整体结论从 −20% 假变成 −55%。
3. **NPU 单队列**，必须串行（与 DML 一样），但**必须用另一把锁**：
   复用 `_GPU_RUN_LOCK` 会让 NPU 与 DML 互相串行，「两个独立引擎并行」立刻失效。

AMD / 非 Intel 机器
-------------------
OpenVINO 的 NPU 插件只认 Intel。`available()` 探测不到就返回不可用，
`config.OV_CHANNELS` 命中也会自动回落 ONNX Runtime —— **行为与今天逐字一致**。
"""
import os
import sys
import threading
import time

import numpy as np

from . import config

# ---------------------------------------------------------------------------
# 运行时发现
#
# 三种来源，按顺序试（都不 insert(0)：让 venv 的 numpy 优先，OV 目录里那份
# numpy 不能抢在系统前面 —— 否则会和 onnxruntime 编到的 ABI 打架）：
#   1) 已经装进解释器（pip install openvino / 打包进 exe 的运行时）
#   2) models/_runtime/ovrt（FEAT-129 的裁剪运行时 zip 解压产物）
#   3) python/.tmp-ovprobe（探针用的 --target 安装，开发期专用）
# ---------------------------------------------------------------------------
_LOCK = threading.Lock()
_OV = None
_OV_ERROR = ""
_OV_SOURCE = ""
_DEVICE_LOCKS: dict[str, threading.Lock] = {}
_DEVICE_LOCK_GUARD = threading.Lock()


def device_lock(device: str) -> threading.Lock:
    """每台设备一把锁（NPU 与 DML 各一把，互不阻塞）。"""
    with _DEVICE_LOCK_GUARD:
        lk = _DEVICE_LOCKS.get(device)
        if lk is None:
            lk = _DEVICE_LOCKS[device] = threading.Lock()
        return lk


# ---------------------------------------------------------------------------
# 可选组件：运行时 zip 的解压
#
# Rust 只负责把一个 `.zip` 落到 `MODEL_DIR/_runtime/`（下载单元仍是「文件」，
# 不引入 zip 依赖与新的失败面）；解压交给 Python 标准库 `zipfile`，首次使用时做一次。
#
# 解压不迷信改名：Windows 上杀毒会在刚写完 DLL 时占句柄，`os.rename` 实测会抛
# `WinError 5`。所以策略是「改名失败不算失败」—— 直接用 `.partial` 目录跑，
# 下次启动再试改名，避免用户每次都要重下 13MB。
# ---------------------------------------------------------------------------
ZIP_NAME = "openvino-npu-win-x64.zip"   # 与 build_ov_runtime_zip.py 逐字一致
RT_DIR = "ovrt"


def _runtime_root() -> str:
    return os.path.join(config.MODEL_DIR, "_runtime")


def ensure_runtime_extracted() -> str:
    """确保 zip 已解压 → 返回可直接注入 sys.path 的目录（空串 = 没东西可解）。"""
    root = _runtime_root()
    final = os.path.join(root, RT_DIR)
    if os.path.isdir(os.path.join(final, "openvino")):
        return final
    zp = os.path.join(root, ZIP_NAME)
    if not os.path.isfile(zp):
        return ""
    partial = final + ".partial"
    _ = partial
    import zipfile

    try:
        os.makedirs(root, exist_ok=True)
        with zipfile.ZipFile(zp) as zf:
            zf.extractall(partial)
    except Exception:  # noqa: BLE001
        return ""
    try:
        os.rename(partial, final)      # 成功则下次直接用
        return final
    except OSError:
        # 杀毒/句柄占用：直接用 .partial 跑，下次启动再试
        return partial


def _candidate_paths() -> list[str]:
    """隐式候选目录：**只放产品路径**，不放开发期目录。

    开发探针用 `VCR_OV_PATH` 显式指定（或直接把目录加进 `sys.path`，那样
    `import openvino` 会直接成功 → 记作 installed）。刻意不把 `.tmp-ovprobe`
    写进候选：隐式参与产品行为会让「未装运行时应该回落 ORT」这条变得不可验证
    （烟测里就真的翻现过：删掉路径后仍然接了 NPU）。
    """
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # python/
    out = []
    for p in os.environ.get("VCR_OV_PATH", "").split(os.pathsep):
        if p.strip():
            out.append(p.strip())
    out.append(os.path.join(config.MODEL_DIR, "_runtime", "ovrt"))
    _ = here
    return out

def load_runtime():
    """加载 openvino 模块 → (module | None, 错误串, 来源标记)。

    幂等；失败结果也缓存（NPU 驱动缺失时不要每次访问都去 import 一遍）。
    """
    global _OV, _OV_ERROR, _OV_SOURCE
    with _LOCK:
        if _OV is not None or _OV_ERROR:
            return _OV, _OV_ERROR, _OV_SOURCE
        try:
            import openvino as ov  # noqa: F401
            _OV, _OV_SOURCE = ov, "installed"
            return _OV, "", _OV_SOURCE
        except Exception:  # noqa: BLE001
            pass
        for p in _candidate_paths():
            if not os.path.isdir(os.path.join(p, "openvino")):
                continue
            if p not in sys.path:
                sys.path.append(p)   # append：不抢 venv 的 numpy
            try:
                import openvino as ov
                _OV, _OV_SOURCE = ov, p
                return _OV, "", _OV_SOURCE
            except Exception as e:  # noqa: BLE001
                _OV_ERROR = f"{type(e).__name__}: {e}"
        # 候选都不在 → 看看是不是组件 zip 还没解压
        rt = ensure_runtime_extracted()
        if rt and os.path.isdir(os.path.join(rt, "openvino")):
            if rt not in sys.path:
                sys.path.append(rt)
            try:
                import openvino as ov
                _OV, _OV_SOURCE = ov, rt
                return _OV, "", _OV_SOURCE
            except Exception as e:  # noqa: BLE001
                _OV_ERROR = f"{type(e).__name__}: {e}"
        _OV_ERROR = _OV_ERROR or "openvino 未安装（应用内下载「OpenVINO 运行时」组件，或 VCR_OV_PATH 指向运行时目录）"
        return None, _OV_ERROR, ""


_DEVINFO: dict | None = None


def probe_devices() -> dict:
    """探测可用设备（供 /health 与设置页如实上报）。

    结果缓存在进程内：`ov.Core()` 初始化是新一次插件加载（百毫秒级），
    而 /health 被宿主每 300ms 探测一次 —— 不缓存会把探活拖慢。
    """
    global _DEVINFO
    if _DEVINFO is not None:
        return _DEVINFO
    ov, err, src = load_runtime()
    if ov is None:
        _DEVINFO = {"installed": False, "error": err, "devices": [], "source": ""}
        return _DEVINFO
    try:
        core = ov.Core()
        _DEVINFO = {"installed": True, "error": "",
                    "devices": list(core.available_devices), "source": src}
    except Exception as e:  # noqa: BLE001
        _DEVINFO = {"installed": False, "error": f"{type(e).__name__}: {e}",
                    "devices": [], "source": src}
    return _DEVINFO


def available(device: str | None = None) -> bool:
    """该设备是否真的能用（不是「装了 openvino」就算能用）。"""
    dev = (device or config.OV_DEVICE).upper()
    info = probe_devices()
    if not info["installed"]:
        return False
    return any(str(d).upper() == dev for d in info["devices"])


# ---------------------------------------------------------------------------
# 编译缓存目录：NPU 首次编译 fp32 档可到 6s 级，落盘后毫秒级加载。
# 缓存键含设备与 shape，所以 reshape 变化会自然重建，不会读到脏 blob。
# ---------------------------------------------------------------------------
def cache_dir() -> str:
    d = config.OV_CACHE_DIR
    try:
        os.makedirs(d, exist_ok=True)
    except OSError:
        pass
    return d


class OvChannel:
    """一条通道的 OpenVINO 编译模型，鸭子类型对齐 `ort.InferenceSession`。

    对齐它的原因：`ModelRegistry._session_facts()` / `_bench_feed()` /
    `benchmark()` 都在读 `get_providers()`、`get_inputs()`、`get_session_options()`，
    服务层（detector / face_service / ocr_service）则只调 `registry.run(key, tensor)`。
    把垫片做在这层，上层**一行都不用改**（FEAT-133 踩过：不给垫片会让
    「测速」按钮报「通道不可用」，形成自相矛盾的 UI）。
    """

    def __init__(self, key: str, path: str, device: str, shape: tuple[int, ...]):
        ov, err, _src = load_runtime()
        if ov is None:
            raise RuntimeError(err)
        self.key = key
        self.path = path
        self.device = device
        self.shape = tuple(shape)
        self._lock = device_lock(device)
        self.compile_s = 0.0

        core = ov.Core()
        core.set_property({"CACHE_DIR": cache_dir()})
        t0 = time.perf_counter()
        model = ov.convert_model(path)
        if len(model.inputs) != 1:
            # 多输入模型（如 CLIP 双塔）不在本模块服务范围：静态化策略要单独设计
            raise RuntimeError(f"{key} 输入数 {len(model.inputs)} != 1，未适配")
        inp = model.inputs[0]
        # 硬前置条件：静态 shape。全为正整数才允许编译（见模块 docstring 第 1 条）
        if not all(isinstance(d, int) and d > 0 for d in self.shape):
            raise ValueError(f"{key} 静态 shape 非法: {self.shape}")
        model.reshape({inp.get_any_name(): self.shape})
        self.cm = core.compile_model(model, device)
        self.compile_s = time.perf_counter() - t0
        self.input_name = inp.get_any_name()
        self._names = [o.get_any_name() for o in self.cm.outputs]
        self.calls = 0
        self.fails = 0

    # ---- ort.InferenceSession 的鸭子垫片 ----
    def get_providers(self) -> list[str]:
        return [f"OpenVINO:{self.device}"]

    def get_inputs(self) -> list:
        """只暴露 shape（不是 ORT 的 NodeArg）：`_bench_feed` 只读 name/shape。"""
        return [_FakeInput(self.input_name, list(self.shape))]

    def get_session_options(self):
        return _FakeOpts()

    def run(self, _output_names, feed: dict) -> list[np.ndarray]:
        """ORT 兼容入口：`sess.run(None, {name: tensor})` 的调用形态。"""
        tensor = feed.get(self.input_name)
        if tensor is None:
            tensor = next(iter(feed.values()))
        return self.infer(tensor)

    def infer(self, tensor: np.ndarray) -> list[np.ndarray]:
        try:
            with self._lock:            # NPU 单队列：串行
                res = self.cm({self.input_name: tensor})
        except Exception:
            self.fails += 1
            raise
        self.calls += 1
        return [np.asarray(res[n]) for n in self._names]


class _FakeInput:
    __slots__ = ("name", "shape", "type")

    def __init__(self, name: str, shape: list[int]):
        self.name = name
        self.shape = shape
        self.type = "tensor(float)"


class _FakeOpts:
    intra_op_num_threads = 0


def build(key: str, path: str, device: str | None = None) -> OvChannel | None:
    """按 config.OV_SHAPES 编译一条通道。

    返回 None = **不适用**（该通道不在名单 / 设备不可用 / 文件不在）；
    真的编译失败则**抛异常**，由调用方用 logutil 记下原因再回落 ORT ——
    不能在这里静默吞掉，否则就是 BUG-2026-1101 那一类「NPU 显示已启用但 0 占用」。
    """
    if key not in config.OV_SHAPES:
        return None
    dev = (device or config.OV_DEVICE).upper()
    if not available(dev):
        return None
    if not os.path.isfile(path):
        return None
    return OvChannel(key, path, dev, config.OV_SHAPES[key])
