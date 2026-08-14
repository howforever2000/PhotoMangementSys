"""服务层：流水线编排（单张图片多路推理 → 仲裁 → 结果）

职责边界：
  - 解码图片（一次）→ 分发各通道
  - 收集元信息（格式/EXIF/尺寸）供仲裁器
  - 条件触发专家通道（flower/food，懒加载模型，避免全量推理开销）
  - 返回 schemas.ClassifyResult
"""
import os
import time

from .. import config, preprocess
from ..schemas import ClassifyResult, TopItem
from . import (arbitrator, classifier, detector, face_service, flower_service,
               food_service, ocr_service, scene_service, tone_service)


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


def _food_trigger(cls_out, scene_out) -> bool:
    """食物专家触发条件：cls 是 other/food 或 scene 命中餐厅语义。"""
    if not config.ENABLE_FOOD_EXPERT:
        return False
    if cls_out.ready and cls_out.category in ("food", "other"):
        return True
    if scene_out is not None and scene_out.ready:
        lab = (scene_out.label or "").lower()
        if any(k in lab for k in ("restaurant", "cafe", "bar", "dining", "food", "kitchen")):
            return True
    return False


def _needs_face(det_out) -> bool:
    """人脸标号条件触发（P2 性能优化）：仅当仲裁器会命中「需要 person_ids」的人物规则分支。

    与 arbitrator 人物规则严格对齐：
      - portrait：最大人框 ≥ PORTRAIT_AREA
      - street：n≥3 且 max_area<STREET_MAX_AREA 且非密集车流
      - 合影：n≥2 且 max_area ≥ GROUP_AREA
    单人小框（路人，max_area 10%~30%）或 2 人小框不返回 person_ids，
    跳过人脸标号（省 SCRFD 检测 + ArcFace 嵌入 + SQLite 匹配 ~50-100ms/张）。
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
    cls_out = classifier.run(img, registry)
    det_out = detector.run(img, registry)
    scene_out = scene_service.get_scene_service(registry).run(img)
    # Phase 1：影调自算（几十毫秒级，与 tone.rs 算法一致）
    tone_out = tone_service.compute_tone(img)
    # Phase 4：OCR 条件触发（仅 text/other/低置信 才跑，省非文本图 ~90ms）
    ocr_out = None
    if cls_out.ready and (
        cls_out.category in ("text", "other")
        or cls_out.confidence < config.SCENE_OVERRIDE_CONF
    ):
        ocr_out = ocr_service.get_ocr_service(registry).run(img)
    # Phase 3/5：专家通道条件触发
    flower_out = None
    if config.ENABLE_FLOWER_EXPERT and cls_out.ready and cls_out.category == "plant_flower":
        flower_out = flower_service.get_flower_service(registry).run(img)
    food_out = None
    if _food_trigger(cls_out, scene_out):
        food_out = food_service.get_food_service(registry).run(img)

    face_hits: list[dict] = []
    if use_face and _needs_face(det_out):
        try:
            face_hits = face_service.get_face_service(registry).process_photo(img, path)
        except Exception:
            face_hits = []

    result = arbitrator.arbitrate(
        img, cls_out, det_out, scene_out, meta, face_hits,
        tone_out=tone_out, ocr_out=ocr_out,
        flower_out=flower_out, food_out=food_out,
    )
    elapsed = (time.perf_counter() - t0) * 1000.0

    return ClassifyResult(
        path=path,
        file_name=os.path.basename(path),
        category=result.category,
        sub_category=result.sub_category,
        label=result.label,
        confidence=round(result.confidence, 4),
        top3=[TopItem(**t) for t in result.top3],
        person_ids=result.person_ids,
        person_count=result.person_count,
        source=result.source,
        elapsed_ms=round(elapsed, 1),
    )
