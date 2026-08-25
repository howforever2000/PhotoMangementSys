"""服务层：文档 OCR 通道（Phase 4，可选）

真 OCR：PaddleOCR ch_PP-OCRv4_det（仅 DB 检测，不做 rec）。模型缺失时
ready=False，仲裁器回退到截图启发式（document 的 sub_category=screenshot）。

DB 后处理（与 PaddleOCR DBPostProcess 对齐，但不依赖 pyclipper）：
  letterbox 640 → 概率图 → prob>0.3 二值化 → 形态学膨胀（unclip 近似）
  → 连通域 minAreaRect → box 平均概率 >0.5 过滤 → 映射回原图 → 文字面积占比

输出：文字框面积占比 area_ratio（≥OCR_AREA_STRONG 且 ≥2 框为强证据）。
"""
import os
from dataclasses import dataclass

import cv2
import numpy as np
from PIL import Image

from .. import config, preprocess


@dataclass
class OcrOutcome:
    area_ratio: float = 0.0
    n_boxes: int = 0
    ready: bool = False
    error: str = ""


DB_STRIDE = 4                # DB 概率图相对输入的下采样倍数
MIN_BOX_PIX = 6              # 文本框最小边长（原图像素）


class OcrService:
    def __init__(self, registry):
        self.registry = registry

    def ready(self) -> bool:
        self.registry.ocr  # 属性访问触发惰性加载
        return self.registry.is_ready("ocr")

    # ------------------------------------------------------------------
    def run(self, img: Image.Image) -> OcrOutcome:
        if not self.ready():
            return OcrOutcome(error="OCR 模型缺失")
        try:
            tensor, scale, pad_x, pad_y = preprocess.ocr_tensor(img)
            out = self.registry.run("ocr", tensor)[0]      # (1,1,H/4,W/4) 概率图（sigmoid）
            prob = np.asarray(out[0, 0], dtype=np.float32)
        except Exception as e:  # noqa: BLE001
            return OcrOutcome(error=f"OCR 推理失败: {e}")

        # 1) 概率图二值化
        bitmap = (prob > config.OCR_PROB_THRESH).astype(np.uint8)
        # 2) 轻微形态学膨胀（1 次 3x3 ≈ 原图 12px，仅修补断笔；膨胀过大会把纹理连成巨型框）
        kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (3, 3))
        bitmap = cv2.dilate(bitmap, kernel, iterations=1)

        # 3) 连通域 → minAreaRect 文本框（概率图坐标，×DB_STRIDE 到输入图坐标）
        contours, _ = cv2.findContours(bitmap, cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
        total_area = 0.0
        n_boxes = 0
        img_area = float(img.size[0] * img.size[1])
        for c in contours:
            ca = float(cv2.contourArea(c))
            if ca < 9:
                continue
            rect = cv2.minAreaRect(c)
            box = cv2.boxPoints(rect)                       # (4,2) 概率图坐标
            # 4) box 平均概率过滤（DB box_thresh）
            mean_p = _mean_prob(prob, box)
            if mean_p < config.OCR_BOX_THRESH:
                continue
            # 5) 概率图 → 输入图坐标（×DB_STRIDE）→ letterbox 去除 → 原图坐标
            box_in = box * DB_STRIDE
            box_orig = (box_in - np.array([pad_x, pad_y])) / scale
            x1, y1 = box_orig[:, 0].min(), box_orig[:, 1].min()
            x2, y2 = box_orig[:, 0].max(), box_orig[:, 1].max()
            wb, hb = x2 - x1, y2 - y1
            # 文字行是横向的（宽>高 且 宽/高<10）；纹理碎块/竖向碎片丢弃（手册回退方案）
            if min(wb, hb) < MIN_BOX_PIX or wb <= hb or wb / max(hb, 1e-6) > 10:
                continue
            box_area = float(cv2.contourArea(box_orig.astype(np.float32)))
            if box_area > img_area * 0.2:                    # 单框超 20% 画面 → 纹理连片误检
                continue
            total_area += box_area
            n_boxes += 1

        return OcrOutcome(
            area_ratio=round(total_area / img_area, 4) if img_area > 0 else 0.0,
            n_boxes=n_boxes,
            ready=True,
        )


def _mean_prob(prob: np.ndarray, box: np.ndarray) -> float:
    """计算四边形框内像素的平均概率（box 得分）。"""
    h, w = prob.shape
    mask = np.zeros((h, w), dtype=np.uint8)
    try:
        cv2.fillPoly(mask, [np.round(box).astype(np.int32)], 1)
    except Exception:
        return 0.0
    vals = prob[mask == 1]
    return float(vals.mean()) if vals.size else 0.0


_ocr_service: OcrService | None = None


def get_ocr_service(registry) -> OcrService:
    global _ocr_service
    if _ocr_service is None:
        _ocr_service = OcrService(registry)
    return _ocr_service
