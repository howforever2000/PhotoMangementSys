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

# 质量判定的三档结果
QUALITY_OK = "ok"        # 正常：可建新人物、可更新质心
QUALITY_MARGINAL = "marginal"  # 边缘：只准并入已有簇，不准新建、不更新质心
QUALITY_REJECT = "reject"      # 硬丢：不参与嵌入


def face_geometry_quality(bbox: tuple[int, int, int, int], kps: np.ndarray) -> str:
    """仅用检测输出的框 + 5 点关键点判质量（不做图像运算，免费）。

    为什么先看几何：实测垃圾框（低头/后脑勺/手挡脸/风扇）与真人脸的差别主要在
    **关键点不成人脸形状**——眼距占框宽中位 0.07 vs 0.46、鼻-眼纵向比 4.96 vs 0.57。
    判别项与阈值均来自实测分布（见 config.FACE_QUALITY_* 注释）。
    """
    x1, y1, x2, y2 = bbox
    bw, bh = max(x2 - x1, 1), max(y2 - y1, 1)
    aspect = bw / bh
    lo, hi = config.FACE_QUALITY_ASPECT
    if not (lo <= aspect <= hi):
        return QUALITY_REJECT
    side = min(bw, bh)
    if side < config.FACE_MIN_PIX:
        return QUALITY_REJECT

    le, re_, nose, mouth_l, mouth_r = kps
    eye_d = float(np.linalg.norm(re_ - le))
    if eye_d <= 1e-6:
        return QUALITY_REJECT
    eye_span = eye_d / bw
    lo, hi = config.FACE_QUALITY_EYE_SPAN
    if not (lo <= eye_span <= hi):
        return QUALITY_REJECT

    yaw = abs(float(nose[0]) - (float(le[0]) + float(re_[0])) / 2.0) / eye_d
    roll = abs(
        float(np.degrees(np.arctan2(float(re_[1]) - float(le[1]), float(re_[0]) - float(le[0]))))
    )
    if yaw > config.FACE_QUALITY_YAW or roll > config.FACE_QUALITY_ROLL_DEG:
        return QUALITY_REJECT

    eye_mid_y = (float(le[1]) + float(re_[1])) / 2.0
    nose_dy = (float(nose[1]) - eye_mid_y) / eye_d
    lo, hi = config.FACE_QUALITY_NOSE_DY
    if not (lo <= nose_dy <= hi):
        return QUALITY_REJECT
    mouth_dy = ((float(mouth_l[1]) + float(mouth_r[1])) / 2.0 - float(nose[1])) / eye_d
    lo, hi = config.FACE_QUALITY_MOUTH_DY
    if not (lo <= mouth_dy <= hi):
        return QUALITY_REJECT

    # 边缘（仍可用，但不该当“新人脸基准”）：侧脸/转过头/小脸/轻模糊
    if (
        yaw > config.FACE_MARGINAL_YAW
        or roll > config.FACE_MARGINAL_ROLL_DEG
        or side < config.FACE_MARGINAL_MIN_PIX
    ):
        return QUALITY_MARGINAL
    return QUALITY_OK


def aligned_blur(tensor: np.ndarray) -> float:
    """对齐后 112×112 张量的清晰度（Laplacian 方差）。

    tensor 是 face_align 输出 (1,3,112,112) float32（已是 (x-127.5)/128 量纲）
    → 回到 0~255 后再算方差，与 config.FACE_QUALITY_BLUR_MIN 同口径。
    """
    arr = tensor[0].transpose(1, 2, 0) * 128.0 + 127.5
    gray = cv2.cvtColor(np.clip(arr, 0, 255).astype(np.uint8), cv2.COLOR_RGB2GRAY)
    return float(cv2.Laplacian(gray, cv2.CV_64F).var())


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
            # 阈值用 FACE_DET_CONF（2026-10-01 起独立于 YOLO 的 PERSON_CONF_MIN）：
            # 人脸框误检（雕像/花纹/屏幕）分数多在 0.35~0.5，真脸普遍 >0.7。
            idx = np.flatnonzero(~(scores.astype(np.float64) < config.FACE_DET_CONF))
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
    def detect_faces(self, lb: preprocess.Letterbox) -> list[Face]:
        if not self.ready():
            return []        # 细粒度记账：face 是单张最大头，必须能区分「检测前向」与「后处理」。
        # face.pre 只剩贴黑底 + 归一化（解码与缩放已上提到 pipeline 的 letterbox()）。
        with timing.span("face.pre"):
            tensor, scale, pad_x, pad_y = preprocess.face_det_tensor(lb)
        with timing.span("face.det"):
            outputs = self.registry.run("face_det", tensor)
        with timing.span("face.post"):
            faces = self._decode_scrfd(outputs, scale, pad_x, pad_y)
            # 过滤过小人脸 + 越界（用**原图**尺寸：解码的 scale 也是从原图算的）。
            # FACE_MIN_PIX 24→32（2026-10-01）：24px 脸喂 ArcFace 产出噪声向量，
            # 是聚类错挂的隐形污染源；32px 以下的远处真人本来也聚不进正确的簇。
            w, h = lb.size
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
            # 几何质量闸门（免费，仅用框+关键点）：把“不像人脸”的框在嵌入之前剔掉。
            # 实测这能把 P111 那种垃圾簇的框去掉 8 成以上（见 config 质量闸门注释）。
            kept = [f for f in kept if face_geometry_quality(f.bbox, f.kps) != QUALITY_REJECT]
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
    def embed_checked(
        self, img: Image.Image, face: Face
    ) -> tuple[np.ndarray | None, str]:
        """对齐 + 清晰度门槛 + 嵌入，返回 (嵌入向量|None, 质量档位)。

        为什么要返回质量档位：垃圾簇是从「一张烂脸先开出一个新 P 编号」开始吸人的。
        边缘质量的脸（极端侧脸/小脸/轻模糊）本身是真脸，丢掉可惜，但让它们参与
        “新建人物 + 更新质心” 就会把簇带偏 ⇒ 由调用方按档位决定（见 QUALITY_* 注释与
        person_store.register(allow_new=..., update_centroid=...)）。
        """
        verdict = face_geometry_quality(face.bbox, face.kps)
        if verdict == QUALITY_REJECT:
            return None, QUALITY_REJECT
        sess = self.registry.face_rec
        if sess is None:
            return None, QUALITY_REJECT
        try:
            with timing.span("face.align"):
                tensor = preprocess.face_align(img, face.kps)
            # 清晰度门槛放在嵌入之前：模糊脸既省下 r50 前向（~39ms/脸），
            # 也避免给库里灌噪声向量。方差只算一次（硬门槛与边缘档共用）。
            blur = aligned_blur(tensor)
            if blur < config.FACE_QUALITY_BLUR_MIN:
                return None, QUALITY_REJECT
            with timing.span("face.rec"):
                out = self.registry.run("face_rec", tensor)[0][0]
            emb = np.asarray(out, dtype=np.float32)
            n = np.linalg.norm(emb)
            if n <= 0:
                return None, QUALITY_REJECT
            emb = emb / n
            if verdict == QUALITY_OK and blur < config.FACE_MARGINAL_BLUR:
                verdict = QUALITY_MARGINAL
            return emb, verdict
        except Exception:
            return None, QUALITY_REJECT

    # ------------------------------------------------------------------
    def embed(self, img: Image.Image, face: Face) -> np.ndarray | None:
        """兼容旧调用点：只要嵌入向量（内部走带质量门槛的 embed_checked）"""
        emb, _ = self.embed_checked(img, face)
        return emb

    # ------------------------------------------------------------------
    def assign(
        self, emb: np.ndarray, verdict: str, photo_path: str, bbox: str
    ) -> tuple[str | None, float]:
        """按质量档位落库（两条链路共用的唯一入口）。

        - ok       ：正常登记（可新建人物、参与质心更新）
        - marginal ：只允许并入已有簇（不准新建、不更新质心）
                     这是防“垃圾簇”的关键：垃圾簇均从“一张烂脸开出一个新 P 编号、
                     然后把质心拖向自己、再吸更多烂脸”开始。
        返回 (person_id | None, sim)；None = 未入库（边缘质量且没匹配上）。
        """
        if verdict == QUALITY_MARGINAL:
            return self.store.register(
                emb, photo_path, bbox, allow_new=False, update_centroid=False
            )
        return self.store.register(emb, photo_path, bbox)

    # ------------------------------------------------------------------
    def process_photo(self, img: Image.Image, photo_path: str) -> list[dict]:
        """返回 [{person_id, bbox, sim}]，空列表 = 无可用人脸。

        质量分层用法：
          - ok：正常登记（可新建人物、参与质心更新）
          - marginal：只允许并入已有簇（不准新建、不更新质心）——这是防垃圾簇的关键
        """
        if not self.ready():
            return []
        # 检测走共用的缩放（letterbox()），对齐仍用原图 —— 只有 pipeline 才能
        # 复用已算好的 letterbox，这个独立入口自己算一次
        faces = self.detect_faces(preprocess.letterbox(img))
        hits: list[dict] = []
        for f in faces:
            emb, verdict = self.embed_checked(img, f)
            if emb is None:
                continue
            pid, sim = self.assign(emb, verdict, photo_path, f"{f.bbox}")
            if pid is None:
                continue
            hits.append({"person_id": pid, "bbox": f.bbox, "sim": round(sim, 3)})
        return hits


_face_service: FaceService | None = None


def get_face_service(registry) -> FaceService:
    global _face_service
    if _face_service is None:
        _face_service = FaceService(registry)
    return _face_service
