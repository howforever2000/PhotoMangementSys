"""VCR 全局配置：路径 / 阈值 / 模型清单

所有硬编码集中于此，服务层只引用 config 常量。
"""
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
ALBUM_GROUPS = os.path.join(MODEL_DIR, "album_groups.json")   # taxonomy 9 组定义

# 模型文件（缺失则对应通道自动降级）
# 2025-09：cls 升级为 yolov8m-cls（准确率 76.0% vs n 的 66.6%，7840HS CPU 推理单张 ~30-40ms）。
# ImageNet 仍是物体库，"风景/城市/室内"依靠 SCENE_MODEL（Places365）补齐，二者互不重叠互补。
CLS_MODELS = ["yolov8m-cls.onnx", "yolov8s-cls.onnx", "yolov8n-cls.onnx"]   # 按准确率依次尝试
DET_MODEL = "yolov8n-det.onnx"                           # COCO 80 类
FACE_DET_MODELS = ["det_10g.onnx", "det_500m.onnx"]     # SCRFD（buffalo_l/s → sc 兜底）
FACE_REC_MODELS = ["w600k_mbf.onnx", "w600k_r50.onnx"]  # ArcFace 识别
SCENE_MODEL = "resnet18_places365.onnx"                  # Places365（必启用：覆盖自然/城市/室内场景）
SCENE_CATEGORIES = "categories_places365.txt"
OCR_MODEL = "paddleocr-det.onnx"                        # PaddleOCR ch_PP-OCRv4 det（可选）
FLOWER_MODEL = "efficientnet-b2-flowers.onnx"       # Lumia101 B2 微调（oxford-102，31MB，实测优于 b0）
FOOD_MODEL = "food101-resnet50.onnx"              # Food101 专家（可选，懒加载）

CLASSES_PATH = os.path.join(MODEL_DIR, "imagenet_classes.txt")
MAPPING_PATH = os.path.join(MODEL_DIR, "imagenet_to_album.json")

# ---------------------------------------------------------------------------
# 推理参数
# ---------------------------------------------------------------------------
CLS_SIZE = 224
DET_SIZE = 640
THREADS = 4
TOP_K = 5
BATCH_CHUNK = 8          # /classify_batch 单次最大张数（客户端默认）
BATCH_CHUNK_MAX = 64     # /classify_batch 安全封顶（前端批次选择上限）

# ---------------------------------------------------------------------------
# GPU 加速（R3）
#   默认自动：若安装的 onnxruntime 包含 GPU 提供方（onnxruntime-directml /
#   onnxruntime-gpu）且 GPU 可用，则自动选用 GPU，否则回退 CPU。
#   可通过环境变量 VCR_PROVIDER=cpu 强制禁用 GPU。
# ---------------------------------------------------------------------------
VCR_PROVIDER = os.environ.get("VCR_PROVIDER", "auto").lower()  # auto | cpu | gpu

# ---------------------------------------------------------------------------
# 阈值（仲裁规则）
#   F20 固化值不重标定；以下标注「校准 2026-08-13」的为实测 §7.3 新增项
# ---------------------------------------------------------------------------
PERSON_CONF_MIN = 0.35        # 人像检测最低置信度（F20 固化值）
VEHICLE_HEAVY_N = 4           # 校准：车辆框 ≥ 4 视为密集车流（street 判定前先排除）
VEHICLE_PERSON_AREA_MAX = 0.03  # 校准：密集车流下人框最大面积 <3% 才视为误检（e-7278 0.6% vs e--7 骑手群 11%）
NMS_IOU = 0.45                # det NMS IoU
PORTRAIT_AREA = 0.30          # 最大人框面积 ≥30% → 人物特写
STREET_PERSON_N = 3           # 人数 ≥3 → 扫街候选
STREET_MAX_AREA = 0.20        # 且最大人框面积 <20%（校准：0.15→0.20，缓解合影吞扫街 #32）
GROUP_AREA = 0.10             # 2 人且面积 ≥10% → 合影（仍归人物）
IGNORE_PERSON_AREA = 0.10     # 最大人框面积 <10% 的单人 → 路人，不覆盖分类
SCENE_OVERRIDE_CONF = 0.40    # cls 置信度低于此值才允许场景路覆盖
SCENE_CONF_STRONG = 0.15      # 场景路自身置信度达到此值才允许翻转 cls 的 landscape/architecture
SCENE_TAKEOVER_MIN = 0.25     # 校准：场景接管全局最低置信度下限（拦 #18/#25 的 0.02x 覆盖）
FACE_MIN_PIX = 24             # 人脸最小边长（像素），小于则跳过标号
FACE_SIM = 0.45               # 人脸 cosine 相似度阈值（≥ 视为同一人）
ANIMAL_SUB_CONF_MIN = 0.3     # 校准：动物子类置信度低于此值不强判 dog/cat（拦白猫→白狗 #31）

# ---------------------------------------------------------------------------
# 夜景通道（Phase 1）
#   实测（53 张回归）：Places365 在夜景图上全部 low-conf 乱判（sky 0.022~
#   orchestra_pit/stage/forest 等），scene 语义不可靠；夜间判定改为纯影调分档：
#   极暗→夜景；低调+语义→夜景；中等低调+sky 语义→夜景。车辆照片排除。
# ---------------------------------------------------------------------------
NIGHT_LUMA_DARK = 25          # avg_luma < 25 → 极暗，无条件夜景（烟花/满月 4.5~23）
NIGHT_LUMA = 60               # avg_luma < 60 → 低调（夜景候选）
NIGHT_LUMA_DEEP = 45          # avg_luma < 45 → 极暗档（需弱 cls 证据）
NIGHT_LUMA_SKY = 70           # avg_luma < 70 且 scene 命中 sky/夜间词 → 夜景
NIGHT_CLS_CONF_MAX = 0.5      # 25~45 档：cls 置信度 ≥ 此值视为强证据，不判夜景（信号灯 conf 1.0）
NIGHT_KEYWORDS = (
    "night", "aurora", "star", "moon", "moonlight",
    "sunset", "dusk", "sky", "dark", "observatory",
)

# ---------------------------------------------------------------------------
# 文档 OCR 通道（Phase 4，可选）
# ---------------------------------------------------------------------------
OCR_AREA_STRONG = 0.12        # 文字框面积占比 >12% 且 ≥2 框 → 强证据 → document（实测校准 0.15→0.12）
OCR_PROB_THRESH = 0.3         # DB 概率图阈值
OCR_BOX_THRESH = 0.5          # box 平均概率阈值（DB box_thresh）

# ---------------------------------------------------------------------------
# 专家通道（Phase 3/5，可选，懒加载，模型缺失自动降级）
# ---------------------------------------------------------------------------
ENABLE_FLOWER_EXPERT = True
FLOWER_CONF = 0.4             # 专家置信度 >0.4 → flower，否则 plant（实测校准：0.5 漏特写花）
ENABLE_FOOD_EXPERT = True
FOOD_CONF = 0.6               # 专家置信度 ≥ 此值 → food

# 截图启发式
SCREEN_ASPECTS = [
    (16, 9), (16, 10), (4, 3), (3, 4), (9, 16), (10, 16),
    (19, 9), (21, 9),
]
SCREEN_MIN_SIZE = 600         # 短边下限
SCREEN_TOL = 0.06             # 宽高比容差

CATEGORY_DESC = {
    "animal": "动物", "food": "食物", "flower": "花朵", "plant": "植物",
    "cityscape": "城市风光", "architecture": "建筑", "sports": "运动",
    "landscape": "自然风景", "night_scene": "夜景", "document": "文档",
    "vehicle": "车辆", "portrait": "人物特写", "street": "扫街",
    "other": "其他",
    # 旧名保留（后端兼容/调试用，输出前经 taxonomy 折叠）
    "plant_flower": "植物花卉", "landscape_nature": "自然风景",
    "text": "文本截图",
}

ANIMAL_SUB_DESC = {
    "dog": "狗", "cat": "猫", "bird": "鸟",
}

os.makedirs(DATA_DIR, exist_ok=True)
