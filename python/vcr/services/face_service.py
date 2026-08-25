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

from .. import config, preprocess
from ..persistence.person_store import get_store

STRIDES = [8, 16, 32]


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
    def _decode_scrfd(self, outputs: list[np.ndarray], img_w: int, img_h: int,
                      scale: float, pad_x: float, pad_y: float) -> list[Face]:
        """解码 SCRFD 输出 → 原图坐标人脸列表。

        兼容两种导出格式：
          4D 通道式 (1,C,H,W)：C=2 分 / 8 框 / 20 关键点（det_10g 风格）
          2D 扁平式 (N,D)：D=1 分 / 4 框 / 10 关键点（det_500m 风格）
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

        faces: list[Face] = []
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
                    # 距离乘 stride（SCRFD 预测的是归一化距离，见 insightface scrfd.py: bbox*stride）
                    d = d * stride
                    x1, y1 = cx - d[0], cy - d[1]
                    x2, y2 = cx + d[2], cy + d[3]
                    # letterbox → 原图
                    faces.append(Face(
                        bbox=(int(round((x1 - pad_x) / scale)), int(round((y1 - pad_y) / scale)),
                              int(round((x2 - pad_x) / scale)), int(round((y2 - pad_y) / scale))),
                        kps=(pts - np.array([pad_x, pad_y])) / scale,
                        score=score,
                    ))
        return faces

    # ------------------------------------------------------------------
    def detect_faces(self, img: Image.Image) -> list[Face]:
        if not self.ready():
            return []
        tensor, scale, pad_x, pad_y = preprocess.face_det_tensor(img)
        outputs = self.registry.run("face_det", tensor)
        faces = self._decode_scrfd(outputs, img.size[0], img.size[1], scale, pad_x, pad_y)
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
        """对人脸框做 IoU NMS 去重（SCRFD 同脸多框）。"""
        from .detector import Box, _iou

        kept: list[Face] = []
        candidates = sorted(faces, key=lambda f: f.score, reverse=True)
        while candidates:
            best = candidates.pop(0)
            kept.append(best)
            bbox = best.bbox
            candidates = [
                f for f in candidates
                if _iou(
                    Box(float(bbox[0]), float(bbox[1]), float(bbox[2]), float(bbox[3]), best.score),
                    Box(float(f.bbox[0]), float(f.bbox[1]), float(f.bbox[2]), float(f.bbox[3]), f.score),
                ) <= 0.45
            ]
        return kept

    # ------------------------------------------------------------------
    def embed(self, img: Image.Image, face: Face) -> np.ndarray | None:
        sess = self.registry.face_rec
        if sess is None:
            return None
        try:
            tensor = preprocess.face_align(img, face.kps)
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
