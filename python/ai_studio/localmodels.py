"""本地 ONNX 模型推理（Real-ESRGAN 超分 / GFPGAN 人脸修复 / SCRFD 人脸检测）。

只做「模型会话管理 + 三种前向推理」这一件事，供 restore 工作流复用：
  - get_session    按路径缓存 onnxruntime 会话，优先 DirectML，失败回落 CPU
  - run_esrgan     分块超分（4× 模型），避免大图一次性占满内存
  - run_gfpgan     单张 512 对齐脸的修复
  - detect_faces   交给 facedetect 做 SCRFD 检测

onnxruntime 采用惰性导入：胶片/颜色/其它工具在没装 ORT 的环境也能正常跑，
只有真正调用超分/人脸时才要求它可用。
"""
from __future__ import annotations

import os
from typing import Optional

import numpy as np
from PIL import Image

from . import config, facedetect

_sessions: dict[str, object] = {}


def _ort():
    import onnxruntime as ort  # 惰性导入

    return ort


def _providers() -> list[str]:
    ort = _ort()
    avail = set(ort.get_available_providers())
    pref = ["DmlExecutionProvider", "CUDAExecutionProvider", "CPUExecutionProvider"]
    chosen = [p for p in pref if p in avail]
    return chosen or ["CPUExecutionProvider"]


def get_session(path: str):
    """按绝对路径缓存会话；文件缺失时抛出带中文提示的 FileNotFoundError。"""
    if not os.path.isfile(path):
        raise FileNotFoundError(os.path.basename(path))
    key = os.path.abspath(path)
    sess = _sessions.get(key)
    if sess is None:
        ort = _ort()
        try:
            sess = ort.InferenceSession(key, providers=_providers())
        except Exception:
            sess = ort.InferenceSession(key, providers=["CPUExecutionProvider"])
        _sessions[key] = sess
    return sess


def esrgan_ready() -> bool:
    return os.path.isfile(config.local_model_path("real_esrgan_x4.onnx"))


def gfpgan_ready() -> bool:
    return os.path.isfile(config.local_model_path("gfpgan_1.4.onnx"))


def face_det_ready() -> bool:
    return os.path.isfile(config.FACE_DET_MODEL)


def run_esrgan(img: Image.Image, tile: int = 256, overlap: int = 16, cancel=None) -> Image.Image:
    """4× 超分（分块推理，块间留 overlap 并只写核心区以避免接缝）。返回 PIL RGB。"""
    from .workflows.base import check_cancel

    sess = get_session(config.local_model_path("real_esrgan_x4.onnx"))
    iname = sess.get_inputs()[0].name
    img = img.convert("RGB")
    w, h = img.size
    arr = np.asarray(img, dtype=np.float32) / 255.0
    out = np.empty((h * 4, w * 4, 3), dtype=np.uint8)

    t = max(64, int(tile))
    o = max(0, min(int(overlap), t // 2 - 1))
    stride = max(1, t - 2 * o)

    for y in range(0, h, stride):
        y0 = min(y, h - 1)
        y1 = min(y0 + t, h)
        for x in range(0, w, stride):
            x0 = min(x, w - 1)
            x1 = min(x0 + t, w)
            check_cancel(cancel)
            sub = arr[y0:y1, x0:x1]
            xin = sub.transpose(2, 0, 1)[None]
            res = sess.run(None, {iname: xin})[0][0]  # (3, 4h, 4w)
            res = np.clip(res.transpose(1, 2, 0), 0.0, 1.0)
            res_u8 = (res * 255.0 + 0.5).astype(np.uint8)
            # 只写核心区（边缘块保留到图像边界）
            ly0 = o if y0 > 0 else 0
            lx0 = o if x0 > 0 else 0
            ly1 = (sub.shape[0] - o) if y1 < h else sub.shape[0]
            lx1 = (sub.shape[1] - o) if x1 < w else sub.shape[1]
            out[(y0 + ly0) * 4 : (y0 + ly1) * 4, (x0 + lx0) * 4 : (x0 + lx1) * 4] = res_u8[
                ly0 * 4 : ly1 * 4, lx0 * 4 : lx1 * 4
            ]
    return Image.fromarray(out)


def run_gfpgan(face512: Image.Image) -> Image.Image:
    """GFPGAN 1.4：输入 512×512 人脸，输出同尺寸修复结果。"""
    sess = get_session(config.local_model_path("gfpgan_1.4.onnx"))
    iname = sess.get_inputs()[0].name
    face = face512.convert("RGB").resize((512, 512), Image.LANCZOS)
    x = np.asarray(face, dtype=np.float32) / 255.0
    x = (x - 0.5) / 0.5
    x = x.transpose(2, 0, 1)[None]
    out = sess.run(None, {iname: x})[0][0]
    y = np.clip(out.transpose(1, 2, 0) * 0.5 + 0.5, 0.0, 1.0)
    return Image.fromarray((y * 255.0 + 0.5).astype(np.uint8))


def detect_faces(img: Image.Image) -> list[dict]:
    """SCRFD 检测，返回 [{bbox,kps,score}]。"""
    sess = get_session(config.FACE_DET_MODEL)
    return facedetect.detect(sess, img)


def restore_faces(upscaled: Image.Image, faces: list[dict], ratio: float) -> Image.Image:
    """把每张检测到的人脸从「已超分图」裁出、GFPGAN 修复后羽化贴回。"""
    out = upscaled.copy()
    cw, ch = out.size
    for f in faces:
        x1, y1, x2, y2 = f["bbox"]
        bw, bh = x2 - x1, y2 - y1
        if bw <= 0 or bh <= 0:
            continue
        # 映射到超分图坐标 + 外扩边距（给 GFPGAN 更多上下文）
        mx, my = int(bw * 0.35 * ratio), int(bh * 0.35 * ratio)
        bx1 = max(0, int(x1 * ratio) - mx)
        by1 = max(0, int(y1 * ratio) - my)
        bx2 = min(cw, int(x2 * ratio) + mx)
        by2 = min(ch, int(y2 * ratio) + my)
        if bx2 - bx1 < 16 or by2 - by1 < 16:
            continue
        crop = out.crop((bx1, by1, bx2, by2))
        fixed = run_gfpgan(crop).resize((bx2 - bx1, by2 - by1), Image.LANCZOS)
        mask = _feather_mask(bx2 - bx1, by2 - by1)
        out.paste(fixed, (bx1, by1), mask)
    return out


def _feather_mask(w: int, h: int) -> Image.Image:
    ys = np.linspace(-1.0, 1.0, h, dtype=np.float32)[:, None]
    xs = np.linspace(-1.0, 1.0, w, dtype=np.float32)[None, :]
    d = np.sqrt(xs * xs + ys * ys) / np.sqrt(2.0)
    alpha = np.clip((1.0 - d) / 0.35, 0.0, 1.0)  # 中心 1，边缘渐隐
    return Image.fromarray((alpha * 255).astype(np.uint8), mode="L")
