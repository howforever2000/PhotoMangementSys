"""OpenVINO 裁剪运行时（按需下载组件）：定位 / 解压 / 探测 / 状态上报。

## 为什么要"裁剪 + 按需下载"

完整 `openvino` 装完 244.8 MB，拆开看（2026.4.1 实测）：

    openvino_intel_npu_compiler.dll   95.7 MB   ← 编图用，本组件**不需要**（见下）
    openvino_intel_cpu_plugin.dll     44.2 MB   ← 我们不用 OV 跑 CPU
    openvino_intel_gpu_plugin.dll     33.1 MB   ← 我们不用 OV 跑核显
    openvino.dll                      15.7 MB   ← 必需
    openvino_intel_npu_plugin.dll      7.3 MB   ← 必需
    其余（各前端 / .lib / cache.json）~48 MB   ← 只要 onnx 前端

裁掉编译器 + CPU/GPU 插件 + 多余前端后 **40.7 MB**，并且实测「只认 NPU」的裁剪运行时
`import_model()` 照常、稳态 22.34 ms、与完整版输出 **cos = 1.00000000**。
（探针：`python/bench/probe_npu_blob_size.py`）

> 编译器.dll 只在「把 ONNX 编成 NPU blob」时需要，而我们每次启动都现编现用
> （编译开销已被 OpenVINO 的 CACHE_DIR 缓存），所以运行期真的不需要它。

## 为什么走 model_dl 下载一个 zip，而不是在 Rust 里解压

`model_dl` 的下载单元是「文件」，加归档解压会引入 zip 依赖与新的失败面。这里改成：
Rust 只负责把一个 `.zip` 落到 `MODEL_DIR/_runtime/`，**Python 侧首次使用时用标准库
`zipfile` 解压**并注入 `sys.path`。下载链路零改动，失败面只有"解压"这一处，且可重试。

## 为什么缺组件时必须与今天完全一致

OpenVINO 的 GPU/NPU 插件**只认 Intel 硬件**（AMD/NVIDIA 装了也用不上）。所以本模块
的全部对外能力都是"有就用、没有就返回 None"，绝不改变任何既有通道的行为 ——
`model_registry` 里只有 `clip_vision` 一条腿会因为它的存在而改变走法。
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import sys
import threading

from . import logutil
from . import config

# 组件在模型目录下的落位（与 model_dl.rs 的 DlFile.local 必须逐字一致）
ZIP_NAME = "openvino-npu-win-x64.zip"
RUNTIME_SUBDIR = "_runtime"
UNPACK_DIRNAME = "ovrt"

# 探测结果缓存（import openvino 有 ~200ms 级开销，/health 每 300ms 被宿主探测）
_probe_lock = threading.Lock()
_probe_cache: dict | None = None
_extract_lock = threading.Lock()


# ---------------------------------------------------------------------------
# 路径
# ---------------------------------------------------------------------------
def runtime_root() -> str:
    return os.path.join(config.MODEL_DIR, RUNTIME_SUBDIR)


def zip_path() -> str:
    return os.path.join(runtime_root(), ZIP_NAME)


def unpack_root() -> str:
    """解压后的 sys.path 父目录（其下应有 `openvino/`）。"""
    return os.path.join(runtime_root(), UNPACK_DIRNAME)


def partial_root() -> str:
    """解压中转目录。

    为什么保留并用它做降级：Windows 上杀毒软件常会在刚写入 DLL 后瞬时持有句柄，
    此时把目录改名会 `WinError 5 拒绝访问`（本机实测复现过）。若直接把中转目录删掉，
    用户每次启动都要重下 15MB；这里改为「改名不成则直接从中转目录跑」。
    """
    return unpack_root() + ".partial"


def package_dir(root: str | None = None) -> str:
    base = root or unpack_root()
    return os.path.join(base, "openvino")


def _valid(root: str) -> bool:
    return os.path.isdir(os.path.join(root, "openvino", "libs"))


def installed() -> bool:
    """组件是否已就位（只看目录，不做 import）。"""
    return _valid(unpack_root()) or _valid(partial_root())


def resolve_root() -> str | None:
    """当前可用的 sys.path 父目录（优先正式目录，其次解压中转目录）。"""
    if _valid(unpack_root()):
        return unpack_root()
    if _valid(partial_root()):
        return partial_root()
    return None


def download_present() -> bool:
    return os.path.isfile(zip_path())


# ---------------------------------------------------------------------------
# 解压
# ---------------------------------------------------------------------------
def ensure_unpacked() -> str | None:
    """确保组件已解压，返回可注入 sys.path 的父目录；不具备条件时返回 None。

    幂等。解压先落中转目录并校验结构，再尝试改名到正式目录；改名失败（杀毒软软/
    索引服务占句柄）**不算失败**，直接用中转目录跑 —— 半解压目录永远不会被当成可用。
    """
    root = resolve_root()
    if root:
        # 已可用：若正在从中转目录跑，顺手再试一次改名（下次启动少走一次中转）
        if root == partial_root():
            _promote(partial_root(), unpack_root())
            root = resolve_root()
        return root
    if not download_present():
        return None
    with _extract_lock:
        root = resolve_root()
        if root:
            return root
        tmp = partial_root()
        try:
            import zipfile

            if os.path.isdir(tmp):
                shutil.rmtree(tmp, ignore_errors=True)
            os.makedirs(tmp, exist_ok=True)
            with zipfile.ZipFile(zip_path()) as zf:
                base = os.path.normpath(tmp)
                # 拒绝绝对路径/上跳路径（zip slip）；本组件是我们自己打的包，属于纵深防御
                for member in zf.namelist():
                    tgt = os.path.normpath(os.path.join(tmp, member))
                    if tgt != base and not tgt.startswith(base + os.sep):
                        raise RuntimeError(f"压缩包内含非法路径: {member}")
                zf.extractall(tmp)
            if not _valid(tmp):
                raise RuntimeError("压缩包结构不符（缺少 openvino/libs）")
        except Exception as e:  # noqa: BLE001
            shutil.rmtree(tmp, ignore_errors=True)
            logutil.err(f"[VCR] OpenVINO 运行时解压失败: {type(e).__name__}: {e}")
            return None
        _promote(tmp, unpack_root())
        root = resolve_root()
        if root:
            logutil.err(f"[VCR] OpenVINO 运行时已就位: {root}")
        return root


def _promote(src: str, dst: str) -> bool:
    """把解压中转目录改名到正式目录；失败不抛（杀毒软软可能瞬时占句柄）。"""
    if _valid(dst):
        return True
    for attempt in range(3):
        try:
            if os.path.isdir(dst):
                shutil.rmtree(dst, ignore_errors=True)
            os.rename(src, dst)
            return True
        except OSError as e:
            if attempt == 2:
                logutil.err(
                    f"[VCR] OpenVINO 运行时改名失败（{type(e).__name__}: {e}），"
                    f"本次直接从 {os.path.basename(src)} 运行，下次启动重试"
                )
                return False
    return False


# ---------------------------------------------------------------------------
# 平台判断（不依赖 openvino，组件未下载时也要能回答"该不该推荐"）
# ---------------------------------------------------------------------------
def cpu_vendor() -> str:
    """返回 'intel' / 'amd' / 'unknown'（Windows 读注册表，其余走 platform）。"""
    name = ""
    try:
        if os.name == "nt":
            import winreg

            with winreg.OpenKey(
                winreg.HKEY_LOCAL_MACHINE,
                r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            ) as k:
                name = str(winreg.QueryValueEx(k, "ProcessorNameString")[0])
        else:
            name = platform.processor() or ""
    except Exception:  # noqa: BLE001
        name = platform.processor() or ""
    low = name.lower()
    if "intel" in low:
        return "intel"
    if "amd" in low or "ryzen" in low:
        return "amd"
    return "unknown"


def recommend() -> bool:
    """是否值得向这台机器推荐本组件（Intel 平台才推荐）。

    只做 CPU 厂商判断：OpenVINO 的 GPU/NPU 插件仅支持 Intel，AMD/NVIDIA 机器装了也
    没有任何收益。**注意这不等于 Intel 一定有 NPU** —— 老 Intel 机器上组件装完只会
    探测到 CPU 设备，此时前端会显示"未发现可用加速设备"，同样不算错。
    """
    return cpu_vendor() == "intel"


# ---------------------------------------------------------------------------
# 探测（会 import openvino）
# ---------------------------------------------------------------------------
def _do_probe() -> dict:
    base = {
        "installed": installed(),
        "zip_present": download_present(),
        "vendor": cpu_vendor(),
        "recommended": recommend(),
        "version": "",
        "devices": [],
        "npu": False,
        "igpu": False,
        "error": "",
    }
    if not base["installed"]:
        return base
    root = resolve_root() or ensure_unpacked()
    if not root:
        base["error"] = "解压失败"
        return base
    if root not in sys.path:
        sys.path.insert(0, root)
    try:
        import openvino as ov

        base["version"] = getattr(ov, "__version__", "")
        core = ov.Core()
        base["devices"] = list(core.available_devices)
        base["npu"] = "NPU" in base["devices"]
        base["igpu"] = "GPU" in base["devices"]
    except Exception as e:  # noqa: BLE001
        base["error"] = f"{type(e).__name__}: {e}"
    return base


def probe(refresh: bool = False) -> dict:
    """组件与设备能力（带缓存；refresh=True 重新探测）。"""
    global _probe_cache
    with _probe_lock:
        if _probe_cache is not None and not refresh:
            return dict(_probe_cache)
        _probe_cache = _do_probe()
        return dict(_probe_cache)


def reset_cache() -> None:
    """组件下载/卸载后调用，让下次 probe 重新探测。"""
    global _probe_cache
    with _probe_lock:
        _probe_cache = None


def status() -> dict:
    """对外状态（/health、性能设置面板共用）。

    `usable=True` 表示「这条腿能用 OV 跑起来」，即组件已解压且至少有一个设备可用。
    """
    p = probe()
    p["usable"] = bool(p["installed"] and not p["error"] and p["devices"])
    p["preferred_device"] = "NPU" if p["npu"] else ("GPU" if p["igpu"] else "")
    return p


def read_mode() -> str:
    """语义塔后端选择：'auto'（默认，有组件就用）| 'ort'（强制 ORT）。"""
    try:
        with open(config.CLIP_RUNTIME_PATH, encoding="utf-8") as f:
            mode = json.load(f).get("backend", "auto")
        return mode if mode in ("auto", "ort") else "auto"
    except Exception:  # noqa: BLE001
        return "auto"


def write_mode(mode: str) -> str:
    if mode not in ("auto", "ort"):
        raise ValueError(f"未知语义塔后端: {mode}")
    os.makedirs(config.MODEL_DIR, exist_ok=True)
    with open(config.CLIP_RUNTIME_PATH, "w", encoding="utf-8") as f:
        json.dump({"backend": mode}, f)
    return mode
