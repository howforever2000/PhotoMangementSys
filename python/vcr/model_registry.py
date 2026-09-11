"""ONNX 模型注册表：集中加载/持有会话，按需惰性初始化。

每个模型一个 session 槽位，缺失时该通道自动降级（is_ready=False），
主流程（人物检测）不依赖任何可选模型。

v5（语义分类）：分类模型（yolov8*-cls）、Places365 场景、花朵/食物专家槽位已删除；
新增「语义模型档位」切换（config.CLIP_MODEL_META：B/16 ↔ L/14-336），
供 UI 按硬件性能选择，与旧分类模型切换同一套异步交互（begin/finish）。
"""
import os
import threading
import time

import numpy as np
import onnxruntime as ort

from . import config


class ModelRegistry:
    def __init__(self):
        self._sessions: dict[str, ort.InferenceSession] = {}
        self._ready: dict[str, bool] = {}
        self._load_errors: dict[str, str] = {}
        # FEAT-053：会话实测事实（ORT 真实绑定的 provider / 源文件 / 输入元数据）。
        # 与 _sessions 同生命周期：会话被清空的场合必须同步 pop，否则上报的是旧会话。
        self._session_info: dict[str, dict] = {}
        # FEAT-051：默认强制 CPU（用户在 UI「检测 GPU → 启用加速」后再切 GPU）。
        # env VCR_PROVIDER=auto 可恢复自动探测。
        self._providers_sel: list[str] | None = ["CPUExecutionProvider"]
        self._gpu_forced_off = True
        # 会话加载/切换锁：后台预加载线程与 FastAPI 线程池并发访问会话槽位，
        # _load 的 check-then-act 必须串行。注意：is_ready / gpu_info 故意不加锁
        # （/health 每 300ms 被宿主探测，等加载锁会被阻塞数分钟 → 宿主误判不可达）。
        self._lock = threading.RLock()
        if os.environ.get("VCR_PROVIDER", "").lower() == "auto":
            self._providers_sel = None
            self._gpu_forced_off = False

    @staticmethod
    def _session_facts(sess: ort.InferenceSession, path: str, cpu_fallback: bool) -> dict:
        """提取会话实测事实 —— 「确实在用 GPU / 确实换了模型」的铁证。"""
        try:
            provs = list(sess.get_providers())
        except Exception:  # noqa: BLE001
            provs = []
        try:
            inputs = sess.get_inputs()
            inp = inputs[0] if inputs else None
        except Exception:  # noqa: BLE001
            inp = None
        shape: list = []
        if inp is not None:
            shape = [d if isinstance(d, int) else str(d) for d in inp.shape]
        try:
            size: int | None = os.path.getsize(path)
        except OSError:
            size = None
        return {
            "file": os.path.basename(path),
            "file_path": path,
            "file_size": size,
            "providers": provs,
            "input_name": inp.name if inp is not None else None,
            "input_shape": shape,
            "input_type": inp.type if inp is not None else None,
            "cpu_fallback": cpu_fallback,
        }

    def _create_session(
        self,
        path: str,
        opts: ort.SessionOptions | None = None,
        providers: list[str] | None = None,
    ) -> tuple[ort.InferenceSession, bool]:
        """按 provider 候选创建会话；GPU 初始化失败回退纯 CPU 重试。

        providers 省略时用全局选择（GPU 开关）；CLIP 需要按档位覆盖
        （fp16 档固定 CPU，见 _clip_providers）。

        返回 (session, cpu_fallback)。回退仅对本会话生效并记录到实测信息，
        不改全局 provider 选择 —— 下次重建仍先尝试用户选择的 provider。
        """
        opts = opts or self._so()
        prov = providers if providers is not None else self._providers()
        try:
            return ort.InferenceSession(path, sess_options=opts, providers=prov), False
        except Exception as gpu_err:  # noqa: BLE001
            if not prov or prov[0] == "CPUExecutionProvider":
                raise
            import sys as _sys

            print(
                f"[VCR] GPU 会话创建失败（{prov[0]}），回退 CPU 重试: {gpu_err}",
                file=_sys.stderr,
            )
            sess = ort.InferenceSession(
                path, sess_options=opts, providers=["CPUExecutionProvider"]
            )
            return sess, True

    # ------------------------------------------------------------------
    def _so(self) -> ort.SessionOptions:
        so = ort.SessionOptions()
        so.intra_op_num_threads = config.THREADS
        so.inter_op_num_threads = 1
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        so.enable_mem_pattern = True
        if hasattr(ort, "ThreadPoolOptions"):
            so.threadpool_options = ort.ThreadPoolOptions()
        return so

    def _so_clip(self) -> ort.SessionOptions:
        """CLIP 会话专用 SessionOptions。

        - provider 由 _clip_providers() 按档位决定（fp16 固定 CPU / fp32 跟随开关）
        - fp16 图在 DML 上有算子级数值 bug，且全量图优化会在 vision 塔初始化时崩溃
          （BUG-2026-0910-006）→ 统一用 BASIC 优化：CPU 与 DML(fp32) 均已实测数值正确
        """
        so = self._so()
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
        return so

    # ------------------------------------------------------------------
    # GPU 提供方选择（R3）
    # ------------------------------------------------------------------
    def _providers(self) -> list[str]:
        """返回优先提供方列表（GPU 优先，CPU 兜底）。"""
        if self._providers_sel is not None:
            return self._providers_sel
        available = ort.get_available_providers()
        pref: list[str] = []
        if config.VCR_PROVIDER != "cpu":
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
            "forced_cpu": self._gpu_forced_off,
            "sessions": {k: v.get("providers", []) for k, v in self._session_info.items()},
        }

    def _load(self, key: str, paths: list[str], required: bool = False):
        with self._lock:
            if key in self._ready:
                return
            for p in paths:
                if os.path.isfile(p):
                    try:
                        sess, cpu_fallback = self._create_session(p)
                        self._sessions[key] = sess
                        self._ready[key] = True
                        self._session_info[key] = self._session_facts(sess, p, cpu_fallback)
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
    # 规则通道
    # ------------------------------------------------------------------
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
    def ocr(self) -> ort.InferenceSession | None:
        self._load("ocr", [os.path.join(config.MODEL_DIR, config.OCR_MODEL)])
        return self._sessions.get("ocr")

    def _clip_providers(self) -> list[str]:
        """CLIP 双塔的 provider：按档位决定（fp16 固定 CPU / fp32 跟随 GPU 开关）。

        为什么必须分开：
          - fp16 档（model_fp16.onnx）在 AMD DirectML 上有**算子级数值 bug**
            （实测输出错误，见 BUG-2026-0910-006）→ 无论 GPU 开关如何都固定 CPU；
            此前这里直接用全局 provider，一旦用户打开 GPU 开关就会把 fp16 送到 DML
            上进而静默产出错误向量（潜在缺陷，本次一并修掉）。
          - fp32 档（model.onnx）实测 DML 与 CPU 数值完全一致（余弦 1.000000）
            且快约 2 倍（53.3ms vs 108.2ms/张，见 python/bench/verify_clip_tiers.py），
            故跟随用户的 GPU 开关（默认仍为 CPU，用户显式开启才用 GPU）。
        """
        p = config.clip_paths()
        if "fp16" in p["onnx"]:
            return ["CPUExecutionProvider"]
        return self._providers()

    # ------------------------------------------------------------------
    # 语义双塔（当前档位）：拆分件由 embed_service.ensure() 生成
    # ------------------------------------------------------------------
    @property
    def clip_vision(self) -> ort.InferenceSession | None:
        p = config.clip_paths()
        with self._lock:
            if "clip_vision" not in self._ready:
                try:
                    sess, cpu_fb = self._create_session(p["vision"], self._so_clip(), self._clip_providers())
                    self._sessions["clip_vision"] = sess
                    self._ready["clip_vision"] = True
                    self._session_info["clip_vision"] = self._session_facts(sess, p["vision"], cpu_fb)
                except Exception as e:  # noqa: BLE001
                    self._load_errors["clip_vision"] = str(e)
                    self._ready["clip_vision"] = False
        return self._sessions.get("clip_vision")

    @property
    def clip_text(self) -> ort.InferenceSession | None:
        p = config.clip_paths()
        with self._lock:
            if "clip_text" not in self._ready:
                try:
                    sess, cpu_fb = self._create_session(p["text"], self._so_clip(), self._clip_providers())
                    self._sessions["clip_text"] = sess
                    self._ready["clip_text"] = True
                    self._session_info["clip_text"] = self._session_facts(sess, p["text"], cpu_fb)
                except Exception as e:  # noqa: BLE001
                    self._load_errors["clip_text"] = str(e)
                    self._ready["clip_text"] = False
        return self._sessions.get("clip_text")

    def load_error(self, key: str) -> str:
        return self._load_errors.get(key, "")

    # ------------------------------------------------------------------
    def run(self, key: str, tensor) -> list[np.ndarray]:
        sess = self._sessions[key]
        return sess.run(None, {sess.get_inputs()[0].name: tensor})

    def run_clip_vision(self, pixel_values: np.ndarray) -> np.ndarray:
        """图像塔前向 → (N,dim) fp32（fp16 图输出已 cast 回 fp32）。"""
        sess = self._sessions["clip_vision"]
        return sess.run(None, {sess.get_inputs()[0].name: pixel_values})[0].astype(np.float32)

    def run_clip_text(self, input_ids: np.ndarray, attention_mask: np.ndarray) -> np.ndarray:
        """文本塔前向 → (N,dim) fp32。"""
        sess = self._sessions["clip_text"]
        return sess.run(None, {
            sess.get_inputs()[0].name: input_ids,
            sess.get_inputs()[1].name: attention_mask,
        })[0].astype(np.float32)

    # ------------------------------------------------------------------
    # FEAT-053：固定张量测速 —— CPU/GPU 真实加速比一键对比
    # ------------------------------------------------------------------
    def _bench_feed(self, key: str, sess: ort.InferenceSession) -> dict:
        """按通道构造固定输入（CLIP 双塔输入不同，其余图像模型一律 1x3xSxS）。"""
        if key.startswith("clip_"):
            p = config.clip_paths()
            if key == "clip_vision":
                return {sess.get_inputs()[0].name:
                        np.random.randn(1, 3, int(p["size"]), int(p["size"])).astype(np.float32)}
            ids = np.ones((1, int(p["max_len"])), dtype=np.int64)
            return {sess.get_inputs()[0].name: ids, sess.get_inputs()[1].name: np.ones_like(ids)}
        inp = sess.get_inputs()[0]
        shape: list[int] = []
        for i, d in enumerate(inp.shape):
            if isinstance(d, int) and d > 0:
                shape.append(d)
            elif i == 0:
                shape.append(1)
            elif i == 1:
                shape.append(3)
            else:
                shape.append(224)
        return {inp.name: np.random.randn(*shape).astype(np.float32)}

    def benchmark(self, key: str = "det", runs: int = 10, warmup: int = 2) -> dict:
        """预热后计时 N 次推理，输出平均/最快/最慢毫秒与实测 provider。

        测速在锁外进行：会话是不可变对象（切换 = 整体换引用），持锁计时
        反而会被并发模型加载阻塞导致读数失真。
        """
        runs = max(1, min(int(runs), 50))
        warmup = max(0, min(int(warmup), 10))
        with self._lock:
            sess = self._sessions.get(key)
            if sess is None:
                _ = getattr(self, key, None)  # 属性即会话 property，触发惰性加载
                sess = self._sessions.get(key)
        if sess is None:
            raise RuntimeError(f"通道 {key} 不可用，无法测速")
        feed = self._bench_feed(key, sess)
        for _ in range(warmup):
            sess.run(None, feed)
        times: list[float] = []
        for _ in range(runs):
            t0 = time.perf_counter()
            sess.run(None, feed)
            times.append((time.perf_counter() - t0) * 1000.0)
        times.sort()
        total = sum(times)
        return {
            "channel": key,
            "runs": runs,
            "warmup": warmup,
            "input_shape": [list(v.shape) for v in feed.values()],
            "providers": list(sess.get_providers()),
            "avg_ms": round(total / runs, 2),
            "min_ms": round(times[0], 2),
            "max_ms": round(times[-1], 2),
            "total_ms": round(total, 2),
            "throughput_per_s": round(runs / (total / 1000.0), 1) if total > 0 else None,
        }

    # ------------------------------------------------------------------
    # 语义模型档位切换（适配不同硬件）
    # ------------------------------------------------------------------
    def current_clip_tier(self) -> str:
        """当前登记生效的语义模型档位（不触发加载，可被 /health 高频调用）。"""
        return config.active_clip()

    def clip_models_info(self) -> dict:
        """语义模型档位清单 + 当前生效 + 会话实测事实（UI 用）。"""
        current = self.current_clip_tier()
        models = []
        for name in config.CLIP_MODELS:
            meta = config.CLIP_MODEL_META[name]
            p = config.clip_paths(name)
            vision_ok = os.path.isfile(p["vision"])
            whole_ok = os.path.isfile(p["whole"])
            models.append({
                "name": name,
                "label": meta["label"],
                "accuracy": meta.get("accuracy", ""),
                "speed": meta.get("speed", ""),
                "note": meta.get("note", ""),
                "dim": meta["dim"],
                "size": meta["size"],
                # 模型体积（UI 在下载/切换前明示成本）
                "bytes": meta.get("bytes", 0),
                # 已就绪 = 拆分件在；仅整图在 → 首次使用时自动拆（仍可选）
                "downloaded": vision_ok or whole_ok,
                "ready": vision_ok,
                "active": name == current,
            })
        return {
            "models": models,
            "current": current,
            "clip_ready": self.is_ready("clip_vision") and self.is_ready("clip_text"),
            "loaded": self._session_info.get("clip_vision"),
            "loaded_text": self._session_info.get("clip_text"),
        }

    def begin_clip_switch(self, name: str) -> dict:
        """校验并登记档位切换目标，清空 CLIP 会话槽位后立即返回。

        实际加载由后台线程 finish_clip_switch 完成（加载可达数十秒，
        不能占住 HTTP 请求 —— 宿主 15s 超时）。
        """
        with self._lock:
            if name not in config.CLIP_MODEL_META:
                raise ValueError(f"未知语义模型档位: {name}")
            p = config.clip_paths(name)
            if not (os.path.isfile(p["vision"]) or os.path.isfile(p["whole"])):
                raise FileNotFoundError(f"语义模型未下载: {p['whole']}")
            config.set_active_clip(name)
            for k in ("clip_vision", "clip_text"):
                self._sessions.pop(k, None)
                self._ready.pop(k, None)
                self._load_errors.pop(k, None)
                self._session_info.pop(k, None)
            return self.clip_models_info()

    def finish_clip_switch(self, name: str) -> bool:
        """后台完成档位切换：确保拆分件 + 加载双塔；失败回退默认档。"""
        with self._lock:
            from .services.clip_subgraph import ensure_subgraphs

            err = ensure_subgraphs(name)
            if err:
                self._load_errors["clip_vision"] = err
                config.set_active_clip(config.CLIP_DEFAULT_MODEL)
                return False
            _ = self.clip_vision
            _ = self.clip_text
            ok = self.is_ready("clip_vision") and self.is_ready("clip_text")
            if not ok:
                config.set_active_clip(config.CLIP_DEFAULT_MODEL)
            return ok

    # ------------------------------------------------------------------
    def set_gpu_enabled(self, enabled: bool) -> dict:
        """切换 GPU 加速：enabled=False 强制 CPU。会话清空后由服务层后台重建。"""
        with self._lock:
            self._gpu_forced_off = not enabled
            self._providers_sel = None if enabled else ["CPUExecutionProvider"]
            self._reload_all()
        return self.gpu_info()

    def _reload_all(self):
        """清空全部会话槽位，下次访问按新 provider 惰性重建。"""
        self._sessions.clear()
        self._ready.clear()
        self._session_info.clear()
        self._load_errors.clear()

    def is_ready(self, key: str) -> bool:
        # 无锁读（dict.get 原子）：调用方为 /health 高频探测，绝不能等加载锁
        return self._ready.get(key, False)

    def preload_main(self) -> dict:
        """启动预热：仅人物检测主链路（每张图必经）。

        CLIP 双塔由首次语义请求触发惰性加载（首次数十秒），避免把服务就绪
        窗口拖长导致宿主等待/误判；face/ocr 为条件触发的专家通道，同样惰性。
        """
        with self._lock:
            self.det
            keys = ["det"]
            return {k: {"ready": self.is_ready(k), "error": self._load_errors.get(k, "")} for k in keys}

    def status(self) -> dict:
        """强制加载全部通道（启动预加载线程 / 测试用）；持锁整个加载过程。"""
        with self._lock:
            self.det
            self.face_det
            self.face_rec
            self.ocr
            keys = ["det", "face_det", "face_rec", "ocr"]
            out = {k: {"ready": self.is_ready(k), "error": self._load_errors.get(k, "")} for k in keys}
            out["clip"] = self.clip_models_info()
            return out


_registry: ModelRegistry | None = None


def get_registry() -> ModelRegistry:
    global _registry
    if _registry is None:
        _registry = ModelRegistry()
    return _registry
