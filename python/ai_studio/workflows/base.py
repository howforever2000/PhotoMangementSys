"""工作流公共工具（FEAT-152）

读图 / 转字节 / 落盘 / 取消检查 / JSON 宽松解析 —— 供四个工作流共享，
避免每个 workfow 各写一份 IO 细节。
"""
from __future__ import annotations

import json
import math
import os
import re
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
    img = Image.open(path)
    img = ImageOps.exif_transpose(img)
    return img.convert("RGB")


def png_bytes(img: Image.Image) -> bytes:
    buf = BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


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


def unique_path(out_dir: str, name: str, ext: str) -> str:
    os.makedirs(out_dir, exist_ok=True)
    p = os.path.join(out_dir, f"{name}{ext}")
    if not os.path.exists(p):
        return p
    i = 1
    while os.path.exists(os.path.join(out_dir, f"{name}_{i}{ext}")):
        i += 1
    return os.path.join(out_dir, f"{name}_{i}{ext}")


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
