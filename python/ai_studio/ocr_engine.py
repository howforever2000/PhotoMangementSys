"""PaddleOCR det+rec 本机推理引擎（FEAT-171）。

为什么本模块是纯 numpy + Pillow：ai_studio 打包刻意 excludes cv2
（`ai-studio.spec`，见 facewarp.py 头注），扫描侧 `vcr/services/ocr_service.py`
的 DB 后处理实现用不了 —— 这里按同一算法语义用 numpy 重写：

  det（ch_PP-OCRv4_det_mobile，与扫描入库共享）：
    letterbox 640 灰底 114 → /255 → ImageNet mean/std → 概率图 (1,1,H,W)
    → 阈值二值化 → 3×3 膨胀一次 → 8 连通域（BFS，替代 cv2.findContours）
    → 框内平均概率过滤 → 按 stride 还原 → 去灰边 → 原图横向文本行框
    （stride 由输出分辨率现算：RapidOCR 导出是 640 全分辨率，PP-OCR 官方导出是 H/4）
  rec（ch_PP-OCRv4_rec_mobile，工坊区下载）：
    行裁剪 → h=48 等比缩放、右补黑到 w=320 → (x/255-0.5)/0.5
    → (1,T,C) logits → CTC 贪心解码（0=blank，字典 ppocr_keys_v1.txt 随包内置）

模型路径必须走 `config.OCR_MODELS`（AI_STUDIO_MODELS_DIR 注入口径，
BUG-2026-1112-005：打包版自行推算会「下载完成但工具报未就绪」）；
就绪判定与 localmodels.model_ready 同口径，会话缓存复用 localmodels.get_session。
"""
from __future__ import annotations

import os

import numpy as np
from PIL import Image

from . import config, localmodels
from .logutil import call_model

DET_SIZE = 640              # det letterbox 边长（与扫描侧 DET_SIZE 同口径）
DB_STRIDE = 4               # DB 概率图相对输入的下采样倍数
PROB_THRESH = 0.5           # 概率图二值化阈值（与扫描侧 OCR_PROB_THRESH 同）
BOX_THRESH = 0.5            # 框内平均概率阈值（DB box_thresh）
MIN_BOX_PIX = 6             # 文本框最小边长（原图像素）
_REC_H, _REC_W = 48, 320    # PP-OCRv4 rec 输入尺寸

_MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
_STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)

_keys_cache: list[str] | None = None


def det_ready() -> bool:
    """检测模型是否就绪（扫描区共享：模型管理「文字 · OCR 检测」）。"""
    return os.path.isfile(config.OCR_MODELS["paddleocr_det"])


def rec_ready() -> bool:
    """识别模型 + 字典是否就绪（工坊区下载）。"""
    return localmodels.model_ready("paddleocr-rec.onnx") and os.path.isfile(
        config.OCR_KEYS_PATH
    )


def _load_keys() -> list[str]:
    """载入 CTC 字典：chars[0]=blank 占位，chars[1..N]=字典行，第 N+1 类=空格。"""
    global _keys_cache
    if _keys_cache is None:
        with open(config.OCR_KEYS_PATH, "r", encoding="utf-8") as fh:
            _keys_cache = [""] + fh.read().splitlines()
    return _keys_cache


def _det_tensor(img: Image.Image) -> tuple[np.ndarray, float, int, int]:
    """det 预处理：letterbox 640 灰底 114 + ImageNet 归一。返回 (tensor, scale, pad_x, pad_y)。"""
    w, h = img.size
    scale = DET_SIZE / max(w, h)
    img2 = img.resize((max(1, round(w * scale)), max(1, round(h * scale))), Image.BILINEAR)
    pad_x = (DET_SIZE - img2.size[0]) // 2
    pad_y = (DET_SIZE - img2.size[1]) // 2
    canvas = Image.new("RGB", (DET_SIZE, DET_SIZE), (114, 114, 114))
    canvas.paste(img2, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    arr = (arr - _MEAN) / _STD
    return arr.transpose(2, 0, 1)[None].astype(np.float32), scale, pad_x, pad_y


def _dilate3x3(bitmap: np.ndarray) -> np.ndarray:
    """3×3 全核膨胀一次（与 cv2.dilate MORPH_RECT(3,3) 等价）：max-filter 平移叠或。"""
    h, w = bitmap.shape
    padded = np.pad(bitmap, 1, mode="constant")
    out = np.zeros_like(bitmap)
    for dy in range(3):
        for dx in range(3):
            out |= padded[dy : dy + h, dx : dx + w]
    return out


def _label_components(bitmap: np.ndarray) -> tuple[np.ndarray, int]:
    """8 连通域标记（BFS 洪水填充，替代 cv2.findContours + minAreaRect）。

    概率图只有 160×160，纯 Python 循环每图几万步、毫秒级，不需要向量化。
    返回 (labels, n)：labels 从 1 编号，0 为背景。
    """
    h, w = bitmap.shape
    labels = np.zeros((h, w), dtype=np.int32)
    cur = 0
    for sy in range(h):
        for sx in range(w):
            if bitmap[sy, sx] and labels[sy, sx] == 0:
                cur += 1
                labels[sy, sx] = cur
                stack = [(sy, sx)]
                while stack:
                    y, x = stack.pop()
                    for ny in range(max(0, y - 1), min(h, y + 2)):
                        for nx in range(max(0, x - 1), min(w, x + 2)):
                            if bitmap[ny, nx] and labels[ny, nx] == 0:
                                labels[ny, nx] = cur
                                stack.append((ny, nx))
    return labels, cur


def _reading_order(boxes: list[tuple[float, float, float, float]]) -> list[tuple[float, float, float, float]]:
    """按阅读顺序排序：先按行（行高 80% 容差归并），行内从左到右。"""
    def key(b: tuple[float, float, float, float]):
        x1, y1, x2, y2 = b
        line_h = max(12.0, (y2 - y1) * 0.8)
        return (round(y1 / line_h), x1)
    return sorted(boxes, key=key)


def find_text_lines(img: Image.Image) -> list[tuple[float, float, float, float]]:
    """det：返回原图坐标系的横向文本行框 [(x1,y1,x2,y2)]（已按阅读顺序排序）。"""
    sess = localmodels.get_session(config.OCR_MODELS["paddleocr_det"])
    tensor, scale, pad_x, pad_y = _det_tensor(img)
    with call_model("onnx", "paddleocr-det.onnx", f"{img.size[0]}x{img.size[1]}"):
        out = sess.run(None, {sess.get_inputs()[0].name: tensor})[0]
    prob = np.asarray(out[0, 0], dtype=np.float32)  # (Hout,Wout) sigmoid 概率图

    # 输出分辨率随导出而异（本 RapidOCR 导出是 640×640 全分辨率；有的导出是 H/4），
    # 不能写死 ×4：按输入/输出边长现算 stride，两种导出都落对坐标。
    stride = max(1, DET_SIZE // prob.shape[0])

    bitmap = (prob > PROB_THRESH).astype(np.uint8)
    bitmap = _dilate3x3(bitmap)                     # 修补断笔（与扫描侧同参数）
    labels, n = _label_components(bitmap)

    w0, h0 = img.size
    img_area = float(w0 * h0)
    boxes: list[tuple[float, float, float, float]] = []
    for cid in range(1, n + 1):
        ys, xs = np.nonzero(labels == cid)
        if ys.size < 9:                             # 与扫描侧 contourArea>=9 同口径
            continue
        # 框平均概率（连通域像素即框内像素，比 fillPoly 近似更精确）
        if float(prob[ys, xs].mean()) < BOX_THRESH:
            continue
        # 连通域外接框（概率图）→ 近似 unclip 外扩 30% → ×stride 还原 → 去 letterbox → 原图
        x1p, x2p = float(xs.min()), float(xs.max()) + 1.0
        y1p, y2p = float(ys.min()), float(ys.max()) + 1.0
        gx, gy = (x2p - x1p) * 0.30, (y2p - y1p) * 0.30
        x1 = ((x1p - gx) * stride - pad_x) / scale
        x2 = ((x2p + gx) * stride - pad_x) / scale
        y1 = ((y1p - gy) * stride - pad_y) / scale
        y2 = ((y2p + gy) * stride - pad_y) / scale
        x1, y1 = max(0.0, x1), max(0.0, y1)
        x2, y2 = min(float(w0), x2), min(float(h0), y2)
        wb, hb = x2 - x1, y2 - y1
        # 横排判据保留（宽>高）；但不沿用扫描侧的 宽/高≤10 上限 —— 那是给
        # 面积占比统计兜纹理误检的，提取场景会把正常的短行文本（比值常到 15+）
        # 整行丢掉。纹理碎块另有两道闸：最小边长 + 单框面积上限。
        if min(wb, hb) < MIN_BOX_PIX or wb <= hb:
            continue
        if wb * hb > img_area * 0.2:                # 单框超 20% 画面 → 纹理连片误检
            continue
        boxes.append((x1, y1, x2, y2))
    return _reading_order(boxes)


def _rec_tensor(crop: Image.Image) -> np.ndarray:
    """rec 预处理：h=48 等比缩放，右补黑到 w=320，(x/255-0.5)/0.5。"""
    crop = crop.convert("RGB")
    w, h = crop.size
    nw = min(_REC_W, max(1, round(w * _REC_H / max(h, 1))))
    img2 = crop.resize((nw, _REC_H), Image.BILINEAR)
    canvas = Image.new("RGB", (_REC_W, _REC_H), (0, 0, 0))
    canvas.paste(img2, (0, 0))
    arr = np.asarray(canvas, dtype=np.float32) / 255.0
    arr = (arr - 0.5) / 0.5
    return arr.transpose(2, 0, 1)[None].astype(np.float32)


def ctc_decode(logits: np.ndarray, chars: list[str]) -> tuple[str, float]:
    """CTC 贪心解码：逐时刻 argmax → 去重 → 去 blank(0)；返回 (文本, 平均字符置信度)。

    类别布局：0=blank，1..N=字典行，N+1=空格（PP-OCR ch rec 带 use_space_char）。
    """
    flat = logits.reshape(-1, logits.shape[-1])
    seq = flat.argmax(axis=-1)
    conf = flat.max(axis=-1)
    n_dict = len(chars) - 1
    out: list[str] = []
    scores: list[float] = []
    prev = -1
    for idx, p in zip(seq.tolist(), conf.tolist()):
        if idx == prev:
            continue
        prev = idx
        if idx == 0:
            continue
        if idx <= n_dict:
            out.append(chars[idx])
            scores.append(p)
        elif idx == n_dict + 1:
            out.append(" ")
        # 超出已知类别数：防御性跳过，不让一个离群类别毁掉整行
    text = "".join(out)
    score = sum(scores) / len(scores) if scores else 0.0
    return text, score


def recognize(img: Image.Image) -> tuple[list[str], float, int]:
    """完整识别：det 找框 → rec 认字。返回 (按阅读顺序的文本行, 平均置信度, 框数)。"""
    boxes = find_text_lines(img)
    if not boxes:
        return [], 0.0, 0
    chars = _load_keys()
    sess = localmodels.get_session(config.OCR_MODELS["paddleocr_rec"])
    iname = sess.get_inputs()[0].name
    lines: list[str] = []
    scores: list[float] = []
    # 一张图可能切出几十行：整段 rec 记一行（带行数），不逐行刷日志。
    with call_model("onnx", "paddleocr-rec.onnx", f"lines={len(boxes)}"):
        for x1, y1, x2, y2 in boxes:
            crop = img.crop(
                (int(round(x1)), int(round(y1)), int(round(x2)), int(round(y2)))
            )
            logits = sess.run(None, {iname: _rec_tensor(crop)})[0]
            text, conf = ctc_decode(np.asarray(logits), chars)
            text = text.strip()
            if text:
                lines.append(text)
                scores.append(conf)
    score = sum(scores) / len(scores) if scores else 0.0
    return lines, score, len(boxes)
