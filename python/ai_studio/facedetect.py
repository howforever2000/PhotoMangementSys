"""SCRFD 人脸检测（纯 numpy + Pillow，无 cv2 依赖）。

只做「给一张图，返回人脸框 + 5 点关键点」这一件事，供 restore 工作流在贴回
GFPGAN 修复结果前定位人脸。SCRFD 输出为 stride 8/16/32 三档、每格 2 锚点，
需 distance2bbox 解码；支持 2D 扁平式（本项目 det_10g.onnx）与 4D 通道式两种导出。

解码逻辑源自 python/vcr/services/face_service.py 的向量化实现；这里做的是
**跨微服务的独立精简版**（ai_studio 不 import vcr 的运行时，避免把其 OpenVINO/
设备拓扑一起拖进来）。采样、阈值、NMS 口径与 vcr 一致。
"""
from __future__ import annotations

import numpy as np
from PIL import Image

STRIDES = [8, 16, 32]
DET_SIZE = 640
CONF = 0.5            # 人脸框最低检测分数
NMS_IOU = 0.45
MIN_PIX = 24          # 最小人脸边长（原图像素），过小不修复
MAX_FACES = 16

# ArcFace 标准 5 点模板（112×112），保留供将来做对齐；当前恢复用 bbox 裁切即可
ARC_DST = np.array(
    [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ],
    dtype=np.float32,
)


def _letterbox_tensor(img: Image.Image):
    """SCRFD 预处理：长边等比缩到 640、黑边填充、归一化 (x-127.5)/128，CHW。"""
    w, h = img.size
    r = DET_SIZE / float(max(w, h))
    bw, bh = max(1, int(round(w * r))), max(1, int(round(h * r)))
    base = img.resize((bw, bh), Image.BILINEAR)
    pad_x = (DET_SIZE - bw) // 2
    pad_y = (DET_SIZE - bh) // 2
    canvas = Image.new("RGB", (DET_SIZE, DET_SIZE), (0, 0, 0))
    canvas.paste(base, (pad_x, pad_y))
    arr = np.asarray(canvas, dtype=np.float32)
    arr = (arr - 127.5) / 128.0
    return np.expand_dims(arr.transpose(2, 0, 1), axis=0), r, pad_x, pad_y


def _candidates(stride: int, sc: np.ndarray, bx: np.ndarray, kps: np.ndarray):
    """单个 stride 的输出 → 摊平候选项（cell 主序、anchor 次序）。"""
    if sc.ndim == 4:  # 4D 通道式 (1,C,H,W)
        num_anc, h, w = sc.shape[1], sc.shape[2], sc.shape[3]
        scores = sc[0].transpose(1, 2, 0).reshape(-1)
        boxes = bx[0].reshape(num_anc, 4, h, w).transpose(2, 3, 0, 1).reshape(-1, 4)
        kpsf = kps[0].reshape(num_anc, 10, h, w).transpose(2, 3, 0, 1).reshape(-1, 10)
        cell = np.repeat(np.arange(h * w), num_anc)
    else:  # 2D 扁平式 (N,D)
        n = sc.shape[0]
        num_anc = 2
        cells = n // num_anc
        h = w = int(round(cells ** 0.5))
        scores = sc[:, 0]
        boxes = bx
        kpsf = kps
        cell = np.arange(n) // num_anc
    return scores, boxes, kpsf, cell, w


def _nms(boxes: np.ndarray, scores: np.ndarray, iou_thr: float) -> list[int]:
    if boxes.shape[0] == 0:
        return []
    x1, y1, x2, y2 = boxes[:, 0], boxes[:, 1], boxes[:, 2], boxes[:, 3]
    areas = np.maximum(0.0, x2 - x1) * np.maximum(0.0, y2 - y1)
    order = scores.argsort()[::-1]
    keep: list[int] = []
    while order.size > 0:
        i = int(order[0])
        keep.append(i)
        if order.size == 1:
            break
        rest = order[1:]
        xx1 = np.maximum(x1[i], x1[rest])
        yy1 = np.maximum(y1[i], y1[rest])
        xx2 = np.minimum(x2[i], x2[rest])
        yy2 = np.minimum(y2[i], y2[rest])
        inter = np.maximum(0.0, xx2 - xx1) * np.maximum(0.0, yy2 - yy1)
        union = areas[i] + areas[rest] - inter + 1e-9
        iou = inter / union
        order = rest[iou <= iou_thr]
    return keep


def decode(outputs: list[np.ndarray], scale: float, pad_x: int, pad_y: int) -> list[dict]:
    """SCRFD 输出 → 原图坐标人脸列表 [{bbox:(x1,y1,x2,y2),kps:(5,2),score}]。"""

    def kind(o: np.ndarray) -> str:
        if o.ndim == 4:
            return {2: "score", 8: "bbox", 20: "kps"}.get(o.shape[1], "?")
        return {1: "score", 4: "bbox", 10: "kps"}.get(o.shape[1], "?")

    groups: dict[str, list[np.ndarray]] = {"score": [], "bbox": [], "kps": []}
    for o in outputs:
        k = kind(o)
        if k != "?":
            groups[k].append(o)
    for k in groups:
        groups[k].sort(
            key=lambda o: o.shape[0] * o.shape[-2] if o.ndim == 4 else o.shape[0],
            reverse=True,
        )

    score_parts, box_parts, kps_parts = [], [], []
    for stride, sc, bx, kps in zip(STRIDES, groups["score"], groups["bbox"], groups["kps"]):
        scores, boxes, kpsf, cell, w = _candidates(stride, sc, bx, kps)
        idx = np.flatnonzero(~(scores.astype(np.float64) < CONF))
        if idx.size == 0:
            continue
        stride_f = np.float32(stride)
        cx = ((cell[idx] % w) * stride).astype(np.float32)
        cy = ((cell[idx] // w) * stride).astype(np.float32)
        d = boxes[idx].astype(np.float32) * stride_f
        pts = kpsf[idx].reshape(-1, 5, 2).astype(np.float32) * stride_f
        pts = pts + np.stack([cx, cy], axis=1)[:, None, :]
        score_parts.append(scores[idx])
        box_parts.append(
            np.stack(
                [
                    (cx - d[:, 0] - pad_x) / scale,
                    (cy - d[:, 1] - pad_y) / scale,
                    (cx + d[:, 2] - pad_x) / scale,
                    (cy + d[:, 3] - pad_y) / scale,
                ],
                axis=1,
            )
        )
        kps_parts.append((pts - np.array([pad_x, pad_y])) / scale)

    if not score_parts:
        return []
    scores = np.concatenate(score_parts)
    boxes = np.concatenate(box_parts)
    kpss = np.concatenate(kps_parts)
    rounded = np.round(boxes).astype(np.int64)
    keep = _nms(boxes, scores, NMS_IOU)
    faces: list[dict] = []
    for i in keep:
        x1, y1, x2, y2 = (
            int(rounded[i, 0]),
            int(rounded[i, 1]),
            int(rounded[i, 2]),
            int(rounded[i, 3]),
        )
        if min(x2 - x1, y2 - y1) < MIN_PIX:
            continue
        faces.append({"bbox": (x1, y1, x2, y2), "kps": kpss[i], "score": float(scores[i])})
    return faces[:MAX_FACES]


def detect(session, img: Image.Image) -> list[dict]:
    """用已加载的 SCRFD session 检测一张图。"""
    tensor, scale, pad_x, pad_y = _letterbox_tensor(img)
    iname = session.get_inputs()[0].name
    outputs = session.run(None, {iname: tensor})
    return decode(outputs, scale, pad_x, pad_y)
