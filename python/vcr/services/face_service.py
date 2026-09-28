"""服务层：人脸标号通道（P2）

流程：全图 SCRFD 检测人脸 → 5 点对齐 → ArcFace(w600k) 512 维嵌入 →
     持久层余弦匹配（≥FACE_SIM 同人）→ 返回 person_id 列表。

模型缺失时静默降级（person_ids 为空，不影响主流程）。
SCRFD 输出为 stride 8/16/32 三档，每格 2 锚点，需 distance2bbox 解码。
"""
from dataclasses import dataclass

import cv2
import numpy as np
from PIL import Image

from .. import config, preprocess, timing
from ..persistence.person_store import get_store

STRIDES = [8, 16, 32]


# ---------------------------------------------------------------------------
# SCRFD 解码（向量化）
#
# 为什么必须向量化（实测数据在下面，勿凭直觉改回逐锚点循环）：
#   旧实现是双层 Python 循环（for cell → for anchor，共 80×80×2 + 40×40×2 +
#   20×20×2 = 16800 次），每次都 numpy 标量取值 + float() + list.append。
#   实测（test 53 取样，串行）：单张**有脸图** face.post = 266ms，是 det.run
#   的约 9 倍；而且这段全程持有 GIL —— 照片级并行 P=6 时它一个人就把别的
#   线程挡在门外（同批 ocr.post 被抬到 40ms 是同一原因：真实计算只要 0.9ms）。
#   改为 numpy 掩码 + 批量反算 bbox/kps 后：0.60ms/张，**442x**，
#   输出与旧实现逐字段全等（bbox/score/kps，见 python/bench/verify_postprocess_equiv.py）。
# ---------------------------------------------------------------------------
def _scrfd_candidates(stride: int, sc: np.ndarray, bx: np.ndarray, kps: np.ndarray):
    """单个 stride 的 SCRFD 输出 → 摊平候选项（尚未乘 stride）。

    返回 (scores, boxes, kpsf, cell, w)：
      scores (N,) / boxes (N,4) 归一化距离 / kpsf (N,10) 归一化关键点 /
      cell (N,) 网格序号 / w 概率图宽度

    摊平顺序必须是「cell 主序、anchor 次序」—— 与旧实现的双层循环
    （for cell: for a:）逐项一致。顺序会影响分数并列时 NMS 的取舍，
    不能为了写法方便改成 anchor 主序（那会改变并列时的去重结果）。

    两种导出格式都兼容：
      4D 通道式 (1,C,H,W)：C=2 分 / 8 框 / 20 关键点（det_10g 风格）
      2D 扁平式 (N,D)：D=1 分 / 4 框 / 10 关键点（det_500m 风格）
    """
    if sc.ndim == 4:
        num_anc, h, w = sc.shape[1], sc.shape[2], sc.shape[3]
        # (A,H,W) → (H,W,A) → (H*W*A,)：把 anchor 挪到最后一维即得 cell 主序
        scores = sc[0].transpose(1, 2, 0).reshape(-1)
        boxes = bx[0].reshape(num_anc, 4, h, w).transpose(2, 3, 0, 1).reshape(-1, 4)
        kpsf = kps[0].reshape(num_anc, 10, h, w).transpose(2, 3, 0, 1).reshape(-1, 10)
        cell = np.repeat(np.arange(h * w), num_anc)
    else:
        n = sc.shape[0]
        num_anc = 2
        cells = n // num_anc
        h = w = int(round(cells ** 0.5))
        scores = sc[:, 0]
        boxes = bx
        kpsf = kps
        cell = np.arange(n) // num_anc
    return scores, boxes, kpsf, cell, w


@dataclass
class Face:
    bbox: tuple[int, int, int, int]   # (x1,y1,x2,y2) 原图
    kps: np.ndarray                   # (5,2) 原图
    score: float


class FaceService:
    def __init__(self, registry, store=None):
        self.registry = registry
        self.store = store or get_store()

    # ------------------------------------------------------------------
    def ready(self) -> bool:
        # 属性访问触发惰性加载，避免新进程误判通道不可用
        self.registry.face_det
        self.registry.face_rec
        return self.registry.is_ready("face_det") and self.registry.is_ready("face_rec")

    # ------------------------------------------------------------------
    @staticmethod
    def _decode_scrfd(outputs: list[np.ndarray],
                      scale: float, pad_x: float, pad_y: float) -> list[Face]:
        """解码 SCRFD 输出 → 原图坐标人脸列表（向量化，见 _scrfd_candidates）。

        输出顺序统一为 score×3 → bbox×3 → kps×3（stride 8/16/32），
        但按形状自适应分组，不依赖模型输出命名。
        """
        # 按形状分组：score(C=2 或 D=1) / bbox(C=8 或 D=4) / kps(C=20 或 D=10)
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
            # 锚点数降序 = stride 升序（stride 8 锚点最多）
            groups[k].sort(key=lambda o: o.shape[0] * o.shape[-2] if o.ndim == 4 else o.shape[0],
                           reverse=True)

        score_parts: list[np.ndarray] = []
        box_parts: list[np.ndarray] = []
        kps_parts: list[np.ndarray] = []
        for stride, sc, bx, kps in zip(STRIDES, groups["score"], groups["bbox"], groups["kps"]):
            scores, boxes, kpsf, cell, w = _scrfd_candidates(stride, sc, bx, kps)
            # 阈值：旧实现是 `score = float(sc[..])` 后 `if score < thresh: continue` ——
            # 即 float64 比较。这里必须先把 float32 分数提升到 float64 再比：
            # NEP50 下 float32 数组与 Python float 比较是「弱提升」，会把阈值降到
            # float32(0.35)=0.3499999940395355，于是「恰好等于 float32 阈值」的候选
            # 在新实现里变成通过、旧实现里被丢弃（实测差 3 个候选）。
            # 用 ~(s < t) 而不是 s >= t，同样是为了让 NaN 的路径与旧实现一致。
            idx = np.flatnonzero(~(scores.astype(np.float64) < config.PERSON_CONF_MIN))
            if idx.size == 0:
                continue
            stride_f = np.float32(stride)
            # 网格中心（旧实现是 Python int：x*stride / y*stride）
            cx = ((cell[idx] % w) * stride).astype(np.float32)
            cy = ((cell[idx] // w) * stride).astype(np.float32)
            # 距离乘 stride（SCRFD 预测归一化距离，见 insightface scrfd.py: bbox*stride）
            d = boxes[idx].astype(np.float32) * stride_f
            pts = kpsf[idx].reshape(-1, 5, 2).astype(np.float32) * stride_f
            pts = pts + np.stack([cx, cy], axis=1)[:, None, :]
            # letterbox → 原图。bbox 取整成像素（int(round)），kps 保持浮点。
            score_parts.append(scores[idx])
            box_parts.append(np.stack([
                (cx - d[:, 0] - pad_x) / scale,
                (cy - d[:, 1] - pad_y) / scale,
                (cx + d[:, 2] - pad_x) / scale,
                (cy + d[:, 3] - pad_y) / scale,
            ], axis=1))
            # kps 的 pad 刻意用 int 数组：旧实现 (pts - np.array([pad_x, pad_y])) / scale
            # 走的是 float32 - int64 → float64，保持同一路径才能逐位一致
            # （实测：写 dtype=float32 会与旧值差 1e-4）。
            kps_parts.append((pts - np.array([pad_x, pad_y])) / scale)

        if not score_parts:
            return []
        scores = np.concatenate(score_parts)
        boxes = np.concatenate(box_parts)
        kpss = np.concatenate(kps_parts)
        rounded = np.round(boxes).astype(np.int64)
        return [
            Face(
                bbox=(int(rounded[i, 0]), int(rounded[i, 1]),
                      int(rounded[i, 2]), int(rounded[i, 3])),
                kps=kpss[i],
                score=float(scores[i]),
            )
            for i in range(scores.shape[0])
        ]

    # ------------------------------------------------------------------
    def detect_faces(self, img: Image.Image) -> list[Face]:
        if not self.ready():
            return []
        # 细粒度记账：face 是单张最大头（实测 42.9%），必须能区分
        # 「检测前向」与「Python 解码后处理（逐锚点循环）」谁更贵 ——
        # 后者是纯 Python 循环，最容易被低估。
        with timing.span("face.pre"):
            tensor, scale, pad_x, pad_y = preprocess.face_det_tensor(img)
        with timing.span("face.det"):
            outputs = self.registry.run("face_det", tensor)
        with timing.span("face.post"):
            faces = self._decode_scrfd(outputs, scale, pad_x, pad_y)
            # 过滤过小人脸 + 越界
            w, h = img.size
            kept = []
            for f in faces:
                bw, bh = f.bbox[2] - f.bbox[0], f.bbox[3] - f.bbox[1]
                if min(bw, bh) < config.FACE_MIN_PIX:
                    continue
                if f.bbox[0] < 0 or f.bbox[1] < 0 or f.bbox[2] > w or f.bbox[3] > h:
                    continue
                kept.append(f)
            kept.sort(key=lambda f: f.score, reverse=True)
            # NMS 去重（SCRFD 同脸多框），IoU 阈值与检测一致
            kept = self._nms_faces(kept)
        return kept[:16]          # 单图最多标号 16 张脸

    @staticmethod
    def _nms_faces(faces: list[Face]) -> list[Face]:
        """对人脸框做 IoU NMS 去重（SCRFD 同脸多框）。

        阈值沿用旧实现的字面量 0.45（与 config.NMS_IOU 当前取值相同，但这里
        刻意保留字面量以维持逐位等价）。NMS 的向量化实现在 detector.nms_indices。
        """
        if len(faces) <= 1:
            return list(faces)
        from .detector import nms_indices

        coords = np.array([f.bbox for f in faces], dtype=np.float64)
        scores = np.array([f.score for f in faces], dtype=np.float64)
        return [faces[int(i)] for i in nms_indices(coords, scores, 0.45)]

    # ------------------------------------------------------------------
    def embed(self, img: Image.Image, face: Face) -> np.ndarray | None:
        sess = self.registry.face_rec
        if sess is None:
            return None
        try:
            with timing.span("face.align"):
                tensor = preprocess.face_align(img, face.kps)
            with timing.span("face.rec"):
                out = self.registry.run("face_rec", tensor)[0][0]
            emb = np.asarray(out, dtype=np.float32)
            n = np.linalg.norm(emb)
            return emb / n if n > 0 else None
        except Exception:
            return None

    # ------------------------------------------------------------------
    def process_photo(self, img: Image.Image, photo_path: str) -> list[dict]:
        """返回 [{person_id, bbox, sim}]，空列表 = 无可用人脸。"""
        if not self.ready():
            return []
        faces = self.detect_faces(img)
        hits: list[dict] = []
        for f in faces:
            emb = self.embed(img, f)
            if emb is None:
                continue
            pid, sim = self.store.register(emb, photo_path, f"{f.bbox}")
            hits.append({"person_id": pid, "bbox": f.bbox, "sim": round(sim, 3)})
        return hits


_face_service: FaceService | None = None


def get_face_service(registry) -> FaceService:
    global _face_service
    if _face_service is None:
        _face_service = FaceService(registry)
    return _face_service
