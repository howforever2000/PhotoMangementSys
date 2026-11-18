"""68 点人脸关键点（peppa_wutz ONNX）

为什么选这个模型（2026-11-13 实测，见 `.dsh/knowledge/local-enhance.md` §10）：
  - 单模型直出 68 点，不需要「先 5 点再拟合」的两段推理；
  - 13.1MB / Apache-2.0，比 PIPNet R18(45.7MB) 与 2dfan4(93.4MB) 都小；
  - 自带逐点置信度（输出的第三维），可直接做质量闸门；
  - 与已有 `det_10g` 交叉验证：眼中心误差 2.4~2.7px、瞳距误差 0.15%；
    旋转 -30°~40° 置信度仍 0.92~0.94，脸宽 72px 仍 0.915。

本模块只做「图 + 检测框 → 68 点（原图坐标）」这一件事：
  - 预处理在这里（bbox 裁切 + roll 对齐 + 归一化），形变在 `facewarp.py`；
  - 读图 / 落盘 / 进度在 `workflows/reshape.py`。

模型 IO（实测）：
  input  `input`  float32 [1,3,256,256]，RGB，**/255 归一化（不是 ImageNet 均值方差）**
  output `output` float32 [1,68,3]，前两维是 x/y（0~64 量纲，×4 得 256 空间坐标），
         第三维是该点的置信度（0~1）

## 对齐为什么要带 roll（踩过的坑）

peppa_wutz 期望**正立的人脸**。若只做「bbox 居中 + 缩放」而不旋转，歪头 25° 的照片
会让整条下颌线落到脖子上（实测：下巴相对 bbox 高度从 1.00 掉到 0.28）。
旋转必须**绕裁切图中心 (128,128)** 进行，写成 `Tr(c)·R·Tr(-c)·S`；
早一版绕原点旋转，同样会把点甩出去。这类错误看文档发现不了，只能靠
「旋转测试 + 下巴相对高度」这个客观指标抓出来。
"""
from __future__ import annotations

import math

import numpy as np
from PIL import Image

from . import localmodels
from .logutil import call_model

MODEL_FILE = "peppa_wutz.onnx"

# 模型输入边长与「bbox 长边映射到该值」的系数（与上游 facefusion 一致）
_INPUT_SIZE = 256
_BBOX_TO_INPUT = 195.0
# 输出坐标量纲：模型给 0~64，乘 4 得 256 空间
_COORD_SCALE = 64.0

# 低于此置信度的脸不动（宁可不动，也不要动歪）
MIN_CONFIDENCE = 0.70
# 小于此边长的人脸不处理：256 输入下细节不可信
MIN_FACE_PX = 40

# 68 点的语义分段（0-based，与 300W / iBUG 布局一致）
JAW = tuple(range(0, 17))                 # 下颌线 0~16
LEFT_BROW = tuple(range(17, 22))
RIGHT_BROW = tuple(range(22, 27))
NOSE_BRIDGE = tuple(range(27, 31))
NOSE_BOTTOM = tuple(range(31, 36))
LEFT_EYE = tuple(range(36, 42))
RIGHT_EYE = tuple(range(42, 48))
OUTER_MOUTH = tuple(range(48, 60))
INNER_MOUTH = tuple(range(60, 68))


def ready() -> bool:
    """关键点模型是否就位。"""
    return localmodels.model_ready(MODEL_FILE)


def _rotation_scale_matrix(scale: float, dx: float, dy: float, roll: float) -> np.ndarray:
    """原图坐标 → 256 裁切空间的 3×3 变换。

    顺序：先缩放平移把 bbox 摆到中心（S），再绕裁切图中心反向旋转 roll（R 绕 c）。
    `roll>0` 表示人脸在图中顺时针歪，这里反向转正。
    """
    s = np.array([[scale, 0.0, dx], [0.0, scale, dy], [0.0, 0.0, 1.0]], dtype=np.float64)
    ca, sa = math.cos(-roll), math.sin(-roll)
    c = _INPUT_SIZE / 2.0
    to_c = np.array([[1.0, 0.0, c], [0.0, 1.0, c], [0.0, 0.0, 1.0]], dtype=np.float64)
    rot = np.array([[ca, -sa, 0.0], [sa, ca, 0.0], [0.0, 0.0, 1.0]], dtype=np.float64)
    back = np.array([[1.0, 0.0, -c], [0.0, 1.0, -c], [0.0, 0.0, 1.0]], dtype=np.float64)
    return to_c @ rot @ back @ s


def detect_68(img: Image.Image, bbox, kps=None) -> tuple[np.ndarray, float] | None:
    """对一张图里的一张脸出 68 点。

    参数：
      img  —— 原图（PIL RGB）
      bbox —— (x1, y1, x2, y2) 原图坐标（`localmodels.detect_faces` 的 bbox）
      kps  —— 可选的 5 点 (5,2)，用于算 roll；缺省则视为正立

    返回 `(landmarks[68,2] 原图坐标, confidence)`；人脸过小返回 None。
    """
    x1, y1, x2, y2 = (float(v) for v in bbox)
    w, h = x2 - x1, y2 - y1
    if min(w, h) < MIN_FACE_PX:
        return None

    scale = _BBOX_TO_INPUT / max(w, h)
    dx = (_INPUT_SIZE - (x2 + x1) * scale) * 0.5
    dy = (_INPUT_SIZE - (y2 + y1) * scale) * 0.5

    roll = 0.0
    if kps is not None:
        k = np.asarray(kps, dtype=np.float64)
        if k.shape[0] >= 2:
            # 双眼连线角度（kps[0] 左眼、kps[1] 右眼，SCRFD 口径）
            roll = math.atan2(float(k[1][1] - k[0][1]), float(k[1][0] - k[0][0]))

    matrix = _rotation_scale_matrix(scale, dx, dy, roll)
    inverse = np.linalg.inv(matrix)

    # PIL 的 AFFINE 吃 「输出 → 输入」 的矩阵，所以传 inverse 的前两行
    crop = img.transform(
        (_INPUT_SIZE, _INPUT_SIZE),
        Image.AFFINE,
        tuple(inverse[:2].ravel()),
        Image.BILINEAR,
    )

    sess = localmodels.get_session(config_path())
    blob = (np.asarray(crop, dtype=np.float32) / 255.0).transpose(2, 0, 1)[None]
    with call_model("onnx", "peppa_wutz.onnx (landmark68)", "68pts"):
        raw = sess.run(None, {sess.get_inputs()[0].name: blob})[0][0]

    points = raw[:, :2] / _COORD_SCALE * _INPUT_SIZE          # → 256 空间
    confidence = float(raw[:, 2].mean()) if raw.shape[1] >= 3 else 1.0

    # 反算回原图：origin = inverse @ [x, y, 1]
    homogeneous = np.hstack([points, np.ones((points.shape[0], 1), dtype=np.float64)])
    restored = homogeneous @ inverse.T
    return restored[:, :2], confidence


def config_path() -> str:
    """模型绝对路径（与下载落位同一口径，见 `knowledge/local-enhance.md` §5）。"""
    from . import config

    return config.local_model_path(MODEL_FILE)


def face_metrics(landmarks: np.ndarray) -> dict:
    """从 68 点推出形状度量（供形变参数与自检使用）。"""
    left_eye = landmarks[list(LEFT_EYE)].mean(axis=0)
    right_eye = landmarks[list(RIGHT_EYE)].mean(axis=0)
    return {
        "eye_left": left_eye,
        "eye_right": right_eye,
        "eye_distance": float(np.linalg.norm(right_eye - left_eye)),
        "face_width": float(landmarks[16][0] - landmarks[0][0]),
        "chin": landmarks[8].copy(),
        "nose_tip": landmarks[30].copy(),
        "center_x": float((left_eye[0] + right_eye[0]) / 2.0),
        "top_y": float(landmarks[27][1]),
    }


__all__ = [
    "JAW",
    "LEFT_BROW",
    "RIGHT_BROW",
    "NOSE_BRIDGE",
    "NOSE_BOTTOM",
    "LEFT_EYE",
    "RIGHT_EYE",
    "OUTER_MOUTH",
    "INNER_MOUTH",
    "MIN_CONFIDENCE",
    "MIN_FACE_PX",
    "MODEL_FILE",
    "detect_68",
    "face_metrics",
    "ready",
]
