"""ONNX 模型注册表：集中加载/持有会话，按需惰性初始化。

每个模型一个 session 槽位，缺失时该通道自动降级（is_ready=False），
主流程（分类）不依赖任何可选模型。
"""
import os

import numpy as np
import onnxruntime as ort

from . import config


class ModelRegistry:
    def __init__(self):
        self._sessions: dict[str, ort.InferenceSession] = {}
        self._ready: dict[str, bool] = {}
        self._load_errors: dict[str, str] = {}
        self._providers_sel: list[str] | None = None

    # ------------------------------------------------------------------
    def _so(self) -> ort.SessionOptions:
        so = ort.SessionOptions()
        so.intra_op_num_threads = config.THREADS
        so.inter_op_num_threads = 1
        return so

    # ------------------------------------------------------------------
    # GPU 提供方选择（R3）：自动探测 + 可选 env 开关，CPU 兜底
    # ------------------------------------------------------------------
    def _providers(self) -> list[str]:
        """返回优先提供方列表（GPU 优先，CPU 兜底）。"""
        if self._providers_sel is not None:
            return self._providers_sel
        available = ort.get_available_providers()
        pref: list[str] = []
        if config.VCR_PROVIDER != "cpu":
            # DirectML（通用 GPU，免 CUDA）优先，其次 CUDA（NVIDIA）
            for g in ("DmlExecutionProvider", "CUDAExecutionProvider"):
                if g in available:
                    pref.append(g)
                    break
        pref.append("CPUExecutionProvider")
        self._providers_sel = pref
        return pref

    def gpu_info(self) -> dict:
        """GPU 可行性探测：可用提供方、当前是否走 GPU、选中提供方。"""
        available = ort.get_available_providers()
        # 仅统计真正的本地 GPU 加速器（排除 Azure 等云端提供方）
        known_gpu = {
            "DmlExecutionProvider", "CUDAExecutionProvider", "ROCmExecutionProvider",
            "TensorrtExecutionProvider", "OpenVINOExecutionProvider",
        }
        gpu = [p for p in available if p in known_gpu]
        providers = self._providers()
        using_gpu = bool(providers) and providers[0] != "CPUExecutionProvider"
        return {
            "available": available,
            "gpu": gpu,
            "use_gpu": using_gpu,
            "provider": providers[0] if providers else "cpu",
        }

    def _load(self, key: str, paths: list[str], required: bool = False):
        if key in self._ready:
            return
        for p in paths:
            if os.path.isfile(p):
                try:
                    self._sessions[key] = ort.InferenceSession(
                        p, sess_options=self._so(), providers=self._providers()
                    )
                    self._ready[key] = True
                    return
                except Exception as e:  # noqa: BLE001
                    import sys as _sys

                    print(f"[VCR] 模型加载失败 {p}: {e}", file=_sys.stderr)
                    self._load_errors[key] = str(e)
                    continue
        self._ready[key] = False
        if required:
            self._load_errors[key] = f"必需模型缺失: {paths}"

    # ------------------------------------------------------------------
    @property
    def cls(self) -> ort.InferenceSession | None:
        self._load("cls", [os.path.join(config.MODEL_DIR, m) for m in config.CLS_MODELS], required=True)
        return self._sessions.get("cls")

    @property
    def det(self) -> ort.InferenceSession | None:
        self._load("det", [os.path.join(config.MODEL_DIR, config.DET_MODEL)])
        return self._sessions.get("det")

    @property
    def face_det(self) -> ort.InferenceSession | None:
        self._load("face_det", [os.path.join(config.MODEL_DIR, m) for m in config.FACE_DET_MODELS])
        return self._sessions.get("face_det")

    @property
    def face_rec(self) -> ort.InferenceSession | None:
        self._load("face_rec", [os.path.join(config.MODEL_DIR, m) for m in config.FACE_REC_MODELS])
        return self._sessions.get("face_rec")

    @property
    def scene(self) -> ort.InferenceSession | None:
        # Places365 是场景分类主通道，依赖性强：标记为 required=True，缺失时服务启动会显式提醒。
        self._load("scene", [os.path.join(config.MODEL_DIR, config.SCENE_MODEL)], required=True)
        return self._sessions.get("scene")

    @property
    def ocr(self) -> ort.InferenceSession | None:
        self._load("ocr", [os.path.join(config.MODEL_DIR, config.OCR_MODEL)])
        return self._sessions.get("ocr")

    @property
    def flower(self) -> ort.InferenceSession | None:
        self._load("flower", [os.path.join(config.MODEL_DIR, config.FLOWER_MODEL)])
        return self._sessions.get("flower")

    @property
    def food(self) -> ort.InferenceSession | None:
        self._load("food", [os.path.join(config.MODEL_DIR, config.FOOD_MODEL)])
        return self._sessions.get("food")

    # ------------------------------------------------------------------
    def run(self, key: str, tensor) -> list[np.ndarray]:
        sess = self._sessions[key]
        return sess.run(None, {sess.get_inputs()[0].name: tensor})

    def is_ready(self, key: str) -> bool:
        self._ready.setdefault(key, False)
        return self._ready[key]

    def status(self) -> dict:
        # 强制加载全部通道，反映真实状态
        self.cls
        self.det
        self.face_det
        self.face_rec
        self.scene
        self.ocr
        self.flower
        self.food
        keys = ["cls", "det", "face_det", "face_rec", "scene", "ocr", "flower", "food"]
        return {k: {"ready": self.is_ready(k), "error": self._load_errors.get(k, "")} for k in keys}


_registry: ModelRegistry | None = None


def get_registry() -> ModelRegistry:
    global _registry
    if _registry is None:
        _registry = ModelRegistry()
    return _registry
