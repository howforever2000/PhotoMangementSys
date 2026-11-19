"""工作流公共工具（FEAT-152）

读图 / 转字节 / 落盘 / 取消检查 / JSON 宽松解析 —— 供四个工作流共享，
避免每个 workfow 各写一份 IO 细节。
"""
from __future__ import annotations

import base64
import json
import math
import os
import re
import threading
from io import BytesIO
from typing import Any, Callable, Dict, Optional

from PIL import Image, ImageOps


class Cancelled(Exception):
    """用户取消作业。"""


Progress = Callable[[int, str, str], None]

# 图生图（i2i）网关 qwen-image-3.0 的硬约束：图像面积须在 [512x512, 2560x2560] 像素之间
MIN_I2I_AREA = 512 * 512      # 262144
MAX_I2I_AREA = 2560 * 2560    # 6553600


def check_cancel(cancel) -> None:
    if cancel is not None and cancel.is_set():
        raise Cancelled()


def read_image(path: str) -> Image.Image:
    """读图并按 EXIF 摆正、转 RGB（统一交给模型/排版）。"""
    if not os.path.isfile(path):
        raise FileNotFoundError(f"照片不存在: {path}")
    # with 确保解码器句柄关闭：批量作业里成千上万张开着不关会累积文件句柄。
    with Image.open(path) as img:
        return ImageOps.exif_transpose(img).convert("RGB")


def png_bytes(img: Image.Image) -> bytes:
    buf = BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


# 云端「理解」调用的输入短边上限（规划器看图、多模态复核）。
# 为什么按**短边**而不是长边：理解类调用关心的是画面里的全部内容与位置关系，
# 按短边定档能保证竖图/横图得到的细节密度一致；按长边定档会让 3:2 横图
# 的短边只剩 720，细小的文字/表情/构图元素会先糊掉。
UNDERSTAND_SHORT_EDGE = 1080


def _fit_short_edge(img: Image.Image, short_edge: int) -> Image.Image:
    """把**短边**缩到 `short_edge`（只缩不放：放大会更慢且不增加信息）。"""
    w, h = img.size
    if w <= 0 or h <= 0:
        return img
    cur = min(w, h)
    if cur <= short_edge:
        return img
    s = short_edge / float(cur)
    size = (max(1, int(round(w * s))), max(1, int(round(h * s))))
    return img.resize(size, Image.LANCZOS)


def understand_bytes(img: Image.Image, short_edge: int = UNDERSTAND_SHORT_EDGE) -> bytes:
    """云端「理解」调用的输入图字节：短边 ≤ `short_edge` 的 JPEG。

    为什么不是直接把原图存成 PNG（旧行为）：实测 24MP 照片 `img.save(PNG)` 要
    **5140 ms**、产出 37.9MB，base64 后 50.5MB —— 每次规划/复核都要付一遍，
    而且视觉模型的输入分辨率远小于 24MP，多出来的像素纯粹是上传与编码浪费。
    缩到短边 1080 后同样一张图约 **0.2MB / 120ms**。

    用 JPEG 而不是 PNG：理解调用只看内容，JPEG 的 12:1 压缩不改变判断；而
    PNG 在照片类内容上是无损但体积 3~5 倍 —— 上行带宽与网关超时都更危险。
    """
    im = _fit_short_edge(img.convert("RGB"), short_edge)
    buf = BytesIO()
    im.save(buf, "JPEG", quality=90)
    return buf.getvalue()


def understand_data_url(img: Image.Image, short_edge: int = UNDERSTAND_SHORT_EDGE) -> str:
    """`understand_bytes` 的 data URL 形态（providers 层要的就是这个）。"""
    return "data:image/jpeg;base64," + base64.b64encode(understand_bytes(img, short_edge)).decode("ascii")


def _fit_i2i_size(w: int, h: int, max_long_edge: int) -> tuple:
    """按图生图网关约束算出目标尺寸：长边 ≤ max_long_edge，且面积 ∈ [MIN, MAX]。

    实测网关（qwen-image-3.0，i2i）硬约束：
      `Image area must be between 262144 (512x512) and 6553600 (2560x2560)`。
    - 先按长边上限等比缩小（不主动放大）；
    - 面积超上限（如超分 3072 单边可达 9.4M）→ 等比缩小到上限；
    - 面积不达下限（小图，如 400x556=222400）→ 等比放大到下限（ceil 保证达标）。
    """
    if w <= 0 or h <= 0:
        return max(1, w), max(1, h)
    m = max(w, h)
    if max_long_edge and max_long_edge > 0 and m > max_long_edge:
        s = max_long_edge / float(m)
        w = max(1, int(round(w * s)))
        h = max(1, int(round(h * s)))
    area = w * h
    if area > MAX_I2I_AREA:  # 面积超上限 → 缩小
        s = math.sqrt(MAX_I2I_AREA / float(area))
        w = max(1, int(w * s))
        h = max(1, int(h * s))
        area = w * h
    if area < MIN_I2I_AREA:  # 面积不达下限 → 放大（ceil 保证 ≥ MIN）
        s = math.sqrt(MIN_I2I_AREA / float(area))
        w = max(1, int(math.ceil(w * s)))
        h = max(1, int(math.ceil(h * s)))
    return w, h


def edit_input_bytes(img: Image.Image, max_long_edge: int = 1024, quality: int = 88) -> bytes:
    """把图生图的输入图压成 JPEG 字节：长边 ≤ max_long_edge，且面积夹在网关允许区间。

    - 美颜/风格/滤镜/手账：默认 1024（1K）输入，既省带宽也避免网关 RST；
      小图会被等比放大到满足网关 512×512 面积下限（否则 400）；
    - 超分：调用方传 max_long_edge=3072、quality=95，尽量保留细节，
      但面积会被夹到 2560×2560 上限以内（否则 400）。
    """
    im = img.convert("RGB")
    nw, nh = _fit_i2i_size(im.width, im.height, max_long_edge)
    if (nw, nh) != im.size:
        im = im.resize((nw, nh), Image.LANCZOS)
    buf = BytesIO()
    im.save(buf, "JPEG", quality=quality)
    return buf.getvalue()


def to_image(data: bytes) -> Image.Image:
    return Image.open(BytesIO(data)).convert("RGB")


def stem_of(path: str) -> str:
    base = os.path.basename(path or "")
    return os.path.splitext(base)[0] or "image"


# 本系统各工作流的产物后缀（与各 save_image 调用点一一对应）。
# 用于识别「上一次跑出的成品又被当成这次的输入」——叠加调色/形变会把
# 效果滚 2~3 层（实测：同一张图被跑出 `_胶片_…_自适应调色_出片` 四层后缀）。
OUTPUT_SUFFIXES = (
    "_出片", "_智能修图", "_自适应调色", "_胶片", "_迁移", "_修复",
    "_美颜", "_形变", "_edit",
)


def is_pipeline_output(path: str) -> bool:
    """判断一个路径是不是本系统产出的成品（按文件名后缀）。"""
    stem = stem_of(path)
    return any(stem.endswith(s) for s in OUTPUT_SUFFIXES)


def filter_inputs(paths: list[str], warnings: list[str]) -> list[str]:
    """把历史产物从输入里摘掉并记 warning；全部是产物时抛错（明确提示，不静默）。"""
    keep = [p for p in paths if not is_pipeline_output(p)]
    dropped = len(paths) - len(keep)
    if dropped:
        warnings.append(
            f"已跳过 {dropped} 张历史产物（文件名带 _出片/_智能修图 等后缀），"
            "避免效果叠加；如确需二次处理，请先重命名原图"
        )
    if not keep:
        raise ValueError("所选照片全部是本系统产出的成品，二次处理会叠加效果；请选择原始照片")
    return keep


_PATH_LOCK = threading.Lock()


def unique_path(out_dir: str, name: str, ext: str) -> str:
    """在 `out_dir` 下取一个**不存在的**文件名，并原子地占住它。

    并发安全（照片级并行后这是真需求）：`os.path.exists` 判定与后续写盘之间有窗口，
    两个线程同批处理同名文件（`IMG_1.jpg` 与 `IMG_1.png` 会归一成同一个 stem）时
    会拿到同一个目标路径、互相覆盖。这里用 `O_CREAT|O_EXCL` 原子创建占位，
    谁创建成功谁拿到这个路径；调用方随后正常写入覆盖空文件即可。

    占位失败（父目录不可写等）时退回「只算路径不占位」的旧行为，让真正的写盘
    去报那个更可读的错误。
    """
    os.makedirs(out_dir, exist_ok=True)
    with _PATH_LOCK:
        candidate = os.path.join(out_dir, f"{name}{ext}")
        index = 1
        while True:
            if _reserve(candidate):
                return candidate
            candidate = os.path.join(out_dir, f"{name}_{index}{ext}")
            index += 1


def _reserve(path: str) -> bool:
    """原子占位：成功（含「已被别人占住」）返回 False 表示要换下一个名字。"""
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o666)
    except FileExistsError:
        return False
    except OSError:
        # 目录不可写等：不在这里报，交给真正写盘的那一步给出上下文错误
        return not os.path.exists(path)
    os.close(fd)
    return True


def save_image(img: Image.Image, out_dir: str, name: str, ext: str = ".jpg", quality: int = 95) -> str:
    p = unique_path(out_dir, name, ext)
    if ext.lower() in (".jpg", ".jpeg"):
        img.convert("RGB").save(p, "JPEG", quality=quality)
    else:
        img.save(p)
    return p


def parse_json_loose(text: str) -> Dict[str, Any]:
    """从模型回复里抠出 JSON（容忍 ```json 包裹 / 前后多余解释）。"""
    if not text:
        return {}
    t = text.strip()
    t = re.sub(r"^```(?:json)?", "", t).strip()
    if t.endswith("```"):
        t = t[:-3].strip()
    try:
        v = json.loads(t)
        return v if isinstance(v, dict) else {}
    except Exception:
        pass
    l, r = t.find("{"), t.rfind("}")
    if l >= 0 and r > l:
        try:
            v = json.loads(t[l : r + 1])
            return v if isinstance(v, dict) else {}
        except Exception:
            return {}
    return {}
