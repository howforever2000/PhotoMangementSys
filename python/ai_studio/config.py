"""AI 创作微服务配置（FEAT-152）

服务是**无状态**的：模型配置（baseUrl/apiKey/model）由 Rust 在每次作业请求里附带，
Python 侧绝不落盘 api_key。这里只放进程级常量与端口解析。
"""
import os

# /health 里回给 Rust 的协议版本：Rust 侧据此判定服务是否需要重启（与 VCR 的 api_version 同思路）
API_VERSION = 1
SERVICE_NAME = "ai-studio"

# 单作业并发上限：图像编辑多为网络等待，2 路并发足够，过多会触发服务商限流
JOB_WORKERS = 2

# 单次 provider 请求超时（秒）：图像编辑可能较慢，给足
REQUEST_TIMEOUT = 180
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
MODELS_DIR = os.path.join(BASE_DIR, "models")
LOCAL_MODELS_DIR = os.path.join(MODELS_DIR, "local")    # 需下载的大模型
LUTS_DIR = os.path.join(_HERE, "luts")                  # 随包内置的 .cube
FACE_DET_MODEL = os.path.join(MODELS_DIR, "det_10g.onnx")  # 扫描入库已有，直接复用

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
