"""语义图像塔（CLIP vision）的 OpenVINO 后端：只接这一条腿。

## 为什么只有这一条腿

本机实测（Intel Ultra 5 245K + Intel AI Boost NPU，OpenVINO 2026.4.1）：

    通道                         NPU        iGPU       CPU
    clip_vision 1x3x224x224     19.7 ms    26.1 ms    56.7 ms   ← NPU 最优
    det         yolov8n 640     12.3 ms     7.0 ms    15.2 ms   ← NPU 输核显
    face_det    det_10g 640    115.9 ms    14.0 ms    36.1 ms   ← NPU 慢 8 倍，禁用
    face_rec    w600k_r50 112    8.2 ms     6.5 ms    22.1 ms   ← NPU 输核显
    ocr         paddleocr 960   33.1 ms    24.4 ms    25.8 ms   ← NPU 输

所以本模块**只**接管语义图像塔；其余通道一律留在 ONNX Runtime（现状不变）。
`face_det` 在 NPU 上的候选资格**永久排除**——它是 8 倍差距，不是调优空间。

## 为什么文本塔也不接管

`clip_text.onnx` 的 attention_mask 带**无界动态维**，NPU 编译器会直接
`LLVM ERROR: Failed to infer result type(s)` 把**整个进程**带走（不可 catch）。
实测两次触发。文本塔本身也不是热点（一次请求只编几个关键词），留在 ORT。

## 数值纪律

NPU 内部把权重压成 f16（fp32 档也一样）。实测 vs CPU fp32 基准：
    fp16 档 cos = 0.999998 | fp32 档 cos = 0.999997
即**不属于** BUG-2026-0910-006（fp16 档在 AMD DML 上静默产出错误向量）那一类。
`bench/probe_npu_clip_tiers.py` 可在任意机器上复跑这条判据。

## 线程纪律

NPU 只有一条队列，且 OV 的 compiled_model 在高并发下仍建议串行喂。这里沿用
`model_registry._GPU_RUN_LOCK` 的同一条纪律：**锁只包住 infer**，
解码/归一化/L2/落库留在锁外，由照片级并行（PHOTO_PARALLEL）继续跑。
"""
from __future__ import annotations

import os
import sys
import threading
import time

import numpy as np

# NPU 单队列：锁只包住 infer，不包预处理/后处理（与 _GPU_RUN_LOCK 同纪律）
_RUN_LOCK = threading.Lock()


class OvClipVision:
    """CLIP 图像塔的 OpenVINO 会话（静态 batch=1）。

    batch 固定为 1 是硬要求：NPU 只吃静态 shape，而 `embed_images()` 会
    `np.vstack` 出 N 张（N 可为 1~8）。这里按张循环，对外仍是 (N, dim)。
    """

    def __init__(self, onnx_path: str, size: int, device: str = ""):
        self.onnx_path = onnx_path
        self.size = int(size)
        self.device = device or "CPU"
        self.compiled = None
        self.load_ms = 0.0
        self.error = ""
        self._input_name = "pixel_values"

    # -- 生命周期 -----------------------------------------------------
    def load(self, devices: list[str] | None = None) -> bool:
        """编译到最优设备；失败按候选链降级，全部失败才返回 False。"""
        t0 = time.perf_counter()
        try:
            import openvino as ov

            self._ov_version = getattr(ov, "__version__", "")
            core = ov.Core()
            model = core.read_model(self.onnx_path)
            # 静态化：动态维会让 NPU 编译器直接 abort（见模块 docstring）
            model.reshape({model.input(0): [1, 3, self.size, self.size]})
        except Exception as e:  # noqa: BLE001
            self.error = f"读取模型失败: {type(e).__name__}: {e}"
            return False

        order = devices or [self.device, "NPU", "GPU", "CPU"]
        # 只尝试**真实存在**的设备：裁剪版运行时里 CPU/GPU 插件已被删掉，
        # 盲目按候选链降级只会刷一堆「设备不存在」日志，掩盖真正原因。
        try:
            available = list(core.available_devices)
        except Exception:  # noqa: BLE001
            available = order
        order = [d for d in order if d in available] or available
        tried: list[str] = []
        for dev in order:
            if not dev or dev in tried:
                continue
            tried.append(dev)
            try:
                compiled = core.compile_model(model, dev)
                # NPU 的真正编译发生在首次 infer，先跑一次空转把代价前置，
                # 否则第一批照片会莫名多花 0.7~6.3 秒
                compiled(
                    {compiled.input(0).get_any_name():
                     np.zeros((1, 3, self.size, self.size), dtype=np.float32)}
                )
                self.compiled = compiled
                self.device = dev
                self.load_ms = (time.perf_counter() - t0) * 1000.0
                try:
                    self._input_name = compiled.input(0).get_any_name()
                except Exception:  # noqa: BLE001
                    pass
                self.error = ""
                return True
            except Exception as e:  # noqa: BLE001
                self.error = f"{dev}: {type(e).__name__}: {str(e)[:300]}"
                print(f"[VCR] 语义塔 {dev} 编译失败: {self.error}", file=sys.stderr)
                continue
        return False

    @property
    def ready(self) -> bool:
        return self.compiled is not None

    # -- 推理 ---------------------------------------------------------
    def infer(self, pixels: np.ndarray) -> np.ndarray:
        """(N,3,S,S) fp32 → (N,dim) fp32。按张循环喂静态 batch=1。"""
        if self.compiled is None:
            raise RuntimeError("OpenVINO 语义塔未就绪")
        if pixels.ndim != 4 or pixels.shape[1:] != (3, self.size, self.size):
            raise ValueError(f"输入形状不符: {pixels.shape}")
        out = []
        for i in range(pixels.shape[0]):
            x = np.ascontiguousarray(pixels[i:i + 1], dtype=np.float32)
            with _RUN_LOCK:
                y = self.compiled({self._input_name: x})[self.compiled.output(0)]
            out.append(np.asarray(y).astype(np.float32).reshape(-1))
        return np.vstack(out) if out else np.zeros((0, 512), dtype=np.float32)

    # -- 适配 registry.benchmark 需要的最小会话形状 -------------------
    # benchmark() 会调 sess.run(None, feed) / get_providers() / get_inputs()[0].name。
    # 两种签名都吃：run(pixels) → 数组；run(None, feed) → [数组]（对齐 ORT 返回值）。
    def run(self, output_names=None, feed=None):
        if feed is None:
            return self.infer(output_names)
        x = next(iter(feed.values()))
        return [self.infer(x)]

    def get_providers(self) -> list[str]:
        return [f"OpenVINO:{self.device}"] if self.ready else []

    def get_inputs(self):
        name = self._input_name

        class _Inp:
            name = None

        _Inp.name = name
        return [_Inp()]

    # -- 事实上报 -----------------------------------------------------
    def facts(self) -> dict:
        return {
            "file": os.path.basename(self.onnx_path),
            "file_path": self.onnx_path,
            "threads": None,
            "file_size": _safe_size(self.onnx_path),
            # 与 ORT 会话同口径：providers[0] = 实际在跑的后端，UI 据此显示徽章
            "providers": self.get_providers() or [self.error or "未就绪"],
            "input_name": self._input_name,
            "input_shape": [1, 3, self.size, self.size],
            "input_type": "tensor(float)",
            "cpu_fallback": self.device == "CPU",
            "backend": "openvino",
            "device": self.device,
            "load_ms": round(self.load_ms, 1),
            "ov_version": getattr(self, "_ov_version", ""),
        }


def _safe_size(path: str) -> int | None:
    try:
        return os.path.getsize(path)
    except OSError:
        return None
