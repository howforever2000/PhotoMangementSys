"""VCR 全局配置：路径 / 阈值 / 模型档位

所有硬编码集中于此，服务层只引用 config 常量。

v5（语义分类）：**分类模型（yolov8*-cls / Places365 / 花朵 / 食物专家）已下线**，
图像内容分类改由 Chinese-CLIP 语义匹配（宿主侧 photo_categories + 关键词）承担。
本服务只保留三条规则通道（都不是"分类模型"）：
  det   —— YOLOv8n-det 人物检测（人物/扫街）
  tone  —— 影调自算（夜景）
  ocr   —— PaddleOCR det（文档）
外加 CLIP 双塔（语义向量 / 文本关键词编码）。
"""
import json
import os
import struct
import sys


# ---------------------------------------------------------------------------
# 路径
# ---------------------------------------------------------------------------
def _project_dir() -> str:
    """返回微服务根目录（其下应含 models/、data/）。

    解析优先级：
      1. 环境变量 VCR_ROOT —— 部署时由宿主（Tauri/MSI）显式指定；
      2. PyInstaller 打包后（sys.frozen）—— 可执行文件所在目录，
         即部署布局 <install>/vcr/vcr-server.exe + 旁侧的 models/、data/；
      3. 开发态 —— python/ 目录（__file__ 位于 python/vcr/ 下）。
    """
    env = os.environ.get("VCR_ROOT") or ""
    if env:
        return env
    if getattr(sys, "frozen", False):
        return os.path.dirname(os.path.abspath(sys.executable))
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


PROJECT_DIR = _project_dir()
MODEL_DIR = os.environ.get("VCR_MODEL_DIR") or os.path.join(PROJECT_DIR, "models")
DATA_DIR = os.environ.get("VCR_DATA_DIR") or os.path.join(PROJECT_DIR, "data")
PERSONS_DB = os.path.join(DATA_DIR, "persons.db")

# ---------------------------------------------------------------------------
# 规则通道模型（缺失则对应通道自动降级）
# ---------------------------------------------------------------------------
DET_MODEL = "yolov8n-det.onnx"                           # COCO 80 类（人物/车辆）
# ---------------------------------------------------------------------------
# 人脸模型档位（2026-10-01）：两档**可切换**，选择持久化在 models/current_face.json
#
#   precise：det_10g（SCRFD-10G）+ w600k_r50（ArcFace R50）—— 非人脸误检与聚类错挂最少
#   light  ：det_500m（SCRFD-500M）+ w600k_mbf（MobileFaceNet）—— 快 6~7 倍，误检与错挂明显更多
#
# 实测（7840HS，CPU intra_op=8，bench/face_tier_bench.py，真实相册 10 张）：
#   precise 检测 66.8ms/张 · 识别 39.4ms/脸；light 检测 11.4ms/张 · 识别 5.5ms/脸；
#   注意 intra_op 12/16 线程反而更慢（det 78.8ms），8 线程是两档的拆中比最优点。
#
# ⚠ 换档 = 换人脸向量空间：persons.db 的 meta.emb_model 会拦住不匹配的扫描
#   （见 person_store.register），必须先去「人物」页执行「重建人物库」再重新扫描。
# ---------------------------------------------------------------------------
FACE_MODEL_META: dict[str, dict] = {
    "precise": {
        "label": "高精度 · det_10g + w600k_r50（推荐）",
        "det": "det_10g.onnx",
        "rec": "w600k_r50.onnx",
        "accuracy": "非人脸误检最少；侧脸 / 大姿态 / 年龄跨度下聚类最稳",
        "speed": "检测 ~67ms/张 · 识别 ~39ms/脸（7840HS·8 线程）",
        "note": "换档会更换人脸向量空间：需先在「人物」页点「重建人物库」，再重新扫描",
    },
    "light": {
        "label": "轻量 · det_500m + w600k_mbf",
        "det": "det_500m.onnx",
        "rec": "w600k_mbf.onnx",
        "accuracy": "误检与聚类错挂明显多于高精度档（老机器 / 大库想跑得快时选它）",
        "speed": "检测 ~11ms/张 · 识别 ~5.5ms/脸（约 6~7 倍快）",
        "note": "换档会更换人脸向量空间：需先在「人物」页点「重建人物库」，再重新扫描",
    },
}
FACE_TIERS = ["precise", "light"]
FACE_DEFAULT_TIER = "precise"
FACE_CURRENT_PATH = os.path.join(MODEL_DIR, "current_face.json")

# 语义图像塔的**推理后端**选择（与「语义模型档位」正交，别混）：
#   auto（默认）= 装了 OpenVINO 运行时组件就用它（优先 NPU），否则 ORT
#   ort          = 强制走 ONNX Runtime（回到 FEAT-SEM 以来的一贯行为）
# 该文件随机器不同（组件装没装），与 current_threads.json 同口径不入库。
CLIP_RUNTIME_PATH = os.path.join(MODEL_DIR, "current_clip_runtime.json")


def face_tier_files(tier: str | None = None) -> tuple[str, str]:
    """档位 → (检测模型文件名, 识别模型文件名)。未知/缺省 → 当前生效档。"""
    name = tier if tier in FACE_MODEL_META else active_face_tier()
    meta = FACE_MODEL_META.get(name) or FACE_MODEL_META[FACE_DEFAULT_TIER]
    return meta["det"], meta["rec"]


def face_tier_ready(tier: str) -> bool:
    """该档两个模型文件是否都在（不在则不能切、也不能作为“生效档”）。"""
    det, rec = face_tier_files(tier)
    return os.path.isfile(os.path.join(MODEL_DIR, det)) and os.path.isfile(
        os.path.join(MODEL_DIR, rec)
    )


def active_face_tier() -> str:
    """当前生效档位：持久化选择优先（文件须在），否则取第一个已就绪档，最后回落默认档。

    与 CLIP 档位（active_clip）同一套语义：模型文件被删后不会报一个“用不了的档”。
    持久化文件缺失/非法/文件不在 → 自动探测，保证升级/拷贝模型目录后仍能开箱可用。
    """
    try:
        if os.path.isfile(FACE_CURRENT_PATH):
            with open(FACE_CURRENT_PATH, encoding="utf-8") as f:
                name = json.load(f).get("name")
            if name in FACE_MODEL_META and face_tier_ready(name):
                return name
    except Exception:  # noqa: BLE001
        pass
    for n in FACE_TIERS:
        if face_tier_ready(n):
            return n
    return FACE_DEFAULT_TIER


def set_active_face_tier(name: str) -> None:
    """持久化人脸档位选择（失败不阻断：下次启动回落自动探测）。

    这就是「设置一次就记住」的落点：写 models/current_face.json，重启后仍生效。
    """
    if name not in FACE_MODEL_META:
        raise ValueError(f"未知人脸模型档位: {name}")
    try:
        with open(FACE_CURRENT_PATH, "w", encoding="utf-8") as f:
            json.dump({"name": name}, f)
    except Exception:  # noqa: BLE001
        pass


def face_det_models() -> list[str]:
    """当前档位的人脸检测模型候选（单元素；保留 list 结构便于未来加兑底）"""
    return [face_tier_files()[0]]


def face_rec_models() -> list[str]:
    """当前档位的人脸识别模型候选（决定嵌入空间）"""
    return [face_tier_files()[1]]


OCR_MODEL = "paddleocr-det.onnx"                        # PaddleOCR ch_PP-OCRv4 det（可选）

# ---------------------------------------------------------------------------
# 语义模型档位（Chinese-CLIP 双塔，Xenova ONNX 转换）
#   - 每档一个模型目录；首次使用前需把整图拆成 clip_vision.onnx / clip_text.onnx
#     （见 embed_service.ensure_subgraphs，幂等）
#   - 固定 CPU 推理：AMD DML 对该 fp16 图存在算子级数值 bug（实测输出错误），
#     且全量图优化会在 vision 塔初始化时崩溃（BUG-2026-0910-006）
#   - `id` 是落库到 photo_embeddings.model 的标识；**历史值不可更改**，
#     换档必须换标识（否则新旧向量会被误认为同一空间）
# ---------------------------------------------------------------------------
CLIP_MODEL_META: dict[str, dict] = {
    "b16": {
        "label": "B/16 · 默认（核显本推荐）",
        "dim": 512,
        "size": 224,
        "max_len": 52,
        "dir": "chinese-clip",
        "onnx": "model_fp16.onnx",
        "bytes": 377377730,
        "repo": "Xenova/chinese-clip-vit-base-patch16",
        "id": "chinese-clip-vit-b16-fp16",
        "accuracy": "零样本 81.1%（53 张标注集）",
        "speed": "CPU ~90ms/张（1 万张约 17 分钟）",
        "note": "精度/速度平衡档，任何核显本都可跑",
    },
    "b16-fp32": {
        # P20：原本写死「（719MB）」在 label 里，与 meta.bytes（754MB，整图时代体积）
        # 以及会话实测的 clip_vision.onnx 单文件（345MB）三个数互相矛盾。
        # 体积改由 `clip_disk_bytes()` 磁盘实算，label 不再携带硬编码体积。
        "label": "B/16 fp32 · GPU 可加速",
        "dim": 512,
        "size": 224,
        "max_len": 52,
        "dir": "chinese-clip-fp32",
        "onnx": "model.onnx",
        "bytes": 753665706,
        "repo": "Xenova/chinese-clip-vit-base-patch16",
        "id": "chinese-clip-vit-b16-fp32",
        "accuracy": "与 B/16 fp16 同权重（数值略更精确，未单独实测）",
        "speed": "CPU 约 2 倍于 fp16；DirectML 实测 37.9ms/张（约 fp16 CPU 的 2.4 倍快）",
        "note": "体积换 GPU 加速可能（fp16 在 DirectML 上有算子级数值 bug，故只有 fp32 档能走 GPU）；"
                "GPU 默认未开启，需先做数值一致性验证；切换后必须重建语义索引",
    },
}

# 候选顺序（默认 = 第一个已下载者）
# 注：L/14-336（chinese-clip-vit-large-patch14-336px）已于 2026-09-21 实测否决并下架 ——
#     官方权重自行导出（数值自校验 cos=1.000000）69.8~71.7%、Xenova 版 67.9~69.8%，
#     均低于 B/16 的 81.1~83.0%，且慢 3.5~10×；崩点在场景类（architecture 0~1/4、night 2~3/7）。
#     证据与复现方式见 design/clip-accuracy-comparison.md（需要时可一键重下/重导）。
CLIP_MODELS = ["b16", "b16-fp32"]
CLIP_DEFAULT_MODEL = "b16"
CLIP_CURRENT_PATH = os.path.join(MODEL_DIR, "current_clip.json")
# CLIP 标准归一化（b16 / b16-fp32 同协议）
CLIP_MEAN = (0.48145466, 0.4578275, 0.40821073)
CLIP_STD = (0.26862954, 0.26130258, 0.27577711)


def clip_paths(model: str | None = None) -> dict:
    """返回指定档位（默认当前生效档）的路径与超参。"""
    name = model or active_clip()
    meta = CLIP_MODEL_META.get(name) or CLIP_MODEL_META[CLIP_DEFAULT_MODEL]
    d = os.path.join(MODEL_DIR, meta["dir"])
    return {
        "name": name,
        "id": meta["id"],
        "label": meta["label"],
        "dim": meta["dim"],
        "size": meta["size"],
        "max_len": meta["max_len"],
        "dir": d,
        # 整图文件名随档位不同（fp16 → model_fp16.onnx / fp32 → model.onnx）
        "onnx": meta["onnx"],
        "whole": os.path.join(d, "onnx", meta["onnx"]),
        "vision": os.path.join(d, "clip_vision.onnx"),
        "text": os.path.join(d, "clip_text.onnx"),
        "tokenizer": os.path.join(d, "tokenizer.json"),
        "vocab": os.path.join(d, "vocab.txt"),
        "mean": CLIP_MEAN,
        "std": CLIP_STD,
    }


def _clip_ready_files(name: str) -> bool:
    p = clip_paths(name)
    return os.path.isfile(p["vision"]) and os.path.isfile(p["text"])


def active_clip() -> str:
    """当前生效档位：持久化选择优先（文件须在），否则按候选顺序取第一个已就绪者。"""
    try:
        if os.path.isfile(CLIP_CURRENT_PATH):
            with open(CLIP_CURRENT_PATH, encoding="utf-8") as f:
                name = json.load(f).get("name")
            if name in CLIP_MODEL_META and _clip_ready_files(name):
                return name
    except Exception:  # noqa: BLE001
        pass
    for n in CLIP_MODELS:
        if _clip_ready_files(n):
            return n
    return CLIP_DEFAULT_MODEL


def set_active_clip(name: str) -> None:
    """持久化档位选择（失败不阻断：下次启动回退自动探测）。"""
    if name not in CLIP_MODEL_META:
        raise ValueError(f"未知语义模型档位: {name}")
    try:
        with open(CLIP_CURRENT_PATH, "w", encoding="utf-8") as f:
            json.dump({"name": name}, f)
    except Exception:  # noqa: BLE001
        pass


# ---------------------------------------------------------------------------
# 推理参数
# ---------------------------------------------------------------------------
DET_SIZE = 640

# ---------------------------------------------------------------------------
# 解码降采样（P-DEC1：2026-10-27）
#
# 背景：AI 腿每张图的 CPU 大头不是推理，是**为了要一张 640px 的图把原图全解码一遍**。
# 实测（245K，6P+8E，P=6 并发，24 张 8~12MP 原图，服务端 [vcr.stat] 记账）：
#   单张墙钟 200.3ms 里 decode 占 173.9ms（82%）；此时 GPU 利用率仅 ~26%。
#
# 做法：解码时给 PIL 一个「解码后长边 ≥ 该值」的下限，让它走 libjpeg 的
# draft()（DCT 域 1/2、1/4、1/8 直接解），而不是全分辨率解完再用 BILINEAR 缩。
# 实测同一批（P=6，decode+resize 合计）：
#   全分辨率           194.9 ms/张（解码后 8.0~12.6 MP）
#   长边≥1600          ~90-110 ms/张（约 -50%）
#   长边≥1024          ~62 ms/张
#   长边≥640            34.2 ms/张
#
# 为什么定 1600 而不是更快的那两档 —— 人脸链路实测（30 张人脸密集图 409 张脸，
# 与全分辨率解码逐脸配对，判据是同一张脸两版嵌入的余弦）：
#   长边≥1600：脸数完全一致，cos 均值 0.98440 / 最小 0.90314，bbox IoU 中位 1.000
#   长边≥1024：cos 均值 0.94479 / 最小 0.72334 —— 开始明显改变嵌入，会带偏聚簇
#   长边≥640 ：cos 均值 0.81862 / 最小 0.12649 —— 不可用
# FACE_SIM 现为 0.55，1600 档的 0.90 最小余弦距它仍有充足余量；且 bbox 坐标、
# FACE_MIN_PIX(48px) 的真值口径全部不变（见 preprocess.Letterbox.dec_scale）。
#
# 注意 denominator 只能取 2 的幂：所以 1600 档对「长边 < 3200」的图不生效
# （3000x2000 → 1/2 = 1500 < 1600，保持全分辨率解码），这类图本来也只有 6MP。
# 设成 0 或负数 = 关闭降采样（回到逐位等价的旧行为，A/B 对照用）。
DECODE_MIN_LONG = max(0, int(os.environ.get("VCR_DECODE_MIN_LONG", "1600")))
# ---------------------------------------------------------------------------
# CPU 线程数（可在「⚙ 性能设置」里改 + 对比测速，落 models/current_threads.json）
#
# ONNX Runtime 的 intra_op 线程数直接决定推理速度。实测（7840HS，8核16线程）：
#   4 线程 → CLIP 编码 277ms/张；8 线程 → 177ms/张（1.57×）；16 线程反而略差
# 默认取「物理核数」并夹在 4~8：物理核才是真并行，逻辑核（超线程）会互抢执行单元。
# 不同机器差异大（老双核本 4 线程即满、12 核本 8 线程足够），故做成用户可调 + 可实测。
# ---------------------------------------------------------------------------
THREADS_MIN = 1
THREADS_MAX = 16
THREADS_CURRENT_PATH = os.path.join(MODEL_DIR, "current_threads.json")


def logical_cores() -> int:
    return os.cpu_count() or 4


def _cpu_topology() -> tuple[int, int, int]:
    """返回 (物理核数, P 核数, E 核数)。拿不到时猜测值并标记。

    为什么要真读拓扑（BUG-2026-1027-003）：旧实现用 `logical // 2` 猜物理核，
    这个假设只在「每个物理核都有超线程」时成立。AMD Zen 4（7840HS，8C/16T）符合，
    但 Intel 从 Alder Lake 起是大小核且**桌面版没有超线程** —— 245K 是 6P+8E、
    14 个逻辑核，`14 // 2 = 7` 恰好猜成「7 个物理核」，而真实是 6 个 P 核 + 8 个 E 核。

    intra_op 线程跑在 E 核上是纯亏：实测同一台 245K 上 det 单次推理
      6 线程 32.8ms · 8 线程 39.9ms · 10 线程 54.2ms · 12 线程 88.9ms · 14 线程 110.2ms
    （线程越多越慢：一旦越过 P 核数量就往 E 核溢出，ORT 的 spin-wait 在 E 核上
    把 P 核一起拖垮）。所以默认值应该取 **P 核数**，不是「逻辑核/2」，也不是总核数。

    数据源：Windows GetLogicalProcessorInformationEx(RelationProcessorCore)，
    每个核带 EfficiencyClass（越大越强），同厂商同架构下可稳定区分 P/E。
    非 Windows 或调用失败 → 回落旧口径 logical//2（不比以前差）。
    """
    guess = max(1, logical_cores() // 2)
    if os.name != "nt":
        return guess, guess, 0
    try:
        import ctypes
        from ctypes import wintypes

        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        fn = k32.GetLogicalProcessorInformationEx
        # RelationProcessorCore = 0
        buf = ctypes.create_string_buffer(0)
        length = wintypes.DWORD(0)
        fn(0, buf, ctypes.byref(length))
        if length.value <= 0:
            return guess, guess, 0
        buf = ctypes.create_string_buffer(length.value)
        if not fn(0, buf, ctypes.byref(length)):
            return guess, guess, 0

        total = 0
        classes: dict[int, int] = {}
        off = 0
        raw = buf.raw
        while off < length.value:
            # SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX:
            #   DWORD Relationship; DWORD Size; <union>
            rel, size = struct.unpack_from("II", raw, off)
            if size <= 0:
                break
            if rel == 0:  # RelationProcessorCore
                # PROCESSOR_RELATIONSHIP: BYTE Flags; BYTE EfficiencyClass;
                #                       BYTE Reserved[20]; WORD GroupCount; ...
                eff = raw[off + 8 + 1]
                classes[eff] = classes.get(eff, 0) + 1
                total += 1
            off += size
        if total <= 0:
            return guess, guess, 0
        if len(classes) == 1:
            # 同质核（AMD 全大核 / 老 Intel）→ 物理核 = 总核
            return total, total, 0
        best = max(classes)
        p = classes[best]
        return total, p, total - p
    except Exception:  # noqa: BLE001
        return guess, guess, 0


def physical_cores_guess() -> int:
    """物理核数（真读拓扑；拿不到才回落 logical//2）。"""
    return _cpu_topology()[0]


def default_threads() -> int:
    """默认线程数：**P 核数**（非混合核时就是物理核数），夹在 [4, 8]。

    上限 8 的理由保持不变（高于 8 收益递减且抢内存带宽）；下限 4 保证小机器能用。
    混合核机器（如 6P+8E）取 6 而不是 7/14 —— 实测 6 线程比 14 线程快 3.4 倍。
    """
    _, p_cores, _ = _cpu_topology()
    return max(4, min(8, p_cores))


def threads() -> int:
    """当前生效线程数：用户持久化选择优先（非法/越界 → 回落默认）。"""
    try:
        if os.path.isfile(THREADS_CURRENT_PATH):
            with open(THREADS_CURRENT_PATH, encoding="utf-8") as f:
                n = int(json.load(f).get("threads"))
            if THREADS_MIN <= n <= THREADS_MAX:
                return n
    except Exception:  # noqa: BLE001
        pass
    return default_threads()


def set_threads(n: int) -> int:
    """持久化线程数（越界则夹紧）；返回**实际生效值**。

    ⚠️ 旧实现写失败时 `except: pass` 然后 `return n`，于是 POST /threads 会返回
    「applied=7」而实际生效仍是旧值（BUG-2026-1027-009，实测撞到过：写 7 读回 12）。
    现在写完回读一次，返回真实生效值；写失败额外打一行 stderr，不再静默。
    """
    n = max(THREADS_MIN, min(THREADS_MAX, int(n)))
    try:
        with open(THREADS_CURRENT_PATH, "w", encoding="utf-8") as f:
            json.dump({"threads": n}, f)
    except Exception as e:  # noqa: BLE001
        print(f"[VCR] 线程数持久化失败（{THREADS_CURRENT_PATH}）: {e}", file=sys.stderr)
    actual = threads()
    if actual != n:
        print(f"[VCR] 线程数未生效：期望 {n} 实际 {actual}", file=sys.stderr)
    return actual


def threads_info() -> dict:
    """给 UI 的线程数现状（当前/默认/核数/可选档）。"""
    physical, p_cores, e_cores = _cpu_topology()
    return {
        "threads": threads(),
        "default": default_threads(),
        "physical_guess": physical,
        "physical": physical,
        "p_cores": p_cores,
        "e_cores": e_cores,
        "hybrid": e_cores > 0,
        "logical": logical_cores(),
        "min": THREADS_MIN,
        "max": THREADS_MAX,
        # 常用档位：1..min(max, 逻辑核)，去重后给出
        "options": sorted({1, 2, 4, 6, 8, 12, 16} & set(range(THREADS_MIN, THREADS_MAX + 1))
                          | {default_threads()}),
    }
BATCH_CHUNK = 8          # /embed_batch 单次最大张数（客户端默认）
BATCH_CHUNK_MAX = 64     # 单次请求安全封顶（前端批次选择上限）

# ---------------------------------------------------------------------------
# P-OPT2：/classify_batch 照片级并行度（一个线程处理一张照片，P 张同时在飞）
# ---------------------------------------------------------------------------
# 为什么需要（实测依据，勿凭直觉改回串行）：
#   - det/face/ocr 会话 intra_op = threads()（默认物理核夹 [4,8]）吃不满机器
#     —— 真机截图：classify 期间 16 核总利用率仅 ~12%；
#   - 单张里有一段串行 PIL 解码（4096px 原图数十 ms）夹在两次推理之间。
#   ⇒ 照片级并行让「下一张解码」与「当前张推理」重叠，并填补 ONNX 同步缝隙。
#     与 P-OPT1 的通道两流并行正交叠加（tone 池随 P 扩容，见 pipeline.py）。
# 实测（photo_parallel_bench.py，7840HS · album 32 真实照片 32 张，解码计入测量，
# 预热 1 轮 + N 轮取最快 · 交错轮转）：
#   P=1/2/3/4/6/8 = 282.6 / 201.2 / 172.2 / 158.4 / 153.9 / 151.9 ms/张
#              = 1.00x / 1.40x / 1.64x / 1.78x / 1.82x / 1.86x
# 等价性：各档 category/sub_category/label/confidence/person_count 逐字段一致，
#   distinct person 数一致（person_ids 允许置换，见 bench docstring）。
# 实测（photo_parallel_bench.py，7840HS，解码计入测量，预热 1 轮 + N 轮取最快 · 交错轮转）：
#   album 32（1.28~7.26MB 混合，2026-09-22）：
#     P=1/2/3/4/6/8 = 282.6 / 201.2 / 172.2 / 158.4 / 153.9 / 151.9 ms/张
#                   = 1.00x / 1.40x / 1.64x / 1.78x / 1.82x / 1.86x
#   test 53（5.14MB 大图为主，2026-09-23）：
#     P=1/2/3/4/6/8 = 352.3 / 238.1 / 199.5 / 181.3 / 162.6 / 152.6 ms/张
#                   = 1.00x / 1.48x / 1.77x / 1.94x / 2.17x / 2.31x
#   ⇒ 最优 P 随相册画像漂移：解码占比越高（大图）可重叠的串行段越多，曲线越晚进平台；
#     但 P=6 在两个画像下都稳赢 P=4（+2.9% / +10.8%），P=8 均为峰值。
# 等价性：各档 category/sub_category/label/confidence/person_count 逐字段一致，
#   distinct person 数一致（person_ids 允许置换，见 bench docstring）。
# 默认取 P=6（2026-09-23 由 4 修订）：两个画像均接近 P=8 峰值（97% / 94%），同时比
#   P=8 多留 CPU 余量给宿主 UI/DB（P=8 通道拉长最狠：album 32 ocr 均摊 208→377ms）。
#   大图相册想榨满吞吐可用 VCR_PHOTO_PARALLEL=8。
# DML 注意（2026-09-23 实测，BUG-2026-0923-001）：DML 会话并发 run 会段错误 ——
#   P≥2 时两个线程同时进 ORT run → ACCESS_VIOLATION（faulthandler 钉死）；
#   未来启用 GPU 必须先给 GPU 会话加进程内串行锁。且 780M 集显 DML P=1 单张
#   395.4ms 还慢于 CPU P=1 的 352.3ms —— 小模型+集显组合 DML 整体判负
#   （数值一致性已验证通过：硬字段全一致，confidence 全在 5e-3 容差内）。
# 注意：intra_op 线程与照片并行共享同一份 CPU 预算，两层不要同时拉满；
# 环境变量 VCR_PHOTO_PARALLEL 可覆盖（设 1 = 回到串行，A/B 对照用）。
PHOTO_PARALLEL = max(1, min(8, int(os.environ.get("VCR_PHOTO_PARALLEL", "6"))))

# ---------------------------------------------------------------------------
# GPU 加速（R3）
# ---------------------------------------------------------------------------
VCR_PROVIDER = os.environ.get("VCR_PROVIDER", "auto").lower()  # auto | cpu | gpu

# ---------------------------------------------------------------------------
# 阈值：人物检测 / 仲裁（F20 固化值 + 校准 2026-08-13）
# ---------------------------------------------------------------------------
PERSON_CONF_MIN = 0.35        # 人像检测最低置信度（F20 固化值）
VEHICLE_HEAVY_N = 4           # 校准：车辆框 ≥ 4 视为密集车流（street 判定前先排除）
VEHICLE_PERSON_AREA_MAX = 0.03  # 校准：密集车流下人框最大面积 <3% 才视为误检
NMS_IOU = 0.45                # det NMS IoU
PORTRAIT_AREA = 0.30          # 最大人框面积 ≥30% → 人物特写
STREET_PERSON_N = 3           # 人数 ≥3 → 扫街候选
STREET_MAX_AREA = 0.20        # 且最大人框面积 <20%
GROUP_AREA = 0.10             # 2 人且面积 ≥10% → 合影（仍归人物）
IGNORE_PERSON_AREA = 0.10     # 最大人框面积 <10% 的单人 → 路人，不覆盖分类
# 人脸误检双门槛（2026-10-01 两轮收紧：雕像/花纹/屏幕 → 人脸，以及后面的低头/后脑勺）
#   FACE_DET_CONF：SCRFD 检测分数门槛。曾经复用 PERSON_CONF_MIN(0.35) → 0.50 → 0.55。
#     第二轮实测（P111 垃圾簇 53 框 vs 大簇真人脸 250 框）：提到 0.55 丢 26% 垃圾框、
#     真脸样本零损失（真脸 p10=0.76，与垃圾中位 0.61 基本不重叠）。
#   FACE_MIN_PIX：24→32→48。小脸放大到 112 喂 ArcFace 只会产出噪声向量，是聚类
#     错挂（陌生人进簇 / 质心漂移）的隐形污染源；实测 24~48px 那档垃圾占 13%、真脸仅 0.8%。
FACE_DET_CONF = 0.55          # SCRFD 人脸框最低检测分数（独立于 YOLO PERSON_CONF_MIN）
FACE_MIN_PIX = 48             # 人脸最小边长（像素），小于则跳过标号
# ---------------------------------------------------------------------------
# 人脸质量闸门（2026-10-01 第三轮；实测脚本 _tmp 诊断 + python/bench/face_quality_check.py）
#
# 背景：用户反馈 det_10g 仍会把非人脸当人脸（P111 簇里混进低头/后脑勺/手挡脸/一张风扇）。
# 实测结论：**单靠置信度救不了**（垃圾框 score 中位 0.61、最高 0.79），但 5 点关键点的
# 几何形状能干净区分——垃圾框眼距占框宽中位 0.07（真人脸 0.46）、鼻-眼纵向比中位 4.96
# （真人脸 0.57）、yaw 中位 2.03（真人脸 0.04）。各门槛实测（丢垃圾 / 误丢真脸）：
#   eye_span 越界 81.1% / 4.8%   yaw>0.35 84.9% / 8.0%   nose_dy 越界 75.5% / 3.6%
#   score<0.55   26.4% / 0.0%    side<48  13.2% / 0.8%   blur<40  17.0% / 1.2%
# 组合（几何+尺寸+模糊）：丢 86.8% 垃圾 / 10.0% 真脸；再叠 score：92.5% / 11.6%。
#
# 被误丢的 10% 几乎全是极端侧脸/低头——它们不该被丢，而是**不该拿它新建人物**：
# 垃圾簇正是从「一张烂脸开出一个新簇」开始吸人的 ⇒ 分两级
#   reject   ：硬丢（连嵌入都不算，省掉 r50 前向）
#   marginal ：允许并入已有簇，但不准新建人物、不更新质心（不让烂脸拖跑质心）
# ---------------------------------------------------------------------------
FACE_QUALITY_ASPECT = (0.62, 1.60)      # 宽高比；实测丢 41.5% 垃圾 / 1.2% 真脸
FACE_QUALITY_EYE_SPAN = (0.28, 0.58)    # 两眼中点距 / 框宽；丢 81.1% / 4.8%
FACE_QUALITY_YAW = 0.50                 # 鼻-眼横向偏移 / 眼距 → 超过判为极端侧脸，丢
FACE_QUALITY_ROLL_DEG = 32.0            # 两眼连线倾角（度）→ 丢
FACE_QUALITY_NOSE_DY = (0.12, 1.00)     # (鼻y - 眼中点y) / 眼距；丢 75.5% / 3.6%
FACE_QUALITY_MOUTH_DY = (0.10, 1.60)    # (嘴角y - 鼻y) / 眼距；丢 66.0% / 5.2%
FACE_QUALITY_BLUR_MIN = 40.0            # 对齐到 112×112 后的 Laplacian 方差；< 40 丢
# 边缘质量（marginal）：只准并入、不准新建、不更新质心
FACE_MARGINAL_YAW = 0.35
FACE_MARGINAL_ROLL_DEG = 25.0
FACE_MARGINAL_MIN_PIX = 64              # 短边 < 64 归为边缘质量
FACE_MARGINAL_BLUR = 80.0
# FACE_SIM：0.45→0.55。实测旧库 1255/9866 张脸与自身质心相似度 <0.60、
# 332 张与「别人的质心」更近（错挂）；0.55 配合 r50 嵌入在「错并」与「过碎」
# 之间取平衡（r50 同人相似度普遍 >0.6，异人 <0.45）。
FACE_SIM = 0.55               # 人脸 cosine 相似度阈值（≥ 视为同一人）

# ---------------------------------------------------------------------------
# 夜景通道（影调主导，降级版）
#   分类模型下线后不再有 cls 弱证据与 Places365 语义，夜景判定回到纯影调：
#   avg_luma < NIGHT_LUMA(45) → night_scene（对齐旧链路"luma<45 且 cls 弱证据"
#   的实际行为，绝大多数照片 cls 置信度都低于 0.5）。
#   需要更精细的夜景范围时，请用「语义分类」自建关键词（如「城市夜景」）。
# ---------------------------------------------------------------------------
NIGHT_LUMA = 45

# ---------------------------------------------------------------------------
# 文档 OCR 通道
# ---------------------------------------------------------------------------
OCR_AREA_STRONG = 0.12        # 文字框面积占比 >12% 且 ≥2 框 → 强证据 → document
# OCR 前置门（P-OPT3，2026-10-27）
#
# 仲裁优先级是「人物规则 > OCR > 截图 > 夜景」，所以人物规则一旦命中，OCR 结果
# 永远不会被采纳 —— 而旧实现每张图都无条件跑一次 OCR。
# 实测（245K + DML，P=6，24 张原图）：整批共 55 次加锁，**持锁时间占墙钟 99.0%**
# （GPU 会话全局串行锁就是天花板），其中 OCR 占 24 次（44%）。
# 前置门用 arbitrator.person_rule_hits(det_out) 判定，与仲裁共用同一个函数，
# 因此**结论等价**（不是“几乎等价”）：被跳过的那些图，OCR 本来就不会被读。
# 设 VCR_OCR_GATE=0 可关闭（A/B 对照用）。
OCR_GATE = os.environ.get("VCR_OCR_GATE", "1") != "0"
OCR_PROB_THRESH = 0.3         # DB 概率图阈值
OCR_BOX_THRESH = 0.5          # box 平均概率阈值（DB box_thresh）

# 截图启发式
SCREEN_ASPECTS = [
    (16, 9), (16, 10), (4, 3), (3, 4), (9, 16), (10, 16),
    (19, 9), (21, 9),
]
SCREEN_MIN_SIZE = 600         # 短边下限
SCREEN_TOL = 0.06             # 宽高比容差

# 规则通道产出的系统分类（与宿主 category.rs 的 builtin slug 一一对应）
CATEGORY_DESC = {
    "portrait": "人物特写",
    "street": "扫街",
    "night_scene": "夜景",
    "document": "文档",
    "other": "其他",
}

os.makedirs(DATA_DIR, exist_ok=True)
