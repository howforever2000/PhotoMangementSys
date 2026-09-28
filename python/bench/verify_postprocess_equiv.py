"""等价性校验（点 2）：向量化后处理 vs 旧逐锚点实现，逐字段一致。

为什么需要这个脚本
------------------
点 2 把 SCRFD 解码+NMS、YOLOv8 解码+NMS 从「Python 逐锚点循环」改成 numpy
向量化。这类改动「读起来一样」，但极易在三个地方悄悄改掉输出：

  1. **顺序**：旧实现是 `for cell: for anchor:`，若 vectorize 时把 anchor 挪到
     前面，分数并列时 NMS 的取舍就会变（NMS 是贪心，顺序即结果）。
  2. **dtype 提升**：旧实现用 Python float（float64）做算术；直接改成 float32
     会让 bbox 的 `int(round())` 与 kps 在 1e-4 量级上漂移（实测踩到过）。
  3. **阈值边界**：旧实现写的是 `if score < thresh: continue`，所以 NaN 分数会
     **被保留**；写成 `scores >= thresh` 就会把 NaN 丢掉。

所以把旧实现原样保留为本文件的**参照实现**（reference），用可重现的合成数据
在每个分支上逐字段比对。它是回归防线，不是死代码。

用法：python python/bench/verify_postprocess_equiv.py
"""
import os
import sys
import tempfile

# 必须先于 vcr 导入：把数据目录隔离到临时目录，绝不碰生产 persons.db
os.environ["VCR_DATA_DIR"] = os.path.join(tempfile.gettempdir(), "vcr_equiv_check")
os.makedirs(os.environ["VCR_DATA_DIR"], exist_ok=True)

import numpy as np  # noqa: E402
from pathlib import Path  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from vcr import config  # noqa: E402
from vcr.services.detector import Box, _decode, nms_indices  # noqa: E402
from vcr.services.face_service import Face, FaceService  # noqa: E402

STRIDES = [8, 16, 32]


# ===========================================================================
# 参照实现（旧代码，原样保留）—— 不要「顺手优化」这里，它就是判据
# ===========================================================================
def ref_decode_scrfd(outputs, scale, pad_x, pad_y):
    """旧版 SCRFD 解码（逐 cell × 逐 anchor 的 Python 双层循环）。"""

    def kind(o):
        if o.ndim == 4:
            return {2: "score", 8: "bbox", 20: "kps"}.get(o.shape[1], "?")
        return {1: "score", 4: "bbox", 10: "kps"}.get(o.shape[1], "?")

    groups = {"score": [], "bbox": [], "kps": []}
    for o in outputs:
        k = kind(o)
        if k != "?":
            groups[k].append(o)
    for k in groups:
        groups[k].sort(key=lambda o: o.shape[0] * o.shape[-2] if o.ndim == 4 else o.shape[0],
                       reverse=True)

    faces = []
    for stride, sc, bx, kps in zip(STRIDES, groups["score"], groups["bbox"], groups["kps"]):
        if sc.ndim == 4:
            num_anc, h, w = sc.shape[1], sc.shape[2], sc.shape[3]
        else:
            n = sc.shape[0]
            num_anc = 2
            cells = n // 2
            h = w = int(round(cells ** 0.5))
        for cell in range(h * w):
            x = cell % w
            y = cell // w
            cx, cy = x * stride, y * stride
            for a in range(num_anc):
                if sc.ndim == 4:
                    score = float(sc[0, a, y, x])
                    d = bx[0, a * 4:(a + 1) * 4, y, x]
                    pts = np.stack([
                        [cx + kps[0, a * 10 + i * 2, y, x] * stride,
                         cy + kps[0, a * 10 + i * 2 + 1, y, x] * stride]
                        for i in range(5)
                    ])
                else:
                    idx = cell * num_anc + a
                    score = float(sc[idx, 0])
                    d = bx[idx, :]
                    pts = np.stack([
                        [cx + kps[idx, i * 2] * stride, cy + kps[idx, i * 2 + 1] * stride]
                        for i in range(5)
                    ])
                if score < config.PERSON_CONF_MIN:
                    continue
                d = d * stride
                x1, y1 = cx - d[0], cy - d[1]
                x2, y2 = cx + d[2], cy + d[3]
                faces.append(Face(
                    bbox=(int(round((x1 - pad_x) / scale)), int(round((y1 - pad_y) / scale)),
                          int(round((x2 - pad_x) / scale)), int(round((y2 - pad_y) / scale))),
                    kps=(pts - np.array([pad_x, pad_y])) / scale,
                    score=score,
                ))
    return faces


def ref_decode_boxes(out, cls_ids, scale, pad_x, pad_y):
    """旧版 YOLOv8 解码（逐 cls_id × 逐锚点），收集完所有类后调一次 NMS。"""
    boxes = []
    for cls_id in cls_ids:
        scores = out[4 + cls_id]
        for i in range(scores.shape[0]):
            conf = float(scores[i])
            if conf < config.PERSON_CONF_MIN:
                continue
            cx, cy = float(out[0, i]), float(out[1, i])
            bw, bh = float(out[2, i]), float(out[3, i])
            boxes.append(Box(((cx - bw / 2) - pad_x) / scale,
                             ((cy - bh / 2) - pad_y) / scale,
                             ((cx + bw / 2) - pad_x) / scale,
                             ((cy + bh / 2) - pad_y) / scale, conf))
    return ref_nms(boxes, config.NMS_IOU)


def ref_nms(boxes, iou_thr):
    """旧版 NMS（Python 层逐对比较）。"""
    if not boxes:
        return []
    boxes = sorted(boxes, key=lambda b: b.conf, reverse=True)
    keep = []
    while boxes:
        best = boxes.pop(0)
        keep.append(best)
        boxes = [b for b in boxes if ref_iou(best, b) <= iou_thr]
    return keep


def ref_iou(a, b):
    x1, y1 = max(a.x1, b.x1), max(a.y1, b.y1)
    x2, y2 = min(a.x2, b.x2), min(a.y2, b.y2)
    inter = max(0.0, x2 - x1) * max(0.0, y2 - y1)
    union = (a.x2 - a.x1) * (a.y2 - a.y1) + (b.x2 - b.x1) * (b.y2 - b.y1) - inter
    return inter / union if union > 0 else 0.0


# ===========================================================================
# 合成数据
# ===========================================================================
def synth_scrfd(rng, layout, thresh_around=False):
    """造一组 SCRFD 输出。layout='flat'（det_500m）或 '4d'（det_10g）。"""
    outs = []
    for stride in STRIDES:
        hw = 640 // stride
        n = hw * hw * 2
        if layout == "4d":
            sc = rng.random((1, 2, hw, hw)).astype(np.float32)
            bx = (rng.random((1, 8, hw, hw)) * 0.05).astype(np.float32)
            kp = (rng.random((1, 20, hw, hw)) * 0.1).astype(np.float32)
        else:
            sc = rng.random((n, 1)).astype(np.float32)
            bx = (rng.random((n, 4)) * 0.05).astype(np.float32)
            kp = (rng.random((n, 10)) * 0.1).astype(np.float32)
        # 让一部分分数越过阈值（否则等于没测）
        sc = np.where(sc > 0.75, sc + 0.24, sc).astype(np.float32)
        if thresh_around:
            # 精确等于阈值 + NaN：考 `score < thresh` 的边界语义
            flat = sc.reshape(-1)
            flat[0] = config.PERSON_CONF_MIN
            flat[1] = np.nan
        outs.extend([sc, bx, kp])
    return outs


def synth_yolo(rng, n=8400, cls_ids=(0, 2, 3, 5, 7)):
    out = rng.random((84, n)).astype(np.float32) * 800
    # 前 4 行是 cx,cy,w,h（给合理尺度），其余是类别分数
    out[2:4] = rng.random((2, n)).astype(np.float32) * 400 + 20
    for c in cls_ids:
        out[4 + c] = np.where(rng.random(n) > 0.8, 0.9, 0.01).astype(np.float32)
    return out


# ===========================================================================
# 比对
# ===========================================================================
def same_float(a, b) -> bool:
    """浮点相等判定；NaN 视为相等（旧实现会保留 NaN 分数，两侧都应是 NaN）。"""
    return bool(a == b) or (np.isnan(a) and np.isnan(b))


def check_scrfd(layout, thresh_around=False):
    rng = np.random.default_rng(20260928)
    scale, pad_x, pad_y = 0.15625, 12, 34
    ok = 0
    for trial in range(3):
        outs = synth_scrfd(rng, layout, thresh_around)
        old = ref_decode_scrfd(outs, scale, pad_x, pad_y)
        new = FaceService._decode_scrfd(outs, scale, pad_x, pad_y)
        assert len(old) == len(new), f"{layout} #{trial}: 数量 {len(old)} vs {len(new)}"
        for i, (a, b) in enumerate(zip(old, new)):
            assert a.bbox == b.bbox, f"{layout} #{trial} [{i}] bbox {a.bbox} vs {b.bbox}"
            assert same_float(a.score, b.score), \
                f"{layout} #{trial} [{i}] score {a.score} vs {b.score}"
            assert a.kps.dtype == b.kps.dtype, f"{layout} #{trial} [{i}] kps dtype"
            assert np.array_equal(a.kps, b.kps), f"{layout} #{trial} [{i}] kps 不一致"
        ok += len(old)
    return ok


def check_yolo():
    rng = np.random.default_rng(7)
    scale, pad_x, pad_y = 0.15625, 12, 34
    total = 0
    for cls_ids in [(0,), (2, 3, 5, 7), (0, 2, 3, 5, 7)]:
        out = synth_yolo(rng, cls_ids=cls_ids)
        old = ref_decode_boxes(out, cls_ids, scale, pad_x, pad_y)
        new = _decode(out, cls_ids, scale, pad_x, pad_y)
        assert len(old) == len(new), f"yolo {cls_ids}: {len(old)} vs {len(new)}"
        for a, b in zip(old, new):
            assert (a.x1, a.y1, a.x2, a.y2, a.conf) == (b.x1, b.y1, b.x2, b.y2, b.conf), \
                f"yolo {cls_ids}: {a} vs {b}"
        total += len(old)
    return total


def check_nms_edge_cases():
    """空输入 / 单元素 / 全重叠 / 分数并列。"""
    assert nms_indices(np.zeros((0, 4)), np.zeros(0), 0.45).size == 0
    assert list(nms_indices(np.array([[0, 0, 10, 10]], float), np.array([0.9]), 0.45)) == [0]
    same = np.array([[0, 0, 10, 10], [0, 0, 10, 10], [0, 0, 10, 10]], float)
    assert list(nms_indices(same, np.array([0.9, 0.8, 0.7]), 0.45)) == [0], "完全重叠只留最高分"
    # 分数并列 → 稳定排序应保留输入顺序（旧实现 sorted(reverse=True) 是稳定的）
    tie = np.array([[0, 0, 10, 10], [0, 0, 10, 10]], float)
    assert list(nms_indices(tie, np.array([0.5, 0.5]), 0.45)) == [0]
    # 互不重叠 → 全留
    apart = np.array([[0, 0, 10, 10], [50, 50, 60, 60]], float)
    assert sorted(nms_indices(apart, np.array([0.5, 0.6]), 0.45).tolist()) == [0, 1]
    # 旧参照实现与新的语义一致
    boxes = [Box(0, 0, 10, 10, 0.9), Box(0, 0, 10, 10, 0.8), Box(50, 50, 60, 60, 0.7)]
    ref_ids = [boxes.index(b) for b in ref_nms(list(boxes), 0.45)]
    new_ids = nms_indices(np.array([[b.x1, b.y1, b.x2, b.y2] for b in boxes], float),
                          np.array([b.conf for b in boxes], float), 0.45)
    assert ref_ids == list(new_ids), f"NMS 取舍不一致 {ref_ids} vs {list(new_ids)}"


def main():
    print("== SCRFD 解码（2D 扁平式 / det_500m） ==")
    n1 = check_scrfd("flat")
    print(f"   通过：{n1} 个候选逐项一致（bbox/score/kps/dtype）")
    print("== SCRFD 解码（4D 通道式 / det_10g） ==")
    n2 = check_scrfd("4d")
    print(f"   通过：{n2} 个候选逐项一致")
    print("== SCRFD 阈值边界（恰好等于阈值 / NaN 保留） ==")
    n3 = check_scrfd("flat", thresh_around=True)
    n4 = check_scrfd("4d", thresh_around=True)
    print(f"   通过：{n3 + n4} 个候选逐项一致")
    print("== YOLOv8 解码（多类并集一次 NMS） ==")
    n5 = check_yolo()
    print(f"   通过：{n5} 个框逐项一致")
    print("== NMS 边界（空 / 单个 / 全重叠 / 并列 / 分离） ==")
    check_nms_edge_cases()
    print("   通过")
    print("\n全部等价性检查通过 ✅（向量化未改变任何输出字段）")


if __name__ == "__main__":
    main()
