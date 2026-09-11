"""服务层：流水线编排（单张图片多路规则推理 → 仲裁 → 结果）

职责边界：
  - 解码图片（一次）→ 分发各通道
  - 收集元信息（格式/EXIF/尺寸）供仲裁器
  - 条件触发人脸标号（仅人物分支，省大量单人半身图开销）
  - 返回 schemas.ClassifyResult

v5：分类模型（cls）/ Places365 场景 / 花朵 / 食物专家通道已全部下线；
图像内容分类改由宿主「语义分类」（Chinese-CLIP）承担，本流水线只负责
人物 / 夜景 / 文档 三条规则通道。单张耗时从 ~437ms 降到 ~150ms。
"""
import os
import time

from .. import config, preprocess
from ..schemas import ClassifyResult, TopItem
from . import arbitrator, detector, face_service, ocr_service, tone_service


def _meta_of(img, path: str) -> dict:
    meta = {"format": "", "has_exif": False}
    try:
        from PIL import Image as _I

        with _I.open(path) as im:
            meta["format"] = (im.format or "").upper()
            meta["has_exif"] = bool(im.getexif())
    except Exception:
        pass
    return meta


def _needs_face(det_out) -> bool:
    """人脸标号条件触发：仅当仲裁器会命中「需要 person_ids」的人物规则分支。

    与 arbitrator 人物规则严格对齐：
      - portrait：最大人框 ≥ PORTRAIT_AREA
      - street：n≥3 且 max_area<STREET_MAX_AREA 且非密集车流
      - 合影：n≥2 且 max_area ≥ GROUP_AREA
    单人小框（路人）不返回 person_ids，跳过人脸标号（省 ~50-100ms/张）。
    """
    if not det_out.ready or det_out.count <= 0:
        return False
    n, max_area = det_out.count, det_out.max_area_ratio
    if max_area >= config.PORTRAIT_AREA:
        return True
    heavy_traffic = (
        getattr(det_out, "vehicle_count", 0) >= config.VEHICLE_HEAVY_N
        and max_area < config.VEHICLE_PERSON_AREA_MAX
    )
    if n >= config.STREET_PERSON_N and max_area < config.STREET_MAX_AREA and not heavy_traffic:
        return True
    if n >= 2 and max_area >= config.GROUP_AREA:
        return True
    return False


def classify_one(path: str, registry, use_face: bool = True) -> ClassifyResult | None:
    t0 = time.perf_counter()
    img = preprocess.open_image(path)
    if img is None:
        return None

    meta = _meta_of(img, path)
    det_out = detector.run(img, registry)
    # 影调自算（几十毫秒级，与 tone.rs 算法一致）→ 夜景通道
    tone_out = tone_service.compute_tone(img)
    # 文档 OCR：分类模型下线后不再有 cls 门控，模型可用即跑（~90ms/张），
    # 换来文档/截图识别的稳定性（旧链路对 most 非强证据图本来也会跑）。
    ocr_out = ocr_service.get_ocr_service(registry).run(img)

    face_hits: list[dict] = []
    if use_face and _needs_face(det_out):
        try:
            face_hits = face_service.get_face_service(registry).process_photo(img, path)
        except Exception:
            face_hits = []

    result = arbitrator.arbitrate(
        img, det_out, meta, face_hits, tone_out=tone_out, ocr_out=ocr_out
    )
    elapsed = (time.perf_counter() - t0) * 1000.0

    return ClassifyResult(
        path=path,
        file_name=os.path.basename(path),
        category=result.category,
        sub_category=result.sub_category,
        label=result.label,
        confidence=round(result.confidence, 4),
        top3=[TopItem(category=result.category, label=result.label,
                      confidence=round(result.confidence, 4))] if result.label else [],
        person_ids=result.person_ids,
        person_count=result.person_count,
        source=result.source,
        elapsed_ms=round(elapsed, 1),
    )
