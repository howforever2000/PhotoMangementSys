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
        # FEAT-051：默认强制 CPU（用户在 UI「检测 GPU → 启用加速」后再切 GPU，
        # 参考单相册扫描面板的交互）；env VCR_PROVIDER=auto 可恢复自动探测
        self._providers_sel: list[str] | None = ["CPUExecutionProvider"]
        self._gpu_forced_off = True
        # FEAT-051：用户指定的分类模型文件名（None = 按 CLS_MODELS 顺序回退）
        # FEAT-052：启动时读取持久化选择（models/current_cls.json），UI 选过的跨重启保持
        self._cls_override: str | None = self._load_persisted_cls()
        if os.environ.get("VCR_PROVIDER", "").lower() == "auto":
            self._providers_sel = None
            self._gpu_forced_off = False

    @staticmethod
    def _load_persisted_cls() -> str | None:
        """读取持久化的分类模型选择（文件缺失/非法/未下载 → None 走默认回退）。"""
        try:
            import json

            path = config.CLS_CURRENT_PATH
            if os.path.isfile(path):
                with open(path, encoding="utf-8") as f:
                    name = json.load(f).get("name")
                if name in config.CLS_MODEL_META and os.path.isfile(
                    os.path.join(config.MODEL_DIR, name)
                ):
                    return name
        except Exception:  # noqa: BLE001
            pass
        return None

    def _persist_cls(self, name: str) -> None:
        """持久化用户选择（失败不阻断）。"""
        try:
            import json

            with open(config.CLS_CURRENT_PATH, "w", encoding="utf-8") as f:
                json.dump({"name": name}, f)
        except Exception:  # noqa: BLE001
            pass

    # ------------------------------------------------------------------
    def _so(self) -> ort.SessionOptions:
        so = ort.SessionOptions()
        so.intra_op_num_threads = config.THREADS
        so.inter_op_num_threads = 1
        # P1 ONNX 优化：启用所有优化（常量折叠/算子融合/layernorm 等），加速推理 30~60%
        # 依赖图完全静态化，dynamic axes 模型会自动跳过不适用的优化
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        # 启用内存规划，减少推理时的内存分配开销
        so.enable_mem_pattern = True
        # 启用 CPU 线程池（配合 THREADS 参数）
        # 修复：部分 onnxruntime 版本无 ThreadPoolOptions Python API（一加载即报错，
        # 全模型瘫痪），存在才启用
        if hasattr(ort, "ThreadPoolOptions"):
            so.threadpool_options = ort.ThreadPoolOptions()
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
            # FEAT-051：是否被用户强制关闭 GPU（与「无 GPU 可用」区分，前端开关初始状态用）
            "forced_cpu": self._gpu_forced_off,
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
        # FEAT-051：用户指定模型优先（文件存在才生效，否则回退默认列表）
        if self._cls_override:
            override_path = os.path.join(config.MODEL_DIR, self._cls_override)
            if os.path.isfile(override_path):
                self._load("cls", [override_path], required=True)
                return self._sessions.get("cls")
            self._cls_override = None
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

    # ------------------------------------------------------------------
    # FEAT-051：运行时切换（GPU 加速 / 分类模型），供 UI 按硬件性能选择
    # ------------------------------------------------------------------
    def set_gpu_enabled(self, enabled: bool) -> dict:
        """切换 GPU 加速：enabled=False 强制 CPU。

        provider 在会话创建时绑定，切换后清空全部已加载会话并惰性重建
        （下一次推理请求时生效，首个批次略有重建开销）。
        """
        self._gpu_forced_off = not enabled
        self._providers_sel = None if enabled else ["CPUExecutionProvider"]
        self._reload_all()
        return self.gpu_info()

    def set_cls_model(self, name: str) -> dict:
        """切换分类模型到指定文件（须在候选清单中且已下载）。"""
        if name not in config.CLS_MODEL_META:
            raise ValueError(f"未知分类模型: {name}")
        path = os.path.join(config.MODEL_DIR, name)
        if not os.path.isfile(path):
            raise FileNotFoundError(f"模型文件未下载: {path}")
        self._cls_override = name
        self._sessions.pop("cls", None)
        self._ready.pop("cls", None)
        self._load_errors.pop("cls", None)
        self._load("cls", [path], required=True)
        if not self._ready.get("cls"):
            err = self._load_errors.get("cls", "加载失败")
            self._cls_override = None
            # 持久化指向的模型加载失败 → 删除记录，下次启动走默认回退
            try:
                os.remove(config.CLS_CURRENT_PATH)
            except OSError:
                pass
            raise RuntimeError(f"模型加载失败: {err}")
        self._persist_cls(name)
        return self.cls_models_info()

    def cls_models_info(self) -> dict:
        """分类模型候选清单 + 当前生效模型（UI 用）。"""
        current = self._cls_override
        if not current:
            for m in config.CLS_MODELS:
                if os.path.isfile(os.path.join(config.MODEL_DIR, m)):
                    current = m
                    break
        models = []
        for m in config.CLS_MODELS:
            meta = config.CLS_MODEL_META.get(m, {})
            models.append({
                "name": m,
                "label": meta.get("label", m),
                "accuracy": meta.get("accuracy", ""),
                "speed": meta.get("speed", ""),
                "downloaded": os.path.isfile(os.path.join(config.MODEL_DIR, m)),
                "active": m == current,
            })
        return {"models": models, "current": current}

    def _reload_all(self):
        """清空全部会话槽位，下次访问按新 provider 惰性重建。"""
        self._sessions.clear()
        self._ready.clear()

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
