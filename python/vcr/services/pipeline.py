"""服务层：流水线编排（单张图片多路规则推理 → 仲裁 → 结果）

职责边界：
  - 解码图片（一次）→ 分发各通道
  - 收集元信息（格式/EXIF/尺寸）供仲裁器
  - 条件触发人脸标号（仅人物分支，省大量单人半身图开销）
  - 返回 schemas.ClassifyResult

v5：分类模型（cls）/ Places365 场景 / 花朵 / 食物专家通道已全部下线；
图像内容分类改由宿主「语义分类」（Chinese-CLIP）承担，本流水线只负责
人物 / 夜景 / 文档 三条规则通道。单张耗时从 ~437ms 降到 ~150ms。

P-OPT1：通道级并行（本次）
--------------------------
五条通道此前被串成一条线顺序执行。现在把无数据依赖的两组拆成两条并行流：
  CPU 流：tone（纯 numpy + PIL，无 ONNX）
  GPU 流：det → (条件触发) face → ocr（ONNX Runtime）

⚠️ 2026-09-22 实测修正 —— 原始动机里有一条是**错的**，保留在此以免后人重犯：
  原以为「tone 吃 CPU、det/ocr/face 吃 GPU，两边是不同资源，所以能重叠」。
  但本机默认配置下 `registry.gpu_info()` 显示四个会话全部绑定在
  **CPUExecutionProvider**（DmlExecutionProvider 只是「可用」，并未启用），
  即两条流**都在抢同一份 CPU**。所以实测收益远低于最初预估。

  实测（python/bench/tone_share_profile.py，30 张 4096px 原图）：
    tone 占串行总墙钟 **8.5%**（均值 35.8ms/张，中位 42.6ms）
    Amdahl 上限 1/(1-0.085) = **1.09x**；实测 **1.07x**
    ⇒ 并行是真的生效了（达成上限的 98%），只是**上限本身只有 1.09x**。
    真正的耗时大头是 face 42.9% / ocr 24.9% / det 23.5%（都是 ONNX 推理）。

  结论：这条改造**保留**（白拿 ~7%，且已验证输出等价），但别指望它解决吞吐问题。
  想再上一个台阶只能从 ONNX 侧动手（启用 DML / 换更小模型 / 少跑一条通道）。

P-OPT2：并行维度放大到「照片」（2026-09-22）
--------------------------
classify_batch 侧用 ThreadPoolExecutor(PHOTO_PARALLEL) 让 P 张照片同时在飞
（一个线程一张照片），与本函数的两流并行正交叠加：本函数内的 tone‖GPU 流重叠
解决「单张内部」的串行段，照片级并行解决「张与张之间」的解码/推理互斥与 ONNX
同步缝隙。动机、安全论证与实测见 python/bench/photo_parallel_bench.py 与
server.classify_batch 注释；config.PHOTO_PARALLEL 可调（1 = 回到纯串行）。

**为什么这个拆分是安全的（逐条已核实，不要凭直觉改动）**：
  1. ONNX Runtime 的 `InferenceSession.run()` 是线程安全的 —— 多个线程可以并发
     调用同一个 session。`ModelRegistry.run()` 只做 `sess.run(...)`，`_sessions`
     是只读字典查找，因此并发复用无需额外加锁（model_registry.py:291）。
  2. tone 不消费任何其他通道的输出（tone_service.compute_tone 只吃 img），
     因此它可以和 GPU 流完全并发而无需任何同步。
  3. face **依赖** det 的 persons（face_service.process_photo 接收 img 与 photo_path，
     但其内部 detect_faces 用的是 registry 的 face_det 会话；触发条件是 _needs_face(det_out)），
     因此 face 必须留在 GPU 流内、排在 det 之后，**绝不能**外提到另一条流。
  4. `person_store` 走 sqlite 且 register() 是「先 match 再写」的读改写序列，
     **不是线程安全的** ⇒ 必须用 _FACE_STORE_LOCK 串行化写入（见下）。
     （当前 /classify_batch 串行执行所以从未暴露；并行后若不加锁会产生重复 person_id。）
  5. 失败降级语义必须逐字保持：face 抛异常 → face_hits=[]（照旧）；
     tone 失败 → ToneOutcome(ready=False)（照旧，由仲裁器降级）。

线程池刻意用**模块级单例**（不是每张图新建）：每张图新建 ThreadPoolExecutor
会带来 2 个线程的创建/销毁开销（约 50~100µs/张，在 150ms/张 的尺度上虽小，
但属于纯浪费，且高频创建线程对 GC 不友好）。
"""
import os
import threading
import time
from concurrent.futures import ThreadPoolExecutor

from .. import config, preprocess, timing
from ..schemas import ClassifyResult, TopItem
from . import arbitrator, detector, face_service, ocr_service, tone_service

# P-OPT1：通道并行用的线程池（模块级单例，见模块 docstring 末尾说明）。
# workers = max(2, 2 × config.PHOTO_PARALLEL)：P-OPT2 打开照片级并行后，最多
# P 张照片同时各提交一个 tone 任务，固定 2 个工人会让 tone 排队（wall_wait 抬升）
# ⇒ 随照片并行度扩容；P=1 时仍是 2，正好对应「CPU 流 + GPU 流」两条流。
_CHANNEL_POOL = ThreadPoolExecutor(
    max_workers=max(2, 2 * config.PHOTO_PARALLEL), thread_name_prefix="vcr-chan"
)

# P-OPT1：人脸库写入锁。
# person_store.register() = match(读全表) → 可能 INSERT/UPDATE → INSERT faces，
# 是典型的「读-改-写」，两个线程同时进来会双双 match 失败各自新建一个人物，
# 同一个人被拆成 P001/P002。串行批处理掩盖了这个竞态，并行后必须显式加锁。
# 只在**写入**时持锁：人脸检测/对齐/编码（耗时大头）仍在锁外并发。
_FACE_STORE_LOCK = threading.Lock()

def _tone_timed(img) -> tuple[object, float]:
    """compute_tone 的计时包装：返回 (原结果, 耗时ms)。

    ⚠️ 这里**刻意不用 threading.local()**（曾经用过，是个 bug）：
    tone 跑在线程池的**工作线程**上，而记账 `_account` 是在**调用方线程**
    （classify_one 所在线程）执行的 —— `threading.local()` 的命名空间按线程隔离，
    工作线程写进去的 value，调用方线程根本读不到，结果 tone 耗时永远记成 0.0。
    （由 python/bench/tone_share_profile.py 暴露：日志里 tone 恒为 0。）

    改为随结果一起返回耗时，「耗时」与「这张图」天然成对绑定，不依赖线程身份，
    也就不会出现跨图串值 —— 这正是 scan_calib 里那条教训的真正落法。

    只负责计时，**不改变**任何返回语义 —— compute_tone 自己的异常吞并逻辑照旧。
    """
    t = time.perf_counter()
    try:
        out = tone_service.compute_tone(img)
    except Exception as e:  # noqa: BLE001
        # compute_tone 内部已吞异常，这里只兜住极端情况；语义仍是「降级而非中断」
        out = tone_service.ToneOutcome(error=str(e) or "tone 任务异常")
    return out, (time.perf_counter() - t) * 1000.0


def _meta_of(img, path: str) -> dict:
    meta = {"format": "", "has_exif": False}
    try:
        from PIL import Image as _I

        with _I.open(path) as im:
            meta["format"] = (im.format or "").upper()
            meta["has_exif"] = bool(im.getexif())
    except Exception:
        pass
    return meta


def _needs_face(det_out) -> bool:
    """人脸标号条件触发：仅当仲裁器会命中「需要 person_ids」的人物规则分支。

    与 arbitrator 人物规则严格对齐：
      - portrait：最大人框 ≥ PORTRAIT_AREA
      - street：n≥3 且 max_area<STREET_MAX_AREA 且非密集车流
      - 合影：n≥2 且 max_area ≥ GROUP_AREA
    单人小框（路人）不返回 person_ids，跳过人脸标号（省 ~50-100ms/张）。
    """
    if not det_out.ready or det_out.count <= 0:
        return False
    n, max_area = det_out.count, det_out.max_area_ratio
    if max_area >= config.PORTRAIT_AREA:
        return True
    heavy_traffic = (
        getattr(det_out, "vehicle_count", 0) >= config.VEHICLE_HEAVY_N
        and max_area < config.VEHICLE_PERSON_AREA_MAX
    )
    if n >= config.STREET_PERSON_N and max_area < config.STREET_MAX_AREA and not heavy_traffic:
        return True
    if n >= 2 and max_area >= config.GROUP_AREA:
        return True
    return False


def _gpu_channels(img, lb, registry, path: str, use_face: bool) -> tuple:
    """GPU 流：det → (条件) face → ocr。返回 (det_out, ocr_out, face_hits, face_used, 各段耗时)。

    ⚠️ face 必须留在本函数内、排在 det 之后 —— 它的触发条件 `_needs_face` 吃的是
    det 的输出。把它挪到另一条流会读到未初始化的 det 结果（等于随机跳过人脸标号）。

    `img` 是原图（只用于 face_align：对齐必须在原分辨率上取脸）；
    `lb` 是点 3 抽出的**共用缩放**，det/ocr/face 的 letterbox 都从它贴底。

    返回的时间戳是单调递增的 perf_counter，供 _account 复用原有口径。
    """
    t_det_in = time.perf_counter()
    det_out = detector.run(lb, registry)
    t_det = time.perf_counter()

    face_hits: list[dict] = []
    face_used = bool(use_face and _needs_face(det_out))
    if face_used:
        try:
            svc = face_service.get_face_service(registry)
            # 检测 + 对齐 + 编码（耗时大头）不加锁；只有落库那一步需要串行
            faces = svc.detect_faces(lb)
            hits: list[dict] = []
            for f in faces:
                emb = svc.embed(img, f)
                if emb is None:
                    continue
                with _FACE_STORE_LOCK:
                    # 落库单独记：这是持锁的串行段，P 路并行时它会成为争用点
                    with timing.span("face.store"):
                        pid, sim = svc.store.register(emb, path, f"{f.bbox}")
                hits.append({"person_id": pid, "bbox": f.bbox, "sim": round(sim, 3)})
            face_hits = hits
        except Exception:
            face_hits = []          # 与串行版逐字一致的降级语义
    t_face = time.perf_counter()

    ocr_out = ocr_service.get_ocr_service(registry).run(lb)
    t_ocr = time.perf_counter()
    return det_out, ocr_out, face_hits, face_used, (t_det_in, t_det, t_face, t_ocr)


def classify_one(path: str, registry, use_face: bool = True) -> ClassifyResult | None:
    t0 = time.perf_counter()
    img = preprocess.open_image(path)
    if img is None:
        return None
    # 点 3：解码一次 + 缩放一次，det/ocr/face 三通道与影调共用同一份缩放结果。
    # 旧实现每个通道都从**原图**重新 resize 一次 —— 4096→640 实测 37.7ms、
    # 4096→256（影调）实测 35.6ms，同一张解码上排了 4 遍缩放、3 遍纯重复。
    lb = preprocess.letterbox(img)
    t_decode = time.perf_counter()

    meta = _meta_of(img, path)
    t_meta = time.perf_counter()

    # P-OPT1：两条流并发。
    #   CPU 流：影调（从共用缩放派生，与原图直算的 avg_luma 实测最大差 0.009，
    #           NIGHT_LUMA=45 阈值在 53 张实测相册上无一穿越）
    #   GPU 流：det → face → ocr（见 _gpu_channels）
    fut_tone = _CHANNEL_POOL.submit(_tone_timed, lb.base)
    t_submit = time.perf_counter()
    det_out, ocr_out, face_hits, face_used, gts = _gpu_channels(
        img, lb, registry, path, use_face
    )
    t_gpu_done = time.perf_counter()
    # 等 CPU 流收尾。若 tone 比 GPU 流快（常态），这里是零等待。
    try:
        tone_out, tone_ms = fut_tone.result()
    except Exception:
        # 兜住「submit 本身失败」这类极端情况，语义仍等价（降级而非中断整批）。
        tone_out, tone_ms = tone_service.ToneOutcome(error="tone 任务异常"), 0.0
    t_join = time.perf_counter()

    t_det_in, t_det, t_face, t_ocr = gts
    result = arbitrator.arbitrate(
        img, det_out, meta, face_hits, tone_out=tone_out, ocr_out=ocr_out
    )
    t_arb = time.perf_counter()
    elapsed = (t_arb - t0) * 1000.0

    # 4.1 观测：把单张耗时按通道累加（纯记账，不影响返回值）。
    #
    # ⚠️ 口径修正（P-OPT1）：并行后各段**不再相加等于 total** ——
    #   tone 是并发跑的，它有真实耗时但在关键路径上的贡献是「被重叠掉的部分」。
    #   这里仍然记 tone 的**实际计算耗时**（便于知道它有多重），
    #   同时新增 wall 口径的三个值，让「通道合计 vs total」的差额可解释：
    #     wall_gpu  = 提交到 GPU 流收尾（含 det/face/ocr 全链）
    #     wall_wait = GPU 流收尾到 join（纯等待 tone 的剩余时间，常态≈0）
    #   若把 tone 也算进「通道合计」，与 total 的差额应≈ wall_wait。
    try:
        ms = lambda a, b: (b - a) * 1000.0  # noqa: E731
        _account(
            decode=ms(t0, t_decode),
            meta=ms(t_decode, t_meta),
            submit=ms(t_meta, t_submit),
            det=ms(t_det_in, t_det),
            tone=tone_ms,                           # 由 _tone_timed 随结果返回，见其注释
            ocr=ms(t_face, t_ocr),
            face=ms(t_det, t_face),
            arb=ms(t_join, t_arb),
            wall_gpu=ms(t_submit, t_gpu_done),
            wall_wait=ms(t_gpu_done, t_join),
            total=elapsed,
            face_used=face_used,
        )
    except Exception:
        pass

    return ClassifyResult(
        path=path,
        file_name=os.path.basename(path),
        category=result.category,
        sub_category=result.sub_category,
        label=result.label,
        confidence=round(result.confidence, 4),
        top3=[TopItem(category=result.category, label=result.label,
                      confidence=round(result.confidence, 4))] if result.label else [],
        person_ids=result.person_ids,
        person_count=result.person_count,
        source=result.source,
        elapsed_ms=round(elapsed, 1),
    )


# ---------------------------------------------------------------------------
# 4.1 观测：单张通道耗时累加器（线程安全；仅用于批次汇总日志）
#
# 为什么需要它：现有日志只记「一张图总共多少 ms」，无法回答「这 500ms 花在哪条
# 通道上」。这里把 decode / meta / det / tone / ocr / face / arb 分别累加，
# 由 server 的 /classify_batch 在整批结束时一次性打印——一行看清成本结构。
#
# 刻意不做的事：不落盘、不暴露接口、不参与任何判断分支。出问题最多是少一行日志。
# ---------------------------------------------------------------------------
_STAT_LOCK = threading.Lock()
_STAT: dict[str, float] = {}
_STAT_N = 0


def _account(**kw) -> None:
    global _STAT_N
    with _STAT_LOCK:
        _STAT_N += 1
        for k, v in kw.items():
            if k == "face_used":
                _STAT["face_used"] = _STAT.get("face_used", 0.0) + (1.0 if v else 0.0)
            else:
                _STAT[k] = _STAT.get(k, 0.0) + float(v)


def take_channel_stats() -> tuple[dict[str, float], int]:
    """取出并清空累加器 → (各通道合计 ms, 样本张数)。"""
    global _STAT_N
    with _STAT_LOCK:
        stats, n = dict(_STAT), _STAT_N
        _STAT.clear()
        _STAT_N = 0
        return stats, n
