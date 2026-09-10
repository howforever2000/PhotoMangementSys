"""ONNX 模型注册表：集中加载/持有会话，按需惰性初始化。

每个模型一个 session 槽位，缺失时该通道自动降级（is_ready=False），
主流程（分类）不依赖任何可选模型。
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
        # FEAT-051：默认强制 CPU（用户在 UI「检测 GPU → 启用加速」后再切 GPU，
        # 参考单相册扫描面板的交互）；env VCR_PROVIDER=auto 可恢复自动探测
        self._providers_sel: list[str] | None = ["CPUExecutionProvider"]
        self._gpu_forced_off = True
        # FEAT-051：用户指定的分类模型文件名（None = 按 CLS_MODELS 顺序回退）
        # FEAT-052：启动时读取持久化选择（models/current_cls.json），UI 选过的跨重启保持
        self._cls_override: str | None = self._load_persisted_cls()
        # 会话加载/切换锁：后台预加载线程与 FastAPI 线程池并发访问会话槽位，
        # _load 的 check-then-act 必须串行。RLock 允许 status() 内部重入。
        # 注意：is_ready / gpu_info 故意不加锁（/health 每 300ms 被宿主探测，
        # 若等加载锁会被阻塞数分钟 → 宿主误判不可达），仅依赖 dict 读的原子性。
        self._lock = threading.RLock()
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
    def current_cls_name(self) -> str | None:
        """当前登记生效的分类模型名（不触发加载，可被 /health 高频调用）。"""
        current = self._cls_override
        if not current:
            for m in config.CLS_MODELS:
                if os.path.isfile(os.path.join(config.MODEL_DIR, m)):
                    current = m
                    break
        return current

    @staticmethod
    def _session_facts(sess: ort.InferenceSession, path: str, cpu_fallback: bool) -> dict:
        """提取会话实测事实 —— 「确实在用 GPU / 确实换了模型」的铁证。

        sess.get_providers() 是 ONNX Runtime 会话创建后实际绑定的提供方
        （而非我们传入的候选列表），源文件 + 输入元数据可对照确认会话
        确实由该模型文件构建。
        """
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
            "file_size": size,
            "providers": provs,
            "input_name": inp.name if inp is not None else None,
            "input_shape": shape,
            "input_type": inp.type if inp is not None else None,
            "cpu_fallback": cpu_fallback,
        }

    def _create_session(self, path: str, opts: ort.SessionOptions | None = None) -> tuple[ort.InferenceSession, bool]:
        """按当前 provider 候选创建会话；GPU 初始化失败回退纯 CPU 重试。

        返回 (session, cpu_fallback)。回退仅对本会话生效并记录到实测信息，
        不改全局 provider 选择 —— 下次重建仍先尝试用户选择的 provider。
        （此前 GPU 建会话失败会把该通道直接标为不可用，驱动异常时全通道瘫痪。）
        """
        opts = opts or self._so()
        try:
            return ort.InferenceSession(
                path, sess_options=opts, providers=self._providers()
            ), False
        except Exception as gpu_err:  # noqa: BLE001
            prov = self._providers()
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


    def _so_clip(self) -> ort.SessionOptions:
        """CLIP fp16 会话专用 SessionOptions（固定 CPU 推理）。

        - 固定 CPU：AMD DML 对该 fp16 图存在算子级数值 bug（实测输出错误），
          且全量图优化会在 vision 塔初始化时崩溃
        - BASIC 优化在 CPU 上已实测数值正确（与 fp32 基线 cos≈1）
        """
        so = self._so()
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_BASIC
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
            # FEAT-053：各通道会话实测 provider（sess.get_providers()，未加载通道不出现）。
            # use_gpu/provider 是「请求值」，这里是「实测值」—— 两者对照才算真验证。
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
                        # FEAT-053：记录会话实测事实（真实 provider / 源文件 / 输入元数据）
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
    # 语义搜索双塔（Chinese-CLIP fp16，可选）：拆分件由 embed_service.ensure() 生成
    # ------------------------------------------------------------------
    @property
    def clip_vision(self) -> ort.InferenceSession | None:
        with self._lock:
            if "clip_vision" not in self._ready:
                try:
                    sess, cpu_fb = self._create_session(
                        config.CLIP_VISION_PATH, self._so_clip())
                    self._sessions["clip_vision"] = sess
                    self._ready["clip_vision"] = True
                    self._session_info["clip_vision"] = self._session_facts(sess, config.CLIP_VISION_PATH, cpu_fb)
                except Exception as e:  # noqa: BLE001
                    self._load_errors["clip_vision"] = str(e)
                    self._ready["clip_vision"] = False
        return self._sessions.get("clip_vision")

    @property
    def clip_text(self) -> ort.InferenceSession | None:
        with self._lock:
            if "clip_text" not in self._ready:
                try:
                    sess, cpu_fb = self._create_session(
                        config.CLIP_TEXT_PATH, self._so_clip())
                    self._sessions["clip_text"] = sess
                    self._ready["clip_text"] = True
                    self._session_info["clip_text"] = self._session_facts(sess, config.CLIP_TEXT_PATH, cpu_fb)
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
        """图像塔前向 → (N,512) fp32（fp16 图输出已 cast 回 fp32）。"""
        sess = self._sessions["clip_vision"]
        return sess.run(None, {sess.get_inputs()[0].name: pixel_values})[0].astype(np.float32)

    def run_clip_text(self, input_ids: np.ndarray, attention_mask: np.ndarray) -> np.ndarray:
        """文本塔前向 → (N,512) fp32。"""
        sess = self._sessions["clip_text"]
        return sess.run(None, {
            sess.get_inputs()[0].name: input_ids,
            sess.get_inputs()[1].name: attention_mask,
        })[0].astype(np.float32)

    # ------------------------------------------------------------------
    # FEAT-053：固定张量测速 —— CPU/GPU 真实加速比一键对比
    # ------------------------------------------------------------------
    def benchmark(self, key: str = "cls", runs: int = 10, warmup: int = 2) -> dict:
        """预热后计时 N 次推理，输出平均/最快/最慢毫秒与实测 provider。

        测速在锁外进行：会话是不可变对象（切换 = 整体换引用），持锁计时
        反而会被并发模型加载阻塞导致读数失真。若测速期间恰好发生切换，
        读数仍对应当时的旧会话，providers 字段取自该会话本身，不会张冠李戴。
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
        inp = sess.get_inputs()[0]
        # 静态维度用真值，动态维度按图像分类惯例补齐（batch=1 / ch=3 / 空间=224）
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
        tensor = np.random.randn(*shape).astype(np.float32)
        feed = {inp.name: tensor}
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
            "input_shape": shape,
            # 本次测速会话实际绑定的 provider（铁证级，而非请求值）
            "providers": list(sess.get_providers()),
            "avg_ms": round(total / runs, 2),
            "min_ms": round(times[0], 2),
            "max_ms": round(times[-1], 2),
            "total_ms": round(total, 2),
            "throughput_per_s": round(runs / (total / 1000.0), 1) if total > 0 else None,
        }

    # ------------------------------------------------------------------
    # FEAT-051：运行时切换（GPU 加速 / 分类模型），供 UI 按硬件性能选择
    # ------------------------------------------------------------------
    def set_gpu_enabled(self, enabled: bool) -> dict:
        """切换 GPU 加速：enabled=False 强制 CPU。

        provider 在会话创建时绑定，切换后清空全部已加载会话；重建由服务层
        （POST /gpu 处理器）的后台线程完成 —— 加载耗时不可预估，不能阻塞请求，
        重建期间 /health 返回 ok=false，宿主会等待就绪。
        """
        with self._lock:
            self._gpu_forced_off = not enabled
            self._providers_sel = None if enabled else ["CPUExecutionProvider"]
            self._reload_all()
        return self.gpu_info()

    def set_cls_model(self, name: str) -> dict:
        """同步切换分类模型到指定文件（须在候选清单中且已下载）。

        仅测试/脚本用；HTTP /model 走 begin/finish 两步后台加载（见 server.py），
        大模型加载可达数十秒，不能占住 HTTP 请求（宿主 15s 超时）。
        """
        with self._lock:
            info = self.begin_cls_model_switch(name)
            ok = self.finish_cls_model_switch(name)
            if not ok:
                raise RuntimeError(f"模型加载失败: {self._load_errors.get('cls', '加载失败')}")
            return info

    def begin_cls_model_switch(self, name: str) -> dict:
        """FEAT-051：校验并登记切换目标，清空 cls 会话槽位后立即返回。

        实际加载由后台线程 finish_cls_model_switch 完成（对齐 /gpu 的异步模式）；
        加载期间 is_ready("cls")=False → /health ok=false，宿主进入等待而非报错。
        """
        with self._lock:
            if name not in config.CLS_MODEL_META:
                raise ValueError(f"未知分类模型: {name}")
            path = os.path.join(config.MODEL_DIR, name)
            if not os.path.isfile(path):
                raise FileNotFoundError(f"模型文件未下载: {path}")
            self._cls_override = name
            self._sessions.pop("cls", None)
            self._ready.pop("cls", None)
            self._load_errors.pop("cls", None)
            self._session_info.pop("cls", None)
            return self.cls_models_info()

    def finish_cls_model_switch(self, name: str) -> bool:
        """后台加载切换目标；成功持久化选择，失败回退默认候选并清除持久化记录。"""
        with self._lock:
            path = os.path.join(config.MODEL_DIR, name)
            self._load("cls", [path], required=True)
            if self._ready.get("cls"):
                self._persist_cls(name)
                return True
            self._cls_override = None
            self._sessions.pop("cls", None)
            self._ready.pop("cls", None)
            self._session_info.pop("cls", None)
            # 持久化指向的模型加载失败 → 删除记录，下次启动走默认回退
            try:
                os.remove(config.CLS_CURRENT_PATH)
            except OSError:
                pass
            return False

    def cls_models_info(self) -> dict:
        """分类模型候选清单 + 当前生效模型 + 会话实测事实（UI 用）。"""
        current = self.current_cls_name()
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
        # cls 会话是否已就绪（切换后台加载期间为 False，UI 据此提示「加载中」）
        # FEAT-053：loaded = cls 会话实测事实（后台加载期间为 None）；
        # loaded.file 与 current 一致 + loaded.providers 含 GPU 提供方 → 切换/加速确凿生效
        return {
            "models": models,
            "current": current,
            "cls_ready": self.is_ready("cls"),
            "loaded": self._session_info.get("cls"),
        }

    def _reload_all(self):
        """清空全部会话槽位，下次访问按新 provider 惰性重建。"""
        self._sessions.clear()
        self._ready.clear()
        # FEAT-053：实测信息与旧会话同生命周期；同时清掉旧 provider 下的过时错误
        self._session_info.clear()
        self._load_errors.clear()

    def is_ready(self, key: str) -> bool:
        # 无锁读（dict.get 原子）：调用方为 /health 高频探测，绝不能等加载锁
        return self._ready.get(key, False)

    def preload_main(self) -> dict:
        """启动预热：仅主链路（cls + det + scene）。

        这三个通道每张图必经，提前加载让 /health ok 尽快翻转、首批扫描零等待；
        face/ocr/flower/food 为条件触发的专家通道，首次命中时经属性访问惰性
        加载（services 各 has_* 已按 is_ready 优雅降级）。此前全量预热 8 通道
        会把服务就绪拖到数十秒，放大宿主等待窗口。
        """
        with self._lock:
            self.cls
            self.det
            self.scene
            keys = ["cls", "det", "scene"]
            return {k: {"ready": self.is_ready(k), "error": self._load_errors.get(k, "")} for k in keys}

    def status(self) -> dict:
        # 强制加载全部通道（启动预加载线程 / 测试用）；持锁整个加载过程
        with self._lock:
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
