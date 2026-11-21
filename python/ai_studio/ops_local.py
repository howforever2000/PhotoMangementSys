"""本地原子算子：单图原语 + `fn(img, params, providers)` 形态包装（FEAT-168）。

分层职责：
  - 本模块只处理「一张 PIL.Image」，**不含**读盘 / 落盘 / 进度 / 取消 / 作业编排；
  - `grade_image` / `reshape_image` / `smooth_image` 是 workflows 与 OPS 共用的单图原语，
    避免同一套算法在「单步工具」和「配方链路」里各写一份、参数逐渐漂移；
  - `OPS_LOCAL` 由 `ops.resolve()` 并入总表，`pipeline.execute_plan` 不感知算子在哪。

复用说明：图像处理算法本身全部来自既有纯函数模块（`autograde` / `facewarp` /
`landmark` / `faceparse` / `skinretouch`），本文件只做「PIL.Image 适配 + 多脸循环」。
"""
from __future__ import annotations

import os
from typing import Any, Callable, Dict, Optional

import numpy as np
from PIL import Image

from . import autograde, colortransfer, faceparse, facewarp, landmark, localmodels, skinretouch, skintone
from .renderopts import EMPTY as EMPTY_CTX, RunContext

Warn = Optional[Callable[[str], None]]

# 与 workflows/local_beauty.py 原有口径一致：检测框外扩比例 / 过小脸阈值
_CONTEXT_MARGIN = 0.30
_MIN_FACE_PX = 48

# 参考图分析用的长边（与 workflows/color_transfer.py 的 _REF_LONG_EDGE 同口径）
_REF_LONG_EDGE = 1024


def _emit(warn: Warn, text: str) -> None:
    if warn is not None:
        warn(text)


def grade_image(
    img: Image.Image,
    style: str = "natural",
    strength: float = 1.0,
) -> tuple[Image.Image, Any, Any]:
    """自适应调色：统计 → 逐图 LUT → 查表。返回 (成片, GradeStats, (lut, n))。

    独立自然饱和度微调由 `local_vibrance` 算子承担（FEAT-206），
    调色算子自身不再带 vibrance 参数。
    """
    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    graded, stats, lut = autograde.grade(arr, style=style, strength=strength)
    return Image.fromarray(graded), stats, lut


def reshape_image(
    img: Image.Image,
    slim: float = 0.3,
    eyes: float = 0.15,
    warn: Warn = None,
    stats: Optional[dict] = None,
) -> Image.Image:
    """68 点关键点驱动的瘦脸 + 大眼。单脸失败只跳过该脸，不抛（懒人链路纪律）。"""
    try:
        faces = localmodels.detect_faces(img)
    except Exception as e:  # noqa: BLE001
        _emit(warn, f"人脸检测失败，已跳过形变（{e}）")
        return img
    if not faces:
        _emit(warn, "未检测到人脸，形变步骤原样通过")
        return img

    result = img
    touched = 0
    for face in faces:
        try:
            points, confidence = landmark.detect_68(result, face["bbox"], face.get("kps"))
            if points is None:
                _emit(warn, f"有脸过小（<{landmark.MIN_FACE_PX}px），已跳过该脸")
                continue
            if confidence < landmark.MIN_CONFIDENCE:
                _emit(warn, f"关键点置信度偏低（{confidence:.2f}），已跳过该脸")
                continue
            result = facewarp.reshape(result, points, slim=slim, eyes=eyes)
            touched += 1
        except Exception as e:  # noqa: BLE001
            _emit(warn, f"形变失败，该脸已跳过（{e}）")
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return result


def _context_box(bbox, width: int, height: int, margin: float):
    x1, y1, x2, y2 = bbox
    bw, bh = x2 - x1, y2 - y1
    if bw <= 0 or bh <= 0:
        return None
    mx, my = int(bw * margin), int(bh * margin)
    bx1, by1 = max(0, int(x1) - mx), max(0, int(y1) - my)
    bx2, by2 = min(width, int(x2) + mx), min(height, int(y2) + my)
    if bx2 - bx1 < _MIN_FACE_PX or by2 - by1 < _MIN_FACE_PX:
        return None
    return bx1, by1, bx2, by2


def _analyze_faces(img: Image.Image, warn: Warn, purpose: str) -> list[tuple]:
    """SCRFD 检测 + BiSeNet 解析，逐脸收集皮肤掩码：[(bbox, box, mask)]。

    磨皮与肤色校正共用（FEAT-208）：同一条链里模型只跑一次，后到的算子
    从 ctx.scratch 复用先到的结果，不再重复推理。
    """
    try:
        faces = localmodels.detect_faces(img)
    except Exception as e:  # noqa: BLE001
        _emit(warn, f"人脸检测失败，已跳过{purpose}（{e}）")
        return []
    usable = [
        f for f in faces
        if min(f["bbox"][2] - f["bbox"][0], f["bbox"][3] - f["bbox"][1]) >= _MIN_FACE_PX
    ]
    if not usable:
        _emit(warn, f"未检测到可用人脸，{purpose}步骤原样通过")
        return []
    out: list[tuple] = []
    for f in usable:
        box = _context_box(f["bbox"], img.width, img.height, _CONTEXT_MARGIN)
        if box is None:
            continue
        x1, y1, x2, y2 = box
        try:
            cls_map = faceparse.predict(img.crop(box))
        except Exception as e:  # noqa: BLE001
            _emit(warn, f"人脸解析失败，该脸已跳过（{e}）")
            continue
        mask = faceparse.skin_mask(cls_map)
        if mask.max() <= 0:
            continue
        out.append((tuple(f["bbox"]), box, mask))
    return out


def smooth_image(
    img: Image.Image,
    strength: float = 0.45,
    texture: float = 0.55,
    warn: Warn = None,
    stats: Optional[dict] = None,
    scratch: Optional[dict] = None,
) -> Image.Image:
    """分区磨皮：SCRFD → BiSeNet → 只压皮肤高频 → 带 alpha 羽化贴回。

    `scratch` 给出时（配方链路）：把本次人脸分析写进去，紧随其后的肤色保真
    直接复用同一份掩码 —— 磨皮只改皮肤像素的纹理不改几何，掩码对磨皮输出
    依然对得上，肤色算子不必再跑一次检测+解析。
    """
    analysis = _analyze_faces(img, warn, "磨皮")
    if scratch is not None:
        scratch["face_analysis"] = analysis
    if not analysis:
        return img

    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    result = arr
    touched = 0
    for bbox, box, mask in analysis:
        x1, y1, x2, y2 = box
        # 半径挂钩脸宽（不是图幅）：高分辨率原图不再被过度抹平
        r = skinretouch.radius_for_face(bbox[2] - bbox[0])
        patch = skinretouch.smooth_skin(
            arr[y1:y2, x1:x2], mask, strength=strength, radius=r, keep_texture=texture
        )
        result = skinretouch.paste_region(
            result, patch, box, skinretouch._feather(mask, max(1, int(round(r))))
        )
        touched += 1
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return Image.fromarray(result)


# ---------------- OPS 形态包装（签名与 ops.py 既有算子一致） ----------------


def op_auto_grade(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    out, _, _ = grade_image(
        img,
        str(params.get("style", "natural")),
        float(params.get("strength", 1.0)),
    )
    return out


def op_face_reshape(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    scratch = getattr(ctx, "scratch", None)
    if scratch:
        # 形变移动了像素：磨皮算过的掩码与本图不再对齐，必须作废（FEAT-208）
        scratch.pop("face_analysis", None)
    return reshape_image(
        img,
        float(params.get("slim", 0.3)),
        float(params.get("eyes", 0.15)),
    )


def op_skin_smooth(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    return smooth_image(
        img,
        float(params.get("strength", 0.45)),
        float(params.get("texture", 0.55)),
        scratch=getattr(ctx, "scratch", None),
    )


def op_color_transfer(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """参考图调色：按参考图的 Lab 统计迁移（Reinhard / 直方图）。

    参考图以**路径参数**进链（`referencePath`）：链条的每步只能带标量参数，
    塞不下一张图，所以步骤参数里存路径、执行时在此处解码。参考图只取统计量，
    降到长边 1024 再转 Lab（与 `workflows/color_transfer.py` 同一口径，24MP
    全量转 Lab 要 2145ms，1MP 只要 24ms，统计量与分辨率无关）。
    """
    ref_path = str(params.get("reference_path", "") or "").strip()
    if not ref_path:
        raise ValueError("未选择参考图（展开这一步，在「参考图」里选择一张图片）")
    if not os.path.isfile(ref_path):
        raise FileNotFoundError(f"参考图不存在: {ref_path}")

    ref = Image.open(ref_path).convert("RGB")
    w, h = ref.size
    m = max(w, h)
    if m > _REF_LONG_EDGE:
        s = _REF_LONG_EDGE / float(m)
        ref = ref.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)
    ref_arr = np.asarray(ref, dtype=np.uint8)

    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    strength = max(0.0, min(1.0, float(params.get("strength", 1.0))))
    if str(params.get("method", "reinhard")) == "histogram":
        out = colortransfer.histogram_match(arr, ref_arr, strength=strength)
    else:
        out = colortransfer.reinhard_transfer(arr, ref_arr, strength=strength)
    return Image.fromarray(out)




def op_local_vibrance(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """独立自然饱和度（FEAT-206）：双向微调，0 = 原样通过。见 `autograde.vibrance_adjust`。"""
    amount = float(params.get("vibrance", 0.0) or 0.0)
    if abs(amount) < 1e-3:
        return img
    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    return Image.fromarray(autograde.vibrance_adjust(arr, amount))

def _pair_faces(
    faces_cur: list[tuple], faces_src: list[tuple]
) -> list[tuple[tuple, tuple]]:
    """当前图与原图的脸配对：数量一致按检测顺序（SCRFD 确定性）；
    不一致（形变丢脸 / 误检差异）按框中心最近邻贪心配对，落单的跳过。"""
    if len(faces_cur) == len(faces_src):
        return list(zip(faces_cur, faces_src))

    def _center(face: tuple) -> tuple[float, float]:
        x1, y1, x2, y2 = face[1]
        return ((x1 + x2) * 0.5, (y1 + y2) * 0.5)

    used: set[int] = set()
    out: list[tuple[tuple, tuple]] = []
    for f in faces_cur:
        cx, cy = _center(f)
        best, best_d = None, None
        for j, g in enumerate(faces_src):
            if j in used:
                continue
            gx, gy = _center(g)
            d = (cx - gx) ** 2 + (cy - gy) ** 2
            if best_d is None or d < best_d:
                best, best_d = j, d
        if best is None:
            break
        used.add(best)
        out.append((f, faces_src[best]))
    return out


def skin_tone_image(
    img: Image.Image,
    params: Dict[str, Any],
    source: Image.Image | None = None,
    faces_cur: Optional[list[tuple]] = None,
    faces_src: Optional[list[tuple]] = None,
    warn: Warn = None,
    stats: Optional[dict] = None,
) -> Image.Image:
    """分区肤色保真（FEAT-208）：皮肤掩码内把色度朝原图锚点拉回，再叠加白黄·嫩红偏置。

    `faces_cur` / `faces_src`：已分析好的 [(bbox, box, skin_mask)]。
    复用纪律：磨皮刚跑过的分析直接传入（几何未变掩码对得上），不重复推理；
    没有就现算。`source` 缺省 → 锚点退化为当前图（还原项归零，只剩偏置）。
    """
    p = skintone.SkinParams.from_raw(params)
    if p.is_noop():
        return img
    if faces_cur is None:
        faces_cur = _analyze_faces(img, warn, "肤色校正")
    if not faces_cur:
        return img
    if faces_src is None:
        if source is not None:
            faces_src = _analyze_faces(source, None, "肤色锚点")
        else:
            faces_src = faces_cur
    if not faces_src:
        faces_src = faces_cur

    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    src_arr = (
        np.asarray(source.convert("RGB"), dtype=np.uint8)
        if source is not None and faces_src is not faces_cur
        else arr
    )
    result = arr
    touched = 0
    for (_bbox, box, mask), (_s_bbox, s_box, s_mask) in _pair_faces(faces_cur, faces_src):
        cheek = skintone.cheek_region(mask.shape)
        s_cheek = skintone.cheek_region(s_mask.shape)
        x1, y1, x2, y2 = s_box
        anchor = skintone.anchor_from_pixels(
            src_arr[y1:y2, x1:x2], s_mask, s_cheek
        )
        x1, y1, x2, y2 = box
        patch = skintone.correct_region(arr[y1:y2, x1:x2], mask, cheek, anchor, p)
        result = skinretouch.paste_region(
            result, patch, box, skinretouch._feather(mask, skintone.FEATHER_RADIUS)
        )
        touched += 1
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return Image.fromarray(result)


def op_skin_tone(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """肤色保真算子：原图锚点取自 ctx.source；人脸分析优先复用 ctx.scratch
    里磨皮留下的结果（消费即取走，几何算子会提前作废它）。"""
    scratch = getattr(ctx, "scratch", None)
    faces_cur = None
    if scratch:
        faces_cur = scratch.pop("face_analysis", None)
    return skin_tone_image(
        img, params, source=getattr(ctx, "source", None), faces_cur=faces_cur
    )


OPS_LOCAL = {
    "auto_grade": op_auto_grade,
    "color_transfer": op_color_transfer,
    "face_reshape": op_face_reshape,
    "skin_smooth": op_skin_smooth,
    "skin_tone": op_skin_tone,
    "local_vibrance": op_local_vibrance,
}

__all__ = [
    "grade_image",
    "reshape_image",
    "smooth_image",
    "skin_tone_image",
    "op_auto_grade",
    "op_color_transfer",
    "op_face_reshape",
    "op_skin_smooth",
    "op_skin_tone",
    "op_local_vibrance",
    "OPS_LOCAL",
]
