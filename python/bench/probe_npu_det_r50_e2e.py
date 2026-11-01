# -*- coding: utf-8 -*-
"""NPU 两条腿（det + face_rec）端到端探针 —— 判断「值不值 45MB 组件」的唯一依据。

为什么需要它
------------
FEAT-133 只把 clip_vision 搬上 NPU，结论是「快 2.9× 但只占扫描 0.2%，用户感知为零」。
本轮（2026-11-02）复测 12 个 ONNX 后，唯一**推翻旧结论**的是 `w600k_r50`
（NPU 5.99ms < iGPU 8.44ms，旧实测是 iGPU 赢）。而 `yolov8n-det` 是唯一
**每张必跑**的大前向。所以本轮要回答的不是「NPU 快不快」，而是：

    det + face_rec 搬上 NPU，AI 腿的分摊墙钟到底降多少？

口径纪律（跨表比较前必读）
--------------------------
1. 单位是 **分摊墙钟 ms/张**（整批墙钟 / 张数），与
   `docs/批量扫描入库-技术现状与性能分析.md` 表 A 同口径；单张真实延迟约 ×P。
2. `face.*` 段只统计**自己被调用**的次数（`timing.record` 带 cnt），
   因为人脸链只在 `_needs_face` 命中时跑。
3. 每轮都跑全量样本，取**最好一轮**（排除首批懒加载与偶发 IO 抖动）。
4. 每轮之间不清人物库、不重建会话：只量稳态吞吐，不量冷启动
   （冷启动另有 `verify_cold_start.py`）。

用法
----
    set VCR_OVPROBE=1
    python python/bench/probe_npu_det_r50_e2e.py "D:\\YUAN HAO\\Pictures\\2026\\test" ^
        --rounds 3 --photos-parallel 6 --modes cpu,dml,npu,npu_dml

    # 只做数值等价（det 框 / r50 余弦），不跑吞吐
    python python/bench/probe_npu_det_r50_e2e.py <相册> --equiv-only --limit 12
"""
import argparse
import glob
import json
import os
import shutil
import sys
import tempfile
import threading
import time
import types
from concurrent.futures import ThreadPoolExecutor

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)

EXTS = (".jpg", ".jpeg", ".png", ".webp", ".bmp")
OVPROBE = os.path.join(ROOT, ".tmp-ovprobe")


# ---------------------------------------------------------------------------
# 静态 shape 表：NPU 只吃静态 shape，动态维会让 NPU 编译器 LLVM abort
# **把整个进程带走**（FEAT-133 实测触发两次，Python 侧不可 catch）。
# 这两个通道的真实输入恒为定长（det 见 preprocess.det_tensor，r50 见 face_align），
# 所以钉死是安全的；将来换模型必须先确认新模型输入也是定长。
#
# ⚠️ 形状必须从 `preprocess.py` 读，**不要抄外部性能报告**：本仓库 OCR 喂的是
# 640（与 det 共用同一份 letterbox 缩放），而多数公开报告测的是 960。
# 第一版按 960 写，结果 ocr 每张都抛 shape 异常、被 ocr_service 的
# `except Exception` 吞成 OcrOutcome(error=...)，于是“ocr 变快了 18ms”——
# 实际上它一次都没跑。静默失败会直接把结论反过来。
# ---------------------------------------------------------------------------
STATIC_SHAPE = {
    "det": (1, 3, 640, 640),
    "face_rec": (1, 3, 112, 112),
    "ocr": (1, 3, 640, 640),
}

# 每个 OV 通道对应的 timing 段名 —— 用来核对「这一轮它到底跑了几次」
OV_SPAN = {"det": "det.run", "face_rec": "face.rec", "ocr": "ocr.run"}

OV_CHANNELS = ("det", "face_rec")


def log(msg: str) -> None:
    print(msg, flush=True)


# ---------------------------------------------------------------------------
# 计时锁：与 pipeline_cost_probe.TimedLock 同构，用来回答
# 「NPU 单队列是不是新瓶颈」（持锁占比 = 队列饱和度）
# ---------------------------------------------------------------------------
class TimedLock:
    def __init__(self, inner=None):
        self.inner = inner or threading.Lock()
        self._m = threading.Lock()
        self.hold = 0.0
        self.wait = 0.0
        self.n = 0
        self._t = threading.local()

    def __enter__(self):
        t0 = time.perf_counter()
        self.inner.acquire()
        w = time.perf_counter() - t0
        self._t.hold = time.perf_counter()
        with self._m:
            self.wait += w
            self.n += 1
        return self

    def __exit__(self, *exc):
        d = time.perf_counter() - self._t.hold
        with self._m:
            self.hold += d
        self.inner.release()
        return False


# ---------------------------------------------------------------------------
# ORT 会话的最小鸭子垫片：让 registry.run() 原样工作
# （FEAT-133 踩过：不给垫片会导致 /benchmark 报「通道不可用」）
# ---------------------------------------------------------------------------
class OvSess:
    def __init__(self, cm, device: str, path: str, lock: TimedLock):
        self.cm = cm
        self.device = device
        self.path = path
        self._lock = lock
        self._in = [types.SimpleNamespace(name=i.get_any_name(), shape=None) for i in cm.inputs]
        self._names = [o.get_any_name() for o in cm.outputs]
        # 完整性计数：calls = 真的跑成功几次；fails = 抛了几次。
        # 没有它，一个形状写错的通道会「静默失败→变快→结论反过来」。
        self.calls = 0
        self.fails = 0
        self.last_error = ""

    def get_inputs(self):
        return self._in

    def get_providers(self):
        return [f"OpenVINO:{self.device}"]

    def get_session_options(self):
        return types.SimpleNamespace(intra_op_num_threads=0)

    def run(self, _outputs, feed):
        # NPU 单队列：与 DML 一样必须串行（但用的是**另一把锁**，
        # 两个引擎才能真并行 —— 一把锁会让「iGPU 与 NPU 独立」这句话失效）
        try:
            with self._lock:
                res = self.cm(feed)
        except Exception as e:  # noqa: BLE001
            self.fails += 1
            self.last_error = f"{type(e).__name__}: {str(e)[:180]}"
            raise
        self.calls += 1
        return [np.asarray(res[n]) for n in self._names]


def build_ov(cm_cache: dict, key: str, path: str, device: str, cache_dir: str,
             lock: TimedLock) -> OvSess:
    import openvino as ov

    if key in cm_cache:
        return OvSess(cm_cache[key], device, path, lock)

    core = ov.Core()
    core.set_property({"CACHE_DIR": cache_dir})
    t0 = time.perf_counter()
    model = ov.convert_model(path)
    if len(model.inputs) != 1:
        raise RuntimeError(f"{key} 输入数不为 1，静态化策略需重写：{len(model.inputs)}")
    inp = model.inputs[0]
    shape = STATIC_SHAPE[key]
    # 静态化是硬前置条件：先断言目标 shape 全为常量，再 reshape
    assert all(isinstance(d, int) and d > 0 for d in shape), shape
    model.reshape({inp.get_any_name(): shape})
    cm = core.compile_model(model, device)
    dt = time.perf_counter() - t0
    log(f"  [OV] {key} → {device} | 编译 {dt:.1f}s | 输入 {inp.get_any_name()} "
        f"{tuple(model.inputs[0].get_partial_shape())} {model.inputs[0].get_element_type()}")
    cm_cache[key] = cm
    return OvSess(cm, device, path, lock)


def install_ov(reg, mr, ov_sessions: dict, paths: dict) -> None:
    """把 OV 通道塞进 registry：只覆盖 det / face_rec 两个槽位。"""
    for key, sess in ov_sessions.items():
        reg._sessions[key] = sess
        reg._ready[key] = True
        try:
            size = os.path.getsize(paths[key])
        except OSError:
            size = None
        reg._session_info[key] = {
            "file": os.path.basename(paths[key]),
            "file_path": paths[key],
            "threads": None,
            "file_size": size,
            "providers": [f"OpenVINO:{sess.device}"],
            "input_name": sess.get_inputs()[0].name,
            "input_shape": list(STATIC_SHAPE[key]),
            "input_type": "float32",
            "cpu_fallback": False,
        }
        reg._load_errors.pop(key, None)

    # run() 分流：OV 通道走自己的锁，绝不复用 _GPU_RUN_LOCK
    if not getattr(mr.ModelRegistry, "_ovprobe_patched", False):
        orig = mr.ModelRegistry.run

        def patched(self, key, tensor):
            sess = self._sessions.get(key)
            if isinstance(sess, OvSess):
                return sess.run(None, {sess.get_inputs()[0].name: tensor})
            return orig(self, key, tensor)

        mr.ModelRegistry.run = patched
        mr.ModelRegistry._ovprobe_patched = True


def collect_paths(src: str, limit: int) -> list[str]:
    if src.endswith(".json"):
        paths = json.load(open(src, encoding="utf-8"))
    else:
        paths = sorted(p for p in glob.glob(os.path.join(src, "**", "*"), recursive=True)
                       if p.lower().endswith(EXTS))
    paths = [p for p in paths if os.path.isfile(p)]
    if limit and len(paths) > limit:
        step = max(1, len(paths) // limit)
        paths = paths[::step][:limit]
    return paths


# ---------------------------------------------------------------------------
# 数值等价：换执行设备就是换数值，必须先证「没变」
# ---------------------------------------------------------------------------
def run_equiv(paths: list[str]) -> int:
    import onnxruntime as ort
    import numpy as np

    from vcr import config, preprocess
    from vcr.services import detector

    det_p = os.path.join(config.MODEL_DIR, config.DET_MODEL)
    rec_p = os.path.join(config.MODEL_DIR, config.face_rec_models()[0])
    fdet_p = os.path.join(config.MODEL_DIR, config.face_det_models()[0])

    so = ort.SessionOptions()
    so.intra_op_num_threads = config.threads()
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL

    log(f"[equiv] 设备 CPU（基准） 线程={config.threads()} 样本={len(paths)} 张")
    s_det = ort.InferenceSession(det_p, sess_options=so, providers=["CPUExecutionProvider"])
    s_rec = ort.InferenceSession(rec_p, sess_options=so, providers=["CPUExecutionProvider"])
    s_fdet = ort.InferenceSession(fdet_p, sess_options=so, providers=["CPUExecutionProvider"])

    nlock = TimedLock()
    cache = {}
    ov_det = build_ov(cache, "det", det_p, "NPU", OV_CACHE, nlock)
    ov_rec = build_ov(cache, "face_rec", rec_p, "NPU", OV_CACHE, nlock)

    det_rows, rec_cos, rec_abs = [], [], []
    for i, p in enumerate(paths):
        img, orig = preprocess.open_image_ex(p)
        if img is None:
            continue
        lb = preprocess.letterbox(img, orig)
        tensor, scale, px, py = preprocess.det_tensor(lb)

        o_cpu = s_det.run(None, {s_det.get_inputs()[0].name: tensor})[0][0]
        o_npu = ov_det.run(None, {ov_det.get_inputs()[0].name: tensor})[0][0]
        d = float(np.max(np.abs(o_cpu.astype(np.float64) - o_npu.astype(np.float64))))

        b_cpu = detector._decode(o_cpu, (detector.PERSON_CLASS_ID,), scale, px, py)
        b_npu = detector._decode(o_npu, (detector.PERSON_CLASS_ID,), scale, px, py)
        # 框级比对：按 IoU≥0.9 配对，统计「只在一边出现」的框数
        matched, only = 0, 0
        used = set()
        for bc in b_cpu:
            best, bi = 0.0, -1
            for j, bn in enumerate(b_npu):
                if j in used:
                    continue
                ix1 = max(bc.x1, bn.x1); iy1 = max(bc.y1, bn.y1)
                ix2 = min(bc.x2, bn.x2); iy2 = min(bc.y2, bn.y2)
                inter = max(0.0, ix2 - ix1) * max(0.0, iy2 - iy1)
                a1 = (bc.x2 - bc.x1) * (bc.y2 - bc.y1)
                a2 = (bn.x2 - bn.x1) * (bn.y2 - bn.y1)
                iou = inter / (a1 + a2 - inter) if (a1 + a2 - inter) > 0 else 0.0
                if iou > best:
                    best, bi = iou, j
            if bi >= 0 and best >= 0.9:
                matched += 1
                used.add(bi)
            else:
                only += 1
        only += len(b_npu) - len(used)
        det_rows.append((os.path.basename(p), len(b_cpu), len(b_npu), matched, only, d))

        # 人脸编码：CPU vs NPU 余弦（同一批真人脸）
        ft, fscale, fpx, fpy = preprocess.face_det_tensor(lb)
        fout = s_fdet.run(None, {s_fdet.get_inputs()[0].name: ft})
        faces = _scrfd(fout, fscale, fpx, fpy, lb, config)
        for f in faces[:4]:
            ten = preprocess.face_align(img, f["kps"])
            e_cpu = s_rec.run(None, {s_rec.get_inputs()[0].name: ten})[0][0].astype(np.float64)
            e_npu = ov_rec.run(None, {ov_rec.get_inputs()[0].name: ten})[0][0].astype(np.float64)
            e_cpu /= (np.linalg.norm(e_cpu) + 1e-12)
            e_npu /= (np.linalg.norm(e_npu) + 1e-12)
            rec_cos.append(float(e_cpu @ e_npu))
            rec_abs.append(float(np.max(np.abs(e_cpu - e_npu))))

    log("\n[equiv] det（yolov8n-det）CPU vs NPU 原始输出/解码框")
    bad = [r for r in det_rows if r[4] > 0]
    for r in det_rows[:8]:
        log(f"  {r[0]:<28} 人框 {r[1]}→{r[2]}  配对 {r[3]}  仅在一边 {r[4]}  max|Δ| {r[5]:.5f}")
    log(f"  合计 {len(det_rows)} 张 · 有人框不一致的 {len(bad)} 张 · "
        f"最大 max|Δ| {max((r[5] for r in det_rows), default=0):.5f}")

    if rec_cos:
        import statistics as st
        log("\n[equiv] face_rec（w600k_r50）CPU vs NPU")
        log(f"  样本 {len(rec_cos)} 张脸 · cos 最小 {min(rec_cos):.8f} · "
            f"中位 {st.median(rec_cos):.8f} · max|Δ| 最大 {max(rec_abs):.6f}")
        ok = min(rec_cos) >= 0.9999
        log(f"  门槛 cos ≥ 0.9999 → {'通过' if ok else '未通过（不可上 NPU）'}")
        return 0 if ok and not bad else 1
    log("\n[equiv] 没检出人脸，无法验证 r50（换一组有脸的样本）")
    return 1


def _scrfd(outputs, scale, pad_x, pad_y, lb, config):
    """复用 face_service 的解码，但只保留 bbox/kps，避免这里再抄一遍阈值逻辑。"""
    from vcr.services.face_service import FaceService, face_geometry_quality, QUALITY_REJECT

    faces = FaceService._decode_scrfd(outputs, scale, pad_x, pad_y)
    w, h = lb.size
    kept = []
    for f in faces:
        bw, bh = f.bbox[2] - f.bbox[0], f.bbox[3] - f.bbox[1]
        if min(bw, bh) < config.FACE_MIN_PIX:
            continue
        if f.bbox[0] < 0 or f.bbox[1] < 0 or f.bbox[2] > w or f.bbox[3] > h:
            continue
        if face_geometry_quality(f.bbox, f.kps) == QUALITY_REJECT:
            continue
        kept.append({"bbox": f.bbox, "kps": f.kps, "score": f.score})
    kept.sort(key=lambda d: d["score"], reverse=True)
    return kept


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("src", help="图片目录或 paths.json")
    ap.add_argument("--limit", type=int, default=0, help="0 = 全部")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--warmup", type=int, default=12)
    ap.add_argument("--photos-parallel", type=int, default=6)
    ap.add_argument("--modes", default="cpu,dml,npu,npu_dml")
    ap.add_argument("--ov-device", default="NPU")
    ap.add_argument("--equiv-only", action="store_true")
    ap.add_argument("--runtime", choices=("auto", "probe", "component"), default="auto",
                    help="用哪份 OV 运行时：probe=python/.tmp-ovprobe（开发），"
                         "component=models/_runtime/ovrt（应用内下载的裁剪组件），"
                         "auto=有 probe 就用它")
    ap.add_argument("--ov-channels", default="det,face_rec",
                    help="哪些通道交给 OpenVINO（默认 det,face_rec；ocr 可加进来做对照）")
    ap.add_argument("--threads", type=int, default=0,
                    help="覆盖 ONNX intra_op 线程数（0 = 用落盘设置）。仓库实测最优是 8；"
                         "P=6×12 线程会把 CPU 基线压到失真，跨模式对比前先对齐")
    args = ap.parse_args()

    global OV_CACHE
    OV_CACHE = os.path.join(HERE, "out", "ov_cache_npuprobe")
    os.makedirs(OV_CACHE, exist_ok=True)

    # 探针会写人脸库：默认落临时 VCR_DATA_DIR，绝不碰真实 persons.db
    if not os.environ.get("VCR_DATA_DIR"):
        tmp = os.path.join(tempfile.gettempdir(), "_tmp_vcrdata_npuprobe")
        shutil.rmtree(tmp, ignore_errors=True)
        os.makedirs(os.path.join(tmp, "data"), exist_ok=True)
        os.environ["VCR_DATA_DIR"] = os.path.join(tmp, "data")
        log(f"[probe] VCR_DATA_DIR → {os.environ['VCR_DATA_DIR']}（临时，不碰真实人物库）")
    os.environ.setdefault("VCR_PHOTO_PARALLEL", str(args.photos_parallel))

    if not os.path.isdir(OVPROBE) and args.runtime == "probe":
        log(f"缺少 OV 运行时：pip install --target {OVPROBE} openvino")
        return 2
    if args.runtime != "component":
        sys.path.append(OVPROBE)   # append 而不是 insert：让 venv 的 numpy 优先

    import numpy as np  # noqa: F401

    # 让 ov_backend 把运行时路径注入 sys.path（含「组件 zip 首次解压」这条路），
    # 后面 build_ov 的 `import openvino` 才能成功；顺便把用的是哪份运行时打出来
    from vcr import ov_backend as _ob
    _dev = _ob.probe_devices()
    log(f"[probe] OV 运行时 source={_dev['source'] or '-'} devices={_dev['devices']}"
        f" installed={_dev['installed']} err={_dev['error'] or '-'}")

    paths = collect_paths(args.src, args.limit)
    if not paths:
        log("没有可用图片")
        return 2
    log(f"[probe] 样本 {len(paths)} 张 · src={args.src}")

    if args.equiv_only:
        return run_equiv(paths[:args.limit] if args.limit else paths)

    import vcr.model_registry as mr
    from vcr import config, timing
    from vcr.model_registry import get_registry
    from vcr.services.pipeline import classify_one, take_channel_stats

    if args.threads > 0:
        # 只改本进程内的读数，**不写 current_threads.json**（探针不得改用户设置）
        _n = max(config.THREADS_MIN, min(config.THREADS_MAX, args.threads))
        config.threads = lambda: _n   # noqa: E731
        log(f"[probe] intra_op 线程已覆盖为 {_n}（不落盘）")

    P = max(1, min(8, args.photos_parallel))
    reg = get_registry()
    chan_files = {
        "det": os.path.join(config.MODEL_DIR, config.DET_MODEL),
        "face_rec": os.path.join(config.MODEL_DIR, config.face_rec_models()[0]),
        "ocr": os.path.join(config.MODEL_DIR, config.OCR_MODEL),
    }
    ov_keys = [c.strip() for c in args.ov_channels.split(",") if c.strip() in STATIC_SHAPE]
    ov_paths = {k: chan_files[k] for k in ov_keys}

    log(f"[probe] threads={config.threads()} P={P} decode_min={config.DECODE_MIN_LONG} "
        f"ocr_gate={config.OCR_GATE} face_tier={config.active_face_tier()} "
        f"clip_tier={config.active_clip()}")
    log(f"[probe] gpu_info={reg.gpu_info()['available']}")

    # OV 只编译一次，两个 npu 模式复用（编译成本不该算进任何一个模式）
    npu_lock = TimedLock()
    cm_cache: dict = {}
    ov_sessions: dict = {}
    want_npu = [m for m in args.modes.split(",") if m.strip() in ("npu", "npu_dml")]
    if want_npu:
        log(f"\n[probe] 预编译 {ov_keys} → {args.ov_device}")
        for key in ov_keys:
            ov_sessions[key] = build_ov(cm_cache, key, ov_paths[key], args.ov_device,
                                        OV_CACHE, npu_lock)

    GROUPS = {
        "det": ("det.pre", "det.run", "det.post"),
        "face": ("face.pre", "face.det", "face.post", "face.align", "face.rec", "face.store"),
        "ocr": ("ocr.pre", "ocr.run", "ocr.post"),
    }

    # 模式分两类：
    #   npu / npu_dml  —— 探针自己把 OV 会话塞进 registry（monkeypatch，用于早期验证）
    #   ov  / ov_dml   —— **走真实代码路径**（config.OV_CHANNELS + _load → _try_ov），
    #                     这才是要落地的实现，数字必须与上方对得上
    NPU_MODES = {"npu", "npu_dml"}
    OV_MODES = {"ov", "ov_dml"}
    DML_MODES = {"dml", "npu_dml", "ov_dml"}
    # FEAT-136：走**加速分配表**（GPU 与 NPU 各有对象）
    # 刻意不用 npu* 前缀：early 判定曾用 startswith("npu")，会把 accel 模式
    # 误当成探针自己的 monkeypatch 模式，校验对象串成另一批会话（踩过）。
    ACCEL_MODES = {
        "accel_gpu": "det=NPU,ocr=NPU,face_rec=NPU,face_det=GPU",
        "accel_cpu": "det=NPU,ocr=NPU,face_rec=NPU,face_det=CPU",
        "gpu_all": "det=GPU,ocr=GPU,face_rec=GPU,face_det=GPU",
    }

    def _patch_dev_lock():
        """真实路径的设备锁是 ov_backend.device_lock()；换成计时锁才能量 NPU 饱和度。"""
        from vcr import ov_backend

        def _timed(dev):
            key = str(dev).upper()
            if key not in _TLOCKS:
                _TLOCKS[key] = TimedLock()
            return _TLOCKS[key]

        ov_backend.device_lock = _timed
        return _TLOCKS.setdefault(args.ov_device.upper(), TimedLock())

    _TLOCKS: dict = {}

    results: list[dict] = []
    for mode in args.modes.split(","):
        mode = mode.strip()
        if not mode:
            continue
        if mode in NPU_MODES and not ov_sessions:
            continue

        use_dml = mode in DML_MODES
        if mode in ACCEL_MODES:
            # 分配表：靠 env 覆盖（不写 current_accel.json，不污染用户设置）
            os.environ["VCR_CHANNEL_DEVICES"] = ACCEL_MODES[mode]
            npu_lock = _patch_dev_lock()
            reg._reload_all()
            log(f"  [accel] {os.environ['VCR_CHANNEL_DEVICES']}")
        else:
            os.environ.pop("VCR_CHANNEL_DEVICES", None)
        if mode in OV_MODES:
            # 真实开关：只在本次运行内改，不写任何配置文件
            config.OV_CHANNELS = list(ov_keys)
            npu_lock = _patch_dev_lock()
            log(f"  [ov] config.OV_CHANNELS={config.OV_CHANNELS} device={config.OV_DEVICE}")
        if mode not in ACCEL_MODES:
            # 1) 先按模式选 provider（这一步会清空所有会话）
            reg.set_gpu_enabled(use_dml)
        # 2) npu* 模式再把两条腿盖成 OV（必须在 set_gpu_enabled 之后，否则被清掉）
        if mode in NPU_MODES:
            install_ov(reg, mr, ov_sessions, ov_paths)
        # 3) 其余通道惰性加载（ov* 模式就是在这一步由 _load → _try_ov 接上 OV）
        _ = reg.det, reg.face_det, reg.face_rec, reg.ocr

        # 完整性校验的对象：本模式下**真正拿到 OV 会话**的通道
        from vcr import ov_backend as _ovb
        check_sessions = {k: v for k, v in reg._sessions.items()
                          if isinstance(v, _ovb.OvChannel)}
        if mode in NPU_MODES:
            check_sessions = dict(ov_sessions)
        prov = {k: reg._session_info.get(k, {}).get("providers") for k in
                ("det", "face_det", "face_rec", "ocr")}
        log(f"\n===== 模式 {mode} =====")
        for k, v in prov.items():
            log(f"  {k:<9} {v}")

        # 预热（不计入）：模型懒加载 + NPU/DML 首轮提交开销
        npu_lock.hold = npu_lock.wait = 0.0
        npu_lock.n = 0
        with ThreadPoolExecutor(P) as ex:
            list(ex.map(lambda p: classify_one(p, reg, use_face=True), paths[:args.warmup]))
        timing.take()
        take_channel_stats()

        best = None
        for r in range(args.rounds):
            npu_lock.hold = npu_lock.wait = 0.0
            npu_lock.n = 0
            for _s in check_sessions.values():
                _s.calls = _s.fails = 0
            lock = TimedLock(mr._GPU_RUN_LOCK)
            mr._GPU_RUN_LOCK = lock
            try:
                take_channel_stats()
                timing_acc, timing_cnt = timing.take()
                t0 = time.perf_counter()
                with ThreadPoolExecutor(P) as ex:
                    list(ex.map(lambda p: classify_one(p, reg, use_face=True), paths))
                wall = (time.perf_counter() - t0) * 1000.0
            finally:
                mr._GPU_RUN_LOCK = lock.inner
            stats, n = take_channel_stats()
            t_acc, t_cnt = timing.take()
            # 完整性校验：OV 通道「实际成功次数」必须等于它那段的调用次数。
            # 不等 = 这个通道在静默失败（被服务层的 except 吞掉），读数不可用。
            invalid = []
            for k, s in (check_sessions.items()
                          if mode in (NPU_MODES | OV_MODES) or mode in ACCEL_MODES else ()):
                expect = t_cnt.get(OV_SPAN[k], 0)
                if s.calls != expect:
                    invalid.append(
                        f"{k} 实跑 {s.calls}/期望 {expect}，抛错 {s.fails} 次 {s.last_error}")
            row = {
                "mode": mode, "wall": wall, "n": len(paths), "invalid": invalid,
                "wall_per": wall / len(paths),
                "ch": {k: v / max(n, 1) for k, v in stats.items()},
                "span": t_acc, "span_n": t_cnt,
                "dml_hold": lock.hold * 1000, "dml_wait": lock.wait * 1000, "dml_n": lock.n,
                "npu_hold": npu_lock.hold * 1000, "npu_wait": npu_lock.wait * 1000,
                "npu_n": npu_lock.n,
            }
            log(f"  第{r + 1}轮 wall {row['wall_per']:6.1f} ms/张 | "
                f"det {row['ch'].get('det', 0):5.1f} · ocr {row['ch'].get('ocr', 0):5.1f} · "
                f"face {row['ch'].get('face', 0):5.1f} · decode {row['ch'].get('decode', 0):5.1f} | "
                f"DML 持锁 {row['dml_hold'] / wall * 100:5.1f}%({row['dml_n']}次) · "
                f"NPU 持锁 {row['npu_hold'] / wall * 100:5.1f}%({row['npu_n']}次)")
            if invalid:
                log("    ⚠️ 本轮有通道静默失败，读数作废：" + " | ".join(invalid))
            # 只接受「每个 OV 通道都真跑过」的轮次
            if not invalid and (best is None or row["wall_per"] < best["wall_per"]):
                best = row
        if best is None:
            log(f"  ✗ 模式 {mode} 全部轮次都有通道静默失败，不入表")
            continue
        results.append(best)

        # 最好一轮的模型段明细（均摊到该段自己的调用次数）
        span, cnt = best["span"], best["span_n"]
        for gname, keys in GROUPS.items():
            parts = []
            for k in keys:
                if cnt.get(k, 0) > 0:
                    parts.append(f"{k.split('.')[1]} {span[k] / cnt[k]:.1f}")
            calls = max((cnt.get(k, 0) for k in keys), default=0)
            if parts:
                log(f"    {gname:<5} x{calls:<4} " + "/".join(parts))
        log(f"    模型段合计 {sum(span.values()) / best['n']:.1f} ms/张 · "
            f"通道合计 {sum(best['ch'].values()):.1f} ms/张")

    # ------------------------------------------------------------------
    log("\n===== 汇总（最好一轮，分摊墙钟 ms/张）=====")
    base = next((r for r in results if r["mode"] == "cpu"), None)
    log(f"{'模式':<10}{'墙钟':>9}{'vs cpu':>9}{'det':>8}{'ocr':>8}{'face':>8}"
        f"{'decode':>9}{'DML锁%':>9}{'NPU锁%':>9}")
    for r in results:
        rel = f"{(r['wall_per'] / base['wall_per'] - 1) * 100:+.1f}%" if base else "-"
        log(f"{r['mode']:<10}{r['wall_per']:>9.1f}{rel:>9}{r['ch'].get('det', 0):>8.1f}"
            f"{r['ch'].get('ocr', 0):>8.1f}{r['ch'].get('face', 0):>8.1f}"
            f"{r['ch'].get('decode', 0):>9.1f}"
            f"{r['dml_hold'] / r['wall'] * 100:>9.1f}{r['npu_hold'] / r['wall'] * 100:>9.1f}")

    # 关键子段：det.run / face.rec 的净变化
    log("\n===== 关键 ONNX 段（该段自己的单次耗时 ms）=====")
    log(f"{'模式':<10}{'det.run':>9}{'face.det':>10}{'face.rec':>10}{'ocr.run':>9}")
    for r in results:
        sp, cn = r["span"], r["span_n"]
        f = lambda k: (sp[k] / cn[k]) if cn.get(k, 0) else 0.0  # noqa: E731
        log(f"{r['mode']:<10}{f('det.run'):>9.1f}{f('face.det'):>10.1f}"
            f"{f('face.rec'):>10.1f}{f('ocr.run'):>9.1f}")

    out = os.path.join(HERE, "out", "probe_npu_det_r50_e2e.json")
    with open(out, "w", encoding="utf-8") as fh:
        json.dump([{k: v for k, v in r.items() if k not in ("span", "span_n")} |
                   {"span": r["span"], "span_n": r["span_n"]} for r in results],
                  fh, ensure_ascii=False, indent=1, default=float)
    log(f"\n落盘 → {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
