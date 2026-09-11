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
FACE_DET_MODELS = ["det_10g.onnx", "det_500m.onnx"]     # SCRFD（buffalo_l/s → sc 兜底）
FACE_REC_MODELS = ["w600k_mbf.onnx", "w600k_r50.onnx"]  # ArcFace 识别
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
        "label": "B/16 fp32 · GPU 可加速（719MB）",
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
    "l14": {
        "label": "L/14-336 · 更准（需更强硬件）",
        "dim": 768,
        "size": 336,
        "max_len": 64,
        "dir": "chinese-clip-l14",
        "onnx": "model_fp16.onnx",
        "bytes": 814388930,
        "repo": "Xenova/chinese-clip-vit-large-patch14-336px",
        "id": "chinese-clip-vit-l14-fp16",
        "accuracy": "官方称优于 B/16（本机未实测）",
        "speed": "CPU 约为 B/16 的 3~4 倍耗时（约 300ms/张）",
        "note": "模型约 814MB；切换后必须重建语义索引（旧向量维度不同会自动跳过）",
    },
}
CLIP_MODELS = ["b16", "b16-fp32", "l14"]   # 候选顺序（默认 = 第一个已下载者）
CLIP_DEFAULT_MODEL = "b16"
CLIP_CURRENT_PATH = os.path.join(MODEL_DIR, "current_clip.json")
# CLIP 标准归一化（B/16 与 L/14-336 同协议）
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
THREADS = 4
BATCH_CHUNK = 8          # /embed_batch 单次最大张数（客户端默认）
BATCH_CHUNK_MAX = 64     # 单次请求安全封顶（前端批次选择上限）

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
FACE_MIN_PIX = 24             # 人脸最小边长（像素），小于则跳过标号
FACE_SIM = 0.45               # 人脸 cosine 相似度阈值（≥ 视为同一人）

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
