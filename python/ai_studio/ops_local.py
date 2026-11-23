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

import hashlib
import os
from collections import OrderedDict
from dataclasses import dataclass
from typing import Any, Callable, Dict, Optional

import numpy as np
from PIL import Image

from . import (
    autograde,
    colortransfer,
    eyebright,
    faceparse,
    facewarp,
    hairlength,
    landmark,
    localmodels,
    skinretouch,
    skintone,
)
from .renderopts import EMPTY as EMPTY_CTX, RunContext

Warn = Optional[Callable[[str], None]]

# 与 workflows/local_beauty.py 原有口径一致：检测框外扩比例 / 过小脸阈值
_CONTEXT_MARGIN = 0.30
_MIN_FACE_PX = 48

# 参考图分析用的长边（与 workflows/color_transfer.py 的 _REF_LONG_EDGE 同口径）
_REF_LONG_EDGE = 1024

# 性别侧重的瘦脸比例（FEAT-210）：用户口径「男生默认 10%、女生 15%」→ 男 = 女 × 2/3。
# 大眼不区分性别（两档都是 20%）。
MALE_SLIM_RATIO = 2.0 / 3.0

# 发长判定结果的记忆表（脸框指纹 → 是否长发）：工作台调参时同一张脸反复请求，
# 命中即省掉一次 BiSeNet 解析（~120ms/脸）。有界 LRU，键含图尺寸与脸框内容哈希。
_HAIR_CACHE: "OrderedDict[tuple, bool]" = OrderedDict()
_HAIR_CACHE_MAX = 128


@dataclass
class FaceAnalysis:
    """一张脸的分析结果（FEAT-208 起的跨算子共享单元）。

    `mask` 是 BiSeNet 皮肤权重；`long_hair` 是发长判定（`hairlength`，零新增模型），
    供肤色保真的性别侧重使用。判不出长发时为 False —— 按男性侧重档（还原 + 轻提亮）
    处理，这是刻意的宽松失败模式。
    """

    bbox: tuple[int, int, int, int]
    box: tuple[int, int, int, int]
    mask: np.ndarray
    long_hair: bool = False


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
    slim: float = 0.15,
    eyes: float = 0.20,
    eye_bright: float = 0.10,
    warn: Warn = None,
    stats: Optional[dict] = None,
    auto_gender: bool = True,
) -> Image.Image:
    """68 点关键点驱动的瘦脸 + 大眼 + 眼睛提亮。单脸失败只跳过该脸，不抛。

    性别侧重（FEAT-210）：`auto_gender` 打开时逐脸按发长收敛瘦脸强度
    （男性 = 女性档 × `MALE_SLIM_RATIO`，用户口径 15% → 10%）；大眼两档相同，
    眼睛提亮也不分性别。判不出/解析模型未就绪 → 按用户给的值，不做收敛。

    顺序：**先提亮眼睛、再液化** —— 液化是对同一张图重采样，提亮结果跟着眼睛走，
    掩码与像素始终对得上（反过来会用到已过时的 68 点，眼睑边缘漏色）。
    """
    if slim <= 1e-3 and eyes <= 1e-3 and eye_bright <= 1e-3:
        return img                     # 三项都为 0 = 不处理，连检测都不必跑
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
            slim_face = slim
            if auto_gender:
                slim_face = slim_for_gender(slim, long_hair_hint(result, face["bbox"], warn))
            if eye_bright > 1e-3:
                result = eyebright.brighten_eyes(result, points, eye_bright)
            result = facewarp.reshape(result, points, slim=slim_face, eyes=eyes)
            touched += 1
        except Exception as e:  # noqa: BLE001
            _emit(warn, f"形变失败，该脸已跳过（{e}）")
    if stats is not None:
        stats["faces"] = stats.get("faces", 0) + touched
    return result


def slim_for_gender(slim: float, long_hair: bool | None) -> float:
    """性别侧重的瘦脸强度：长发（女性）用原值，非长发（男性）×2/3，未知不收敛。

    `None` = 无法判定（解析模型未就绪 / 解析失败）→ 用用户给的值，不悄悄按男性打折。
    """
    if long_hair is None:
        return slim
    return slim if long_hair else slim * MALE_SLIM_RATIO


def long_hair_hint(
    img: Image.Image, bbox: tuple[int, int, int, int], warn: Warn = None
) -> bool | None:
    """单脸发长（性别侧重）：只跑一次 BiSeNet 解析，复用调用方已有的检测框。

    返回 None 表示「无法判定」：解析模型未就绪（形变的唯一硬依赖是 det+68 点，
    不该因为缺 BiSeNet 就报错）或解析抛异常。宁可不动，也不要按错误的性别打折。

    带一个按「脸框指纹」的记忆表：工作台调参时同一张脸会被反复请求，指纹不变就
    直接命中，省掉每次 ~120ms 的解析（指纹 = 图尺寸 + 检测框 + 脸框 8×8 灰度哈希，
    照片被改动则指纹变化，不会读到旧结论）。
    """
    if not faceparse.ready():
        return None
    key = _face_key(img, bbox)
    if key is not None:
        hit = _HAIR_CACHE.get(key)
        if hit is not None:
            _HAIR_CACHE.move_to_end(key)
            return hit
    box = _context_box(bbox, img.width, img.height, _CONTEXT_MARGIN)
    if box is None:
        return None
    try:
        cls_map = faceparse.predict(img.crop(box))
    except Exception as e:  # noqa: BLE001
        _emit(warn, f"人脸解析失败，性别侧重已跳过（{e}）")
        return None
    verdict = _long_hair_of(cls_map, bbox, box)
    if key is not None:
        _HAIR_CACHE[key] = verdict
        if len(_HAIR_CACHE) > _HAIR_CACHE_MAX:
            _HAIR_CACHE.popitem(last=False)
    return verdict


def _face_key(
    img: Image.Image, bbox: tuple[int, int, int, int]
) -> tuple | None:
    """脸框指纹（发长判定的缓存键）。裁剪退化时返回 None（不缓存，照常解析）。"""
    x1, y1, x2, y2 = (int(round(v)) for v in bbox)
    x1, y1 = max(0, x1), max(0, y1)
    x2, y2 = min(img.width, x2), min(img.height, y2)
    if x2 - x1 < 8 or y2 - y1 < 8:
        return None
    thumb = img.crop((x1, y1, x2, y2)).convert("L").resize((8, 8))
    digest = hashlib.blake2b(thumb.tobytes(), digest_size=8).digest()
    return (img.size, (x1, y1, x2, y2), digest)


def _long_hair_of(cls_map: np.ndarray, bbox, box) -> bool:
    """由解析类别图判发长（`hairlength` 的入口，两处共用同一口径）。"""
    return hairlength.looks_long_hair(cls_map == faceparse.IDX["hair"], bbox, box)


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


def _analyze_faces(img: Image.Image, warn: Warn, purpose: str) -> list[FaceAnalysis]:
    """SCRFD 检测 + BiSeNet 解析，逐脸收集皮肤掩码与发长：list[FaceAnalysis]。

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
    out: list[FaceAnalysis] = []
    for f in usable:
        bbox = tuple(f["bbox"])
        box = _context_box(bbox, img.width, img.height, _CONTEXT_MARGIN)
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
        out.append(
            FaceAnalysis(
                bbox=bbox,
                box=box,
                mask=mask,
                long_hair=_long_hair_of(cls_map, bbox, box),
            )
        )
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
    for face in analysis:
        x1, y1, x2, y2 = face.box
        # 半径挂钩脸宽（不是图幅）：高分辨率原图不再被过度抹平
        r = skinretouch.radius_for_face(face.bbox[2] - face.bbox[0])
        patch = skinretouch.smooth_skin(
            arr[y1:y2, x1:x2], face.mask, strength=strength, radius=r, keep_texture=texture
        )
        result = skinretouch.paste_region(
            result, patch, face.box, skinretouch._feather(face.mask, max(1, int(round(r))))
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
    # 形变移动了像素：磨皮算过的掩码与本图不再对齐，必须作废（FEAT-208）。
    # 但不置 geometry_dirty —— 局部液化后掩码仍能标回原图取锚点（实测 ≤0.52°）。
    ctx.drop_face_analysis()
    return reshape_image(
        img,
        float(params.get("slim", 0.15)),
        float(params.get("eyes", 0.20)),
        float(params.get("eye_bright", 0.10)),
        auto_gender=True,
    )


def op_skin_smooth(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    # 默认上下文（直接调用算子）没有跨步通道：不往模块级单例 EMPTY 里写分析结果
    scratch = None if ctx is EMPTY_CTX else getattr(ctx, "scratch", None)
    return smooth_image(
        img,
        float(params.get("strength", 0.45)),
        float(params.get("texture", 0.55)),
        scratch=scratch,
    )


# 参考图迁移产物的批量复用表（键含 mtime/size，改图即失效）：链条批量执行时
# 同一批参考图逐图重算 Lab 统计是纯浪费。有界 LRU，模式与 _HAIR_CACHE 一致。
_REF_CACHE: "OrderedDict[tuple, object]" = OrderedDict()
_REF_CACHE_MAX = 16


def _split_ref_paths(raw: str) -> list[str]:
    """链条步参数是标量，多张参考图用 `|` 拼接（Windows 文件名不允许 `|`，无歧义）。"""
    seen, uniq = set(), []
    for part in raw.split("|"):
        p = part.strip()
        if p and p not in seen:
            seen.add(p)
            uniq.append(p)
    return uniq


def _fit_ref_image(img: Image.Image) -> np.ndarray:
    """参考图降到长边 `_REF_LONG_EDGE` 再转数组（与 workflows/color_transfer.py 同口径）。"""
    w, h = img.size
    m = max(w, h)
    if m > _REF_LONG_EDGE:
        s = _REF_LONG_EDGE / float(m)
        img = img.resize((max(1, int(round(w * s))), max(1, int(round(h * s)))), Image.LANCZOS)
    return np.asarray(img, dtype=np.uint8)


def _ref_artifact(paths: list[str], method: str):
    """取参考图迁移产物（LRU 缓存）：reinhard → 合并 Lab 统计；histogram → 逐通道已排序像素列。

    单张参考图时 `merge_reference_stats` 直取该图统计，行为与旧版逐像素传图完全一致。
    """
    key = (method, tuple((p, os.stat(p).st_mtime_ns, os.stat(p).st_size) for p in paths))
    hit = _REF_CACHE.get(key)
    if hit is not None:
        _REF_CACHE.move_to_end(key)
        return hit
    arrs = [_fit_ref_image(Image.open(p).convert("RGB")) for p in paths]
    if method == "histogram":
        artifact = colortransfer.histogram_ref_sorted(arrs)
    else:
        artifact = colortransfer.merge_reference_stats(
            [colortransfer.reference_stats(a) for a in arrs]
        )
    while len(_REF_CACHE) >= _REF_CACHE_MAX:
        _REF_CACHE.popitem(last=False)
    _REF_CACHE[key] = artifact
    return artifact


def op_color_transfer(
    img: Image.Image, params: Dict[str, Any], providers: Any, ctx: RunContext = EMPTY_CTX
) -> Image.Image:
    """参考图调色：按参考图的 Lab 统计迁移（Reinhard / 直方图），支持多张参考图等权提取共性。

    参考图以**路径参数**进链（`reference_path`）：链条的每步只能带标量参数，
    塞不下一张图，所以步骤参数里存路径（多张用 `|` 拼接）、执行时在此处解码。
    参考图只取统计量，降到长边 1024 再转 Lab（与 `workflows/color_transfer.py`
    同一口径，24MP 全量转 Lab 要 2145ms，1MP 只要 24ms，统计量与分辨率无关）。
    `l_weight` 控制明度迁移比例（1 = 与参考图明暗一致，0 = 只迁色彩不动明暗）。
    """
    paths = _split_ref_paths(str(params.get("reference_path", "") or ""))
    if not paths:
        raise ValueError("未选择参考图（展开这一步，在「参考图」里选择一张图片）")
    missing = [p for p in paths if not os.path.isfile(p)]
    if missing:
        raise FileNotFoundError(f"参考图不存在: {missing[0]}")
    if len(paths) > 10:
        raise ValueError(f"参考图最多 10 张（当前 {len(paths)} 张），请删掉一些后再试")

    method = str(params.get("method", "reinhard"))
    strength = max(0.0, min(1.0, float(params.get("strength", 1.0))))
    l_weight = max(0.0, min(1.0, float(params.get("l_weight", 1.0))))
    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    if method == "histogram":
        out = colortransfer.histogram_match_sorted(arr, _ref_artifact(paths, "histogram"), strength=strength)
    else:
        out = colortransfer.reinhard_transfer_stats(arr, _ref_artifact(paths, "reinhard"), l_weight=l_weight, strength=strength)
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
    faces_cur: list[FaceAnalysis], faces_src: list[FaceAnalysis]
) -> list[tuple[FaceAnalysis, FaceAnalysis]]:
    """当前图与原图的脸配对：**一律按框中心最近邻贪心**，落单的跳过。

    为什么不按检测序号 zip：SCRFD 的输出顺序会随图像内容变化（形变/调色后都可能重排），
    数量相等时按序号配也可能把两张脸配错 —— 实测强形变下两次锚点差到 ΔC≈4。
    几何一致的同尺寸链路根本走不到这里（肤色锚点直接复用掩码），这里只服务
    「整幅重绘后再肤色」这类需要跨图配对的场景。
    """
    def _center(face: FaceAnalysis) -> tuple[float, float]:
        x1, y1, x2, y2 = face.box
        return ((x1 + x2) * 0.5, (y1 + y2) * 0.5)

    used: set[int] = set()
    out: list[tuple[FaceAnalysis, FaceAnalysis]] = []
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


def _can_reuse_anchor_masks(
    img: Image.Image, source: Image.Image | None, source_aligned: bool
) -> bool:
    """原图锚点能否直接复用本图的人脸掩码：几何一致 + 同尺寸。

    实测（2026-11-22，3 张 12MP 人像、每人 2 脸）：几何一致时复用掩码取锚点，相对
    「在原图上重新检测」的偏差 Δh ≤0.11°、ΔC ≤0.10、Δcheek_a ≤0.21；连形变后
    （slim=0.8）也只有 Δh ≤0.52°、ΔC ≤0.45、Δcheek_a ≤0.54 —— 可忽略，却省掉一次
    SCRFD + 每脸一次 BiSeNet（2 脸实测 ~375ms/张，占肤色步的 65%）。
    只有整幅重绘/缩放（`mark_geometry_changed`）或尺寸不同才必须回退重分析。

    另外：复用掩码还顺带绕开了「原图重新检测后按序号配对」的隐患 —— 强形变会改变
    SCRFD 的检测顺序，序号 zip 可能把两张脸配错（实测两次锚点差到 ΔC≈4）；复用同一份
    掩码时根本没有配对这一步。
    """
    return source is not None and source_aligned and source.size == img.size


def skin_tone_image(
    img: Image.Image,
    params: Dict[str, Any],
    source: Image.Image | None = None,
    faces_cur: Optional[list[FaceAnalysis]] = None,
    faces_src: Optional[list[FaceAnalysis]] = None,
    source_aligned: bool = True,
    warn: Warn = None,
    stats: Optional[dict] = None,
) -> Image.Image:
    """分区肤色保真（FEAT-208/209）：皮肤掩码内把色度朝原图锚点拉回，再叠加偏置。

    `faces_cur` / `faces_src`：已分析好的 `FaceAnalysis` 列表。
    复用纪律：磨皮刚跑过的分析直接传入（几何未变掩码对得上），不重复推理；
    没有就现算。`source` 缺省 → 锚点退化为当前图（还原项归零，只剩偏置）。

    `source_aligned`（FEAT-209）：本图与 `source` 的几何是否一致。一致且同尺寸时
    原图锚点**复用本图的掩码**，不再对原图重跑检测/解析；改过几何（形变/超分/缩放）
    才回退到原图重新分析。

    性别侧重（FEAT-209）：逐脸按发长取 `skintone.for_gender` —— 长发用用户档位
    （更白更嫩红），其余只还原原肤色 + 轻提亮。判错只会让妆感变淡，不会毁片。
    """
    p = skintone.SkinParams.from_raw(params)
    if p.is_noop():
        return img
    if faces_cur is None:
        faces_cur = _analyze_faces(img, warn, "肤色校正")
    if not faces_cur:
        return img
    if faces_src is None:
        if _can_reuse_anchor_masks(img, source, source_aligned):
            faces_src = faces_cur
        elif source is not None:
            faces_src = _analyze_faces(source, None, "肤色锚点")
        else:
            faces_src = faces_cur
    if not faces_src:
        faces_src = faces_cur

    # 锚点像素：复用掩码时几何一致 → 取原图；锚点来自原图自己的分析 → 取原图；
    # 都没有（source 缺省）→ 退化为当前图。
    if faces_src is faces_cur:
        src_img = source if (source is not None and source.size == img.size) else img
    else:
        src_img = source if source is not None else img

    arr = np.asarray(img.convert("RGB"), dtype=np.uint8)
    src_arr = arr if src_img is img else np.asarray(src_img.convert("RGB"), dtype=np.uint8)
    result = arr
    touched = 0
    for face, s_face in _pair_faces(faces_cur, faces_src):
        cheek = skintone.cheek_region(face.mask.shape)
        s_cheek = skintone.cheek_region(s_face.mask.shape)
        x1, y1, x2, y2 = s_face.box
        anchor = skintone.anchor_from_pixels(
            src_arr[y1:y2, x1:x2], s_face.mask, s_cheek
        )
        x1, y1, x2, y2 = face.box
        patch = skintone.correct_region(
            arr[y1:y2, x1:x2],
            face.mask,
            cheek,
            anchor,
            skintone.for_gender(p, face.long_hair),
        )
        result = skinretouch.paste_region(
            result, patch, face.box, skinretouch._feather(face.mask, skintone.FEATHER_RADIUS)
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
        img,
        params,
        source=getattr(ctx, "source", None),
        faces_cur=faces_cur,
        source_aligned=not getattr(ctx, "geometry_dirty", False),
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
    "FaceAnalysis",
    "MALE_SLIM_RATIO",
    "grade_image",
    "reshape_image",
    "slim_for_gender",
    "long_hair_hint",
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
