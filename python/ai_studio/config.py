"""AI 创作微服务配置（FEAT-152）

服务是**无状态**的：模型配置（baseUrl/apiKey/model）由 Rust 在每次作业请求里附带，
Python 侧绝不落盘 api_key。这里只放进程级常量与端口解析。
"""
import os
import sys

from . import cputopo

# /health 里回给 Rust 的协议版本：Rust 侧据此判定服务是否需要重启（与 VCR 的 api_version 同思路）
API_VERSION = 1
SERVICE_NAME = "ai-studio"

# 单作业并发上限：图像编辑多为网络等待，2 路并发足够，过多会触发服务商限流
JOB_WORKERS = 2

# 本机 ONNX 会话的 CPU 线程口径（与 VCR 的 `config.threads()` 同一纪律）：
#   - intra_op：单个算子内部并行度。默认按 ORT 满核开，而本服务同时有
#     「照片并行 P + 预览线程池」，多会话叠加会 CPU 超订、互相抢核反而更慢；
#   - inter_op 固定 1（算子级并行同样会和并发会话打架，见 vcr/model_registry.py）。
#
# 取 `min(P 核数, 逻辑核 // 2)`（见 `cputopo.py`）：
#   旧口径是 `cpu_count // 2`，在大小核机器上会**越过 P 核溢到 E 核**——
#   本机 245K（6P+8E，14 逻辑核）旧值算出 7，而实测 6 线程 32.8ms、8 线程 39.9ms、
#   14 线程 110.2ms（越加越慢，BUG-2026-1027-003）。现按 P 核数取 6。
ONNX_INTRA_OP = cputopo.threads()
# 小模型（SCRFD / peppa / BiSeNet / OCR rec）用更少线程：单次前向只有 3~60ms，
# 线程铺满时调度开销占大头。按模型分档见 `localmodels.get_session(intra=...)`。
ONNX_INTRA_OP_SMALL = cputopo.small_model_threads()

# 单次 provider 请求超时（秒）：图像编辑可能较慢，给足。
# 本地网关（image2_1turbo / sd.cpp，qwen-image-2.1-turbo）在 1K 尺寸上一张要 2~5
# 分钟（CPU 文本编码 + Vulkan 扩散），180s 会稳定超时；云端网关通常 <60s，
# 放宽不会影响正常路径，只是把「慢」等成结果而不是报错。
REQUEST_TIMEOUT = 600
# 连通性探测超时（/models）
PROBE_TIMEOUT = 20


def port() -> int:
    """服务端口：由 Rust 拉进程时通过 AI_STUDIO_PORT 注入（动态分配，避免固定端口冲突）。"""
    raw = os.environ.get("AI_STUDIO_PORT", "0") or "0"
    try:
        return int(raw)
    except ValueError:
        return 0


# ---------------------------------------------------------------------------
# 本地增强（FEAT-163）：模型 / LUT 资源位置与目录清单
#
# 三类工具都跑在 ai_studio 进程内、纯本机、免费：
#   film_sim       .cube 3D LUT（随包内置，体积小）
#   color_transfer 纯算法（无模型）
#   restore        onnxruntime 本地模型（体积大，由用户在工坊内按链接下载）
# ---------------------------------------------------------------------------

_HERE = os.path.dirname(os.path.abspath(__file__))
BASE_DIR = os.path.dirname(_HERE)                       # python/


def _bundled_dir(name: str) -> str:
    """随包内置资产目录（如 luts/）的真实位置。

    开发态 = 包目录；PyInstaller 单文件模式下代码在 PYZ、资产解包到 sys._MEIPASS，
    此时 `__file__` 一般已指向解包目录，但仍显式兜一次 `_MEIPASS`：
    打包版曾因路径算不到而「胶片模拟报 LUT 缺失」，两者必须同一口径。
    """
    candidates = [
        os.path.join(_HERE, name),
        os.path.join(getattr(sys, "_MEIPASS", ""), "ai_studio", name),
    ]
    for p in candidates:
        if os.path.isdir(p):
            return p
    return candidates[0]


def _bundled_file(name: str) -> str:
    """随包内置单文件的真实位置（与 _bundled_dir 同口径，兜 _MEIPASS）。"""
    candidates = [
        os.path.join(_HERE, name),
        os.path.join(getattr(sys, "_MEIPASS", ""), "ai_studio", name),
    ]
    for p in candidates:
        if os.path.isfile(p):
            return p
    return candidates[0]


# 模型根目录：Rust 拉起子进程时注入 AI_STUDIO_MODELS_DIR（= 扫描模型下载的落位目录），
# 未注入时回落源码目录 python/models（直接 `python -m ai_studio.server` 的场景）。
# 为什么必须跟下载侧同一个根：打包版 ai-server.exe 是单文件，__file__ 指向临时解包目录，
# 自行推算会「设置里下载完成、工具仍报模型未就绪」（BUG-2026-1112-005）。
MODELS_DIR = os.environ.get("AI_STUDIO_MODELS_DIR") or os.path.join(BASE_DIR, "models")
LOCAL_MODELS_DIR = os.path.join(MODELS_DIR, "local")    # 需下载的大模型
LUTS_DIR = _bundled_dir("luts")                         # 随包内置的 .cube
FACE_DET_MODEL = os.path.join(MODELS_DIR, "det_10g.onnx")  # 扫描入库已有，直接复用

# ---------------------------------------------------------------------------
# 文字提取（FEAT-171）：PaddleOCR det+rec 本机离线 OCR
#
# 两个模型的分区不同（见 .dsh/knowledge/model-download-areas.md）：
#   - det：扫描区共享，模型管理下载（rust 名 "ocr"），落 <MODELS_DIR> 根；
#   - rec：工坊区下载（rust 名 "local-ocr-rec"），落 <MODELS_DIR>/local/，
#     LOCAL_MODELS 登记 + LocalModelHint.DL_NAMES 才能出下载入口。
# CTC 字典 ppocr_keys_v1.txt（Apache-2.0，26KB）随包内置（_bundled_dir），
# 每次模型重下都换字典是漂移源，固定跟代码走。
# ---------------------------------------------------------------------------
OCR_DET_MODEL = os.path.join(MODELS_DIR, "paddleocr-det.onnx")
OCR_REC_MODEL = os.path.join(LOCAL_MODELS_DIR, "paddleocr-rec.onnx")
OCR_KEYS_PATH = _bundled_file("ppocr_keys_v1.txt")
OCR_MODELS = {"paddleocr_det": OCR_DET_MODEL, "paddleocr_rec": OCR_REC_MODEL}

_RELEASE = "https://github.com/facefusion/facefusion-assets/releases/download/models-3.0.0"

# 本地模型清单：id / 显示名 / 文件名 / 期望大小(MB) / 下载地址
LOCAL_MODELS = [
    {
        "id": "real_esrgan_x4",
        "name": "Real-ESRGAN ×4 超分",
        "file": "real_esrgan_x4.onnx",
        "sizeMb": 69,
        "url": f"{_RELEASE}/real_esrgan_x4.onnx",
        "required": True,
    },
    {
        "id": "gfpgan_1.4",
        "name": "GFPGAN 1.4 人脸修复",
        "file": "gfpgan_1.4.onnx",
        "sizeMb": 340,
        "url": f"{_RELEASE}/gfpgan_1.4.onnx",
        "required": False,
    },
    {
        # 本地美颜用：BiSeNet ResNet-34 十九类人脸解析。
        # 上游 yakhyo/face-parsing（MIT），与 facefusion 的 bisenet_resnet_34.onnx 是同一套权重；
        # 选 yakhyo 的 release 是因为许可标注更明确、直链长期稳定。
        # 输入 1×3×512×512 RGB / ImageNet 归一化，输出 1×19×512×512 logits。
        "id": "face_parsing_r34",
        "name": "BiSeNet 人脸解析",
        "file": "face_parsing_bisenet_r34.onnx",
        "sizeMb": 93,
        "url": "https://github.com/yakhyo/face-parsing/releases/download/weights/resnet34.onnx",
        "required": False,
    },
    {
        # 本地形变用：peppa_wutz 68 点人脸关键点（FEAT-167）。
        # 选型依据（2026-11-13 实测）：单模型直出 68 点、13.1MB、Apache-2.0、
        # 自带逐点置信度；与扫描侧 det_10g 交叉验证眼中心误差 2.4~2.7px。
        # 输入 1×3×256×256 RGB /255，输出 1×68×3（x,y 为 0~64 量纲，第三维为置信度）。
        "id": "face_landmark_68",
        "name": "68 点人脸关键点",
        "file": "peppa_wutz.onnx",
        "sizeMb": 14,
        "url": f"{_RELEASE}/peppa_wutz.onnx",
        "required": False,
    },
    {
        # 文字提取用：PP-OCRv4 识别小模型（FEAT-171）。
        # ModelScope RapidAI/RapidOCR（Apache-2.0）与扫描侧 det 同一条源；
        # 10.4MB 纯推理权重，CTC 字典随包内置不在此列（换字典是漂移源）。
        # 输入 1×3×48×320，输出 (1,T,6625) logits；类别 0=blank、1..6623=字典行、6624=空格。
        "id": "paddleocr_rec",
        "name": "PaddleOCR 识别",
        "file": "paddleocr-rec.onnx",
        "sizeMb": 11,
        "url": (
            "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/"
            "onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_mobile.onnx"
        ),
        "required": False,
    },
]

# 内置胶片 LUT 清单：id / 显示名 / 文件名（随包发在 python/ai_studio/luts/）
LUTS = [
    {"id": "provia", "name": "Provia 标准", "file": "provia.cube"},
    {"id": "velvia", "name": "Velvia 浓郁", "file": "velvia.cube"},
    {"id": "astia", "name": "Astia 柔和", "file": "astia.cube"},
    {"id": "classic_chrome", "name": "Classic Chrome", "file": "classic_chrome.cube"},
    {"id": "classic_neg", "name": "Classic Negative", "file": "classic_neg.cube"},
    {"id": "eterna", "name": "Eterna 电影", "file": "eterna.cube"},
    {"id": "nostalgic_neg", "name": "Nostalgic Neg", "file": "nostalgic_neg.cube"},
]


def local_model_path(file_name: str) -> str:
    return os.path.join(LOCAL_MODELS_DIR, file_name)


def lut_path(file_name: str) -> str:
    return os.path.join(LUTS_DIR, file_name)
