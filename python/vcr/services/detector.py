"""服务层：目标检测通道（YOLOv8n-det, COCO 80 类）

职责：
  1. 全图推理，NMS 聚合 person 框（旧实现用锚点最大值，1 个人有 9~10 个锚点
     导致人数不可统计——P1 修复）
  2. 产出 person 统计：NMS 后人数 / 最大人框面积占比 / 最大置信度，供仲裁器
  3. 输出原始人框坐标，供人脸标号通道裁剪
"""
from dataclasses import dataclass, field

import numpy as np

from .. import config, preprocess, timing

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


def nms_indices(coords: np.ndarray, scores: np.ndarray, iou_thr: float) -> np.ndarray:
    """贪心 IoU NMS（向量化）：返回保留下来的下标，按置信度降序。

    coords (N,4) = x1,y1,x2,y2；scores (N,)。

    为什么用 float64：旧实现是 Python float（float64）算 IoU 的，降成 float32
    会在阈值附近产生非等价取舍（实测 kps 用 float32 就会与旧值差 1e-4）。
    并列分数用**稳定**排序保持输入顺序 —— 与旧实现 Python 的
    `sorted(..., reverse=True)`（稳定）逐项一致。

    为什么仍是 while 循环：NMS 本身是串行贪心（后一个的取舍依赖前一个的结果），
    不能整体向量化；但循环体一次处理「当前框 vs 剩余全部框」，N 个候选只需
    N 次 numpy 调用而不是 O(N²) 次 Python 层比较。
    """
    n = coords.shape[0]
    if n == 0:
        return np.empty(0, dtype=np.int64)
    c = coords.astype(np.float64, copy=False)
    order = np.argsort(-scores.astype(np.float64, copy=False), kind="stable")
    keep: list[int] = []
    while order.size:
        i = int(order[0])
        keep.append(i)
        if order.size == 1:
            break
        rest = order[1:]
        x1 = np.maximum(c[i, 0], c[rest, 0])
        y1 = np.maximum(c[i, 1], c[rest, 1])
        x2 = np.minimum(c[i, 2], c[rest, 2])
        y2 = np.minimum(c[i, 3], c[rest, 3])
        inter = np.maximum(0.0, x2 - x1) * np.maximum(0.0, y2 - y1)
        area_i = (c[i, 2] - c[i, 0]) * (c[i, 3] - c[i, 1])
        area_r = (c[rest, 2] - c[rest, 0]) * (c[rest, 3] - c[rest, 1])
        union = area_i + area_r - inter
        iou = np.where(union > 0, inter / np.where(union > 0, union, 1.0), 0.0)
        order = rest[iou <= iou_thr]
    return np.asarray(keep, dtype=np.int64)


def _decode(out: np.ndarray, cls_ids: tuple[int, ...],
            scale: float, pad_x: int, pad_y: int) -> list[Box]:
    """YOLOv8 原始输出 (84, N) → 某几类的框（过滤 + 反 letterbox + NMS）。

    向量化：旧实现对每个 cls_id 扫 8400 个锚点（person + 4 个车类共 42000 次
    Python 迭代），每次都要 float() 取值、拼 Box。现在一次掩码取全部命中项。

    等价性要点（别改）：
      - 阈值用 `~(s < thresh)` 而不是 `s >= thresh` —— 旧实现是
        `if conf < thresh: continue`，NaN 会被保留；
      - NMS 仍对**cls_ids 的并集**做一次（旧实现的 decode 是在收集完所有
        类别后才调一次 _nms），不是逐类各做一次；
      - 候选项顺序保持「类别主序、锚点次序」，NMS 并列分数时取舍与旧实现一致。
    """
    coord_parts: list[np.ndarray] = []
    conf_parts: list[np.ndarray] = []
    for cls_id in cls_ids:
        scores = out[BOX_HEAD + cls_id]
        # 阈值：旧实现是 `conf = float(scores[i])` 后 `if conf < thresh: continue`，
        # 即 float64 比较。NEP50 下 float32 数组与 Python float 比是弱提升，会把
        # 阈值降到 float32(0.35)，让「恰好等于 float32 阈值」的框多活一个
        # （与 face 侧同一坑）。用 ~(s < t) 保留 NaN 的旧语义。
        idx = np.flatnonzero(~(scores.astype(np.float64) < config.PERSON_CONF_MIN))
        if idx.size == 0:
            continue
        # float64：旧实现用 float(out[..]) 做 Python 浮点运算，降精度会改变取值
        cx = out[0, idx].astype(np.float64)
        cy = out[1, idx].astype(np.float64)
        bw = out[2, idx].astype(np.float64)
        bh = out[3, idx].astype(np.float64)
        coord_parts.append(np.stack([
            ((cx - bw / 2) - pad_x) / scale,
            ((cy - bh / 2) - pad_y) / scale,
            ((cx + bw / 2) - pad_x) / scale,
            ((cy + bh / 2) - pad_y) / scale,
        ], axis=1))
        conf_parts.append(scores[idx].astype(np.float64))

    if not coord_parts:
        return []
    coords = np.concatenate(coord_parts)
    confs = np.concatenate(conf_parts)
    keep = nms_indices(coords, confs, config.NMS_IOU)
    return [
        Box(float(coords[i, 0]), float(coords[i, 1]),
            float(coords[i, 2]), float(coords[i, 3]), float(confs[i]))
        for i in keep
    ]


def run(lb: preprocess.Letterbox, registry) -> DetOutcome:
    sess = registry.det
    if sess is None:
        return DetOutcome(ready=False, error="检测模型缺失")

    # 细粒度记账：预处理 / ONNX 前向 / 后处理（NMS+统计）三分。
    # det 是每张图必经的通道，这三项的比例决定了「该优化预处理还是换模型」。
    # 注意：解码与 4096→640 的缩放已上提到 pipeline 的 letterbox() 一次做完，
    # 这里的 det.pre 只剩「贴灰底 + 归一化」（亚毫秒级）。
    with timing.span("det.pre"):
        tensor, scale, pad_x, pad_y = preprocess.det_tensor(lb)
    with timing.span("det.run"):
        out = registry.run("det", tensor)[0][0]    # (84, 8400)

    with timing.span("det.post"):
        persons = _decode(out, (PERSON_CLASS_ID,), scale, pad_x, pad_y)
        vehicles = _decode(out, VEHICLE_CLASS_IDS, scale, pad_x, pad_y)

    # 说明：人框与车辆框重叠降级方案实测会误伤「骑电动车的人」（e--7 骑手框与车
    # 重叠被判为误检），且对车流误检（e-7278 假框与车不重叠）无效，故弃用；
    # 改用仲裁器的「密集车流 + 全小框 → 跳过 street」规则（config.VEHICLE_HEAVY_N）。

    w, h = lb.size
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
