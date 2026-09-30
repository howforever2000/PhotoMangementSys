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


def physical_cores_guess() -> int:
    """物理核数推测：CPython 拿不到真实拓扑，按「逻辑核 = 物理核 × 2（超线程）」估计。"""
    return max(1, logical_cores() // 2)


def default_threads() -> int:
    """默认线程数：物理核数，夹在 [4, 8]（低于 4 太慢、高于 8 收益递减且抢内存带宽）。"""
    return max(4, min(8, physical_cores_guess()))


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
    """持久化线程数（越界则夹紧）；返回生效值。失败不阻断（下次启动回落默认）。"""
    n = max(THREADS_MIN, min(THREADS_MAX, int(n)))
    try:
        with open(THREADS_CURRENT_PATH, "w", encoding="utf-8") as f:
            json.dump({"threads": n}, f)
    except Exception:  # noqa: BLE001
        pass
    return n


def threads_info() -> dict:
    """给 UI 的线程数现状（当前/默认/核数/可选档）。"""
    return {
        "threads": threads(),
        "default": default_threads(),
        "physical_guess": physical_cores_guess(),
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
# 人脸误检双门槛（2026-10-01 收紧，修复「雕像/花纹/屏幕被判成人脸」）：
#   FACE_DET_CONF：SCRFD 检测分数门槛。此前复用 PERSON_CONF_MIN(0.35)，假脸
#     （纹理/海报）分数多落在 0.35~0.5，真脸普遍 >0.7 → 提到 0.5 误杀极少；
#   FACE_MIN_PIX：24→32。24px 的脸放大到 112 喂给 ArcFace 产出的是噪声向量，
#     是聚类错挂（陌生人进簇/质心漂移）的隐形污染源。
FACE_DET_CONF = 0.50          # SCRFD 人脸框最低检测分数（独立于 YOLO PERSON_CONF_MIN）
FACE_MIN_PIX = 32             # 人脸最小边长（像素），小于则跳过标号
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
