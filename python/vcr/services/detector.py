"""服务层：目标检测通道（YOLOv8n-det, COCO 80 类）

职责：
  1. 全图推理，NMS 聚合 person 框（旧实现用锚点最大值，1 个人有 9~10 个锚点
     导致人数不可统计——P1 修复）
  2. 产出 person 统计：NMS 后人数 / 最大人框面积占比 / 最大置信度，供仲裁器
  3. 输出原始人框坐标，供人脸标号通道裁剪
"""
from dataclasses import dataclass, field

import numpy as np
from PIL import Image

from .. import config, preprocess

PERSON_CLASS_ID = 0          # COCO 索引 0 = person
VEHICLE_CLASS_IDS = (2, 3, 5, 7)   # car / motorcycle / bus / truck
BOX_HEAD = 4                 # 检测输出每锚点前 4 行为 x,y,w,h


@dataclass
class Box:
    x1: float
    y1: float
    x2: float
    y2: float
    conf: float


@dataclass
class DetOutcome:
    persons: list[Box] = field(default_factory=list)
    count: int = 0
    max_conf: float = 0.0
    max_area_ratio: float = 0.0
    vehicles: list[Box] = field(default_factory=list)   # 车辆框（供仲裁器密集车流判定）
    vehicle_count: int = 0
    ready: bool = False
    error: str = ""


def _nms(boxes: list[Box], iou_thr: float) -> list[Box]:
    """标准 IoU NMS，按置信度降序贪心抑制。"""
    if not boxes:
        return []
    boxes = sorted(boxes, key=lambda b: b.conf, reverse=True)
    keep: list[Box] = []
    while boxes:
        best = boxes.pop(0)
        keep.append(best)
        boxes = [b for b in boxes if _iou(best, b) <= iou_thr]
    return keep


def _iou(a: Box, b: Box) -> float:
    x1 = max(a.x1, b.x1)
    y1 = max(a.y1, b.y1)
    x2 = min(a.x2, b.x2)
    y2 = min(a.y2, b.y2)
    inter = max(0.0, x2 - x1) * max(0.0, y2 - y1)
    area_a = (a.x2 - a.x1) * (a.y2 - a.y1)
    area_b = (b.x2 - b.x1) * (b.y2 - b.y1)
    union = area_a + area_b - inter
    return inter / union if union > 0 else 0.0


def run(img: Image.Image, registry) -> DetOutcome:
    sess = registry.det
    if sess is None:
        return DetOutcome(ready=False, error="检测模型缺失")

    tensor, scale, pad_x, pad_y = preprocess.det_tensor(img)
    out = registry.run("det", tensor)[0][0]        # (84, 8400)

    def decode(cls_ids: tuple) -> list[Box]:
        boxes: list[Box] = []
        for cls_id in cls_ids:
            scores = out[BOX_HEAD + cls_id]
            for i in range(scores.shape[0]):
                conf = float(scores[i])
                if conf < config.PERSON_CONF_MIN:
                    continue
                cx, cy = float(out[0, i]), float(out[1, i])
                bw, bh = float(out[2, i]), float(out[3, i])
                # 锚点坐标 → letterbox 坐标 → 原图坐标
                x1 = ((cx - bw / 2) - pad_x) / scale
                y1 = ((cy - bh / 2) - pad_y) / scale
                x2 = ((cx + bw / 2) - pad_x) / scale
                y2 = ((cy + bh / 2) - pad_y) / scale
                boxes.append(Box(x1, y1, x2, y2, conf))
        return _nms(boxes, config.NMS_IOU)

    persons = decode((PERSON_CLASS_ID,))
    vehicles = decode(VEHICLE_CLASS_IDS)

    # 说明：人框与车辆框重叠降级方案实测会误伤「骑电动车的人」（e--7 骑手框与车
    # 重叠被判为误检），且对车流误检（e-7278 假框与车不重叠）无效，故弃用；
    # 改用仲裁器的「密集车流 + 全小框 → 跳过 street」规则（config.VEHICLE_HEAVY_N）。

    w, h = img.size
    area_ratio = max((((bk.x2 - bk.x1) * (bk.y2 - bk.y1)) / (w * h)) for bk in persons) if persons else 0.0
    max_conf = max((b.conf for b in persons), default=0.0)
    return DetOutcome(
        persons=persons,
        count=len(persons),
        max_conf=max_conf,
        max_area_ratio=area_ratio,
        vehicles=vehicles,
        vehicle_count=len(vehicles),
        ready=True,
    )
