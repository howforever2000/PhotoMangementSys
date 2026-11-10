"""工作流公共工具（FEAT-152）

读图 / 转字节 / 落盘 / 取消检查 / JSON 宽松解析 —— 供四个工作流共享，
避免每个 workfow 各写一份 IO 细节。
"""
from __future__ import annotations

import json
import os
import re
from io import BytesIO
from typing import Any, Callable, Dict, Optional

from PIL import Image, ImageOps


class Cancelled(Exception):
    """用户取消作业。"""


Progress = Callable[[int, str, str], None]


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
