"""服务层：规则仲裁器

将多路结果（分类 cls / 检测 det / 场景 scene / 影调 tone / OCR / 专家）+
元信息融合为最终大类。

规则优先级（Phase 1~5 全量，VCR 手册 v2 §三）：
  1. 人物规则（person 优先于一切——taxonomy 决策 Q3：证件照归特写可接受）
  2. OCR 强证据（文字框面积 >15%）→ document（sub=paper）
  3. 截图启发式（PNG + 无 EXIF + 屏幕比例 + 弱分类证据）→ document（sub=screenshot）
  4. 夜景规则（scene 命中夜间词 + 低调 / 低调 + 城市 / 极暗 + 自然）→ night_scene
  5. 场景路覆盖（cls 弱证据且 scene 置信度 ≥ SCENE_TAKEOVEN_MIN）
  6. 花朵专家（cls=plant_flower 时，置信度 ≥ FLOWER_CONF → flower 否则 plant）
  7. 食物专家（专家通道就绪且触发条件满足时接管）
  8. 兜底 cls 大类；动物子类低置信不强判；食物温和修正
"""
from dataclasses import dataclass, field

from .. import config


@dataclass
class FinalResult:
    category: str
    sub_category: str = ""
    label: str = ""
    confidence: float = 0.0
    top3: list[dict] = field(default_factory=list)
    person_ids: list[str] = field(default_factory=list)
    person_count: int = 0
    source: str = ""


def _near_screen_ratio(w: int, h: int) -> bool:
    if w <= 0 or h <= 0:
        return False
    ratio = w / h
    for aw, ah in config.SCREEN_ASPECTS:
        ref = aw / ah
        if abs(ratio - ref) / ref < config.SCREEN_TOL:
            return True
    return False


def is_screenshot(img, meta: dict, cls_cat: str, cls_conf: float) -> bool:
    """截图启发式：PNG 格式 + 无 EXIF + 分辨率符合屏幕比例。"""
    fmt = (meta.get("format") or "").upper()
    if fmt != "PNG":
        return False
    if meta.get("has_exif", True):
        return False
    w, h = img.size
    if min(w, h) < config.SCREEN_MIN_SIZE:
        return False
    if not _near_screen_ratio(w, h):
        return False
    # 分类弱证据：文本/其他 或 低置信度
    return cls_cat in ("text", "other") or cls_conf < config.SCENE_OVERRIDE_CONF


def _night_hit(cls_out, scene_out, tone_out) -> bool:
    """夜景判定（Phase 1，实测校准版）。

    实测：Places365 在夜景图上全部 low-conf 乱判（sky 0.022 / orchestra_pit /
    stage / forest），scene 语义不可靠 → 以影调分档为主：
      a) luma < NIGHT_LUMA_DARK(25) → 无条件夜景（烟花/满月 luma 4.5~23）
      b) luma < NIGHT_LUMA_DEEP(45) → 需弱 cls 证据（信号灯 conf 1.0 排除）
      c) luma < NIGHT_LUMA(60) → 需 scene 夜间词或城市场景
      d) luma < NIGHT_LUMA_SKY(70) → 需 scene 夜间词（月亮天空，如 #30 luma 63.9）
    主体明确的照片排除：车辆（白天环卫车 #53）与动物（白猫 #42）。
    """
    if tone_out is None or not tone_out.ready or tone_out.avg_luma is None:
        return False
    cls_cat = cls_out.category if cls_out is not None else "other"
    cls_conf = cls_out.confidence if cls_out is not None else 0.0
    if cls_cat in ("vehicle", "animal"):
        return False
    luma = tone_out.avg_luma
    night_label = False
    scene_urban = False
    if scene_out is not None and scene_out.ready:
        lab = (scene_out.label or "").lower()
        night_label = any(k in lab for k in config.NIGHT_KEYWORDS)
        scene_urban = scene_out.category in ("cityscape", "architecture", "street")
    if luma < config.NIGHT_LUMA_DARK:
        return True
    if luma < config.NIGHT_LUMA_DEEP:
        return cls_conf < config.NIGHT_CLS_CONF_MAX
    if luma < config.NIGHT_LUMA:
        return night_label or scene_urban
    if luma < config.NIGHT_LUMA_SKY:
        return night_label
    return False


def _scene_takeover_conf(scene_out, cls_cat: str) -> float:
    """场景接管所需的最低置信度：全局下限 SCENE_TAKEOVER_MIN。"""
    return max(config.SCENE_TAKEOVER_MIN, config.SCENE_CONF_STRONG
               if cls_cat in ("landscape_nature", "architecture") else 0.0)


def arbitrate(img, cls_out, det_out, scene_out, meta: dict, face_hits: list[dict],
              tone_out=None, ocr_out=None, flower_out=None, food_out=None) -> FinalResult:
    # ---- 分类路基础信息 ----
    cls_cat = cls_out.category
    cls_conf = cls_out.confidence
    cls_label = cls_out.label
    cat_scores = cls_out.cat_scores or []
    source_parts = ["cls"] if cls_out.ready else []

    # ================= 1. 人物规则（最高优先） =================
    if det_out.ready and det_out.count > 0:
        n, max_area = det_out.count, det_out.max_area_ratio
        person_ids = [h["person_id"] for h in face_hits]
        # 校准（2026-08-13 §7.3.1）：密集车流中 det 会把车头/车窗误检为人（e-7278），
        # 且假框置信度 0.48~0.73 高于提高 PERSON_CONF_MIN 的任何实用取值；
        # 车辆 ≥ VEHICLE_HEAVY_N 且人框全为小框时跳过 street（回退 cls=vehicle）。
        heavy_traffic = (
            getattr(det_out, "vehicle_count", 0) >= config.VEHICLE_HEAVY_N
            and max_area < config.VEHICLE_PERSON_AREA_MAX
        )
        if n >= config.STREET_PERSON_N and max_area < config.STREET_MAX_AREA and not heavy_traffic:
            return FinalResult(
                category="street",
                label=f"扫街·{n}人",
                confidence=det_out.max_conf,
                top3=[{"category": "street", "label": f"扫街·{n}人", "confidence": det_out.max_conf}],
                person_ids=person_ids,
                person_count=n,
                source="det",
            )
        if max_area >= config.PORTRAIT_AREA:
            label = f"人物特写·{len(person_ids) or n}人"
            return FinalResult(
                category="portrait",
                label=label,
                confidence=det_out.max_conf,
                top3=[{"category": "portrait", "label": label, "confidence": det_out.max_conf}],
                person_ids=person_ids,
                person_count=n,
                source="det+face" if person_ids else "det",
            )
        if n >= 2 and max_area >= config.GROUP_AREA:
            label = f"合影·{len(person_ids) or n}人"
            return FinalResult(
                category="portrait",
                label=label,
                confidence=det_out.max_conf,
                top3=[{"category": "portrait", "label": label, "confidence": det_out.max_conf}],
                person_ids=person_ids,
                person_count=n,
                source="det+face" if person_ids else "det",
            )
        # 单人小框 → 路人，不覆盖；记入 source 但不改类别
        if max_area >= config.IGNORE_PERSON_AREA:
            source_parts.append("det")

    # ================= 2. OCR 强证据 → document =================
    # 至少 2 个文字框 + 面积占比达标（单巨型框/纹理碎块不算，实测拦波斯菊/黄昏/山地车）
    if ocr_out is not None and ocr_out.ready and ocr_out.n_boxes >= 2 \
            and ocr_out.area_ratio >= config.OCR_AREA_STRONG:
        return FinalResult(
            category="document",
            sub_category="paper",
            label="文档",
            confidence=min(0.99, 0.5 + ocr_out.area_ratio),
            top3=[{"category": "document", "label": "文档", "confidence": 0.9}],
            source="ocr",
        )

    # ================= 3. 截图启发式 → document（弱证据） =================
    if is_screenshot(img, meta, cls_cat, cls_conf):
        return FinalResult(
            category="document",
            sub_category="screenshot",
            label="截图",
            confidence=max(cls_conf, 0.6),
            top3=[{"category": "document", "label": "截图", "confidence": 1.0}],
            source="heuristic",
        )

    # ================= 4. 夜景规则 =================
    if _night_hit(cls_out, scene_out, tone_out):
        return FinalResult(
            category="night_scene",
            label="夜景",
            confidence=0.85,
            top3=[{"category": "night_scene", "label": "夜景", "confidence": 0.9}],
            source="tone",
        )

    # ================= 5. 场景路覆盖（弱 cls 且 scene 置信度达标） =================
    if scene_out is not None and scene_out.ready and scene_out.confidence > 0:
        weak_cls = (
            cls_conf < config.SCENE_OVERRIDE_CONF
            or cls_cat in ("other", "landscape_nature", "architecture")
        )
        need_conf = _scene_takeover_conf(scene_out, cls_cat)
        if weak_cls and scene_out.confidence >= need_conf:
            # other 兜底：场景路合理置信接管
            if cls_cat == "other" and scene_out.category in ("landscape_nature", "cityscape", "architecture"):
                source_parts.append("scene")
                return FinalResult(
                    category=scene_out.category,
                    label=scene_out.label,
                    confidence=scene_out.confidence,
                    top3=[{"category": c, "label": lbl, "confidence": s} for c, s, lbl in cat_scores[:3]]
                    or [{"category": scene_out.category, "label": scene_out.label, "confidence": scene_out.confidence}],
                    source="+".join(source_parts),
                )
            # landscape/architecture 已被 cls 判定：仅当场景证据足够强才翻转
            if (
                cls_cat in ("landscape_nature", "architecture")
                and scene_out.category not in ("indoor", "other")
                and scene_out.category != cls_cat
            ):
                source_parts.append("scene")
                return FinalResult(
                    category=scene_out.category,
                    label=scene_out.label,
                    confidence=scene_out.confidence,
                    top3=[{"category": c, "label": lbl, "confidence": s} for c, s, lbl in cat_scores[:3]]
                    or [{"category": scene_out.category, "label": scene_out.label, "confidence": scene_out.confidence}],
                    source="+".join(source_parts),
                )
            if scene_out.category == "street" and det_out.ready and det_out.count >= 1:
                source_parts.append("scene")
                return FinalResult(
                    category="street",
                    label=scene_out.label,
                    confidence=scene_out.confidence,
                    top3=[{"category": "street", "label": scene_out.label, "confidence": scene_out.confidence}],
                    person_count=det_out.count,
                    source="+".join(source_parts),
                )

    # ================= 6. 花朵专家（cls=plant_flower 时触发） =================
    # 只做升级：高置信 → flower；低置信保持 plant_flower（fold → 花朵组），
    # 不降级为 plant（实测 daisy 波斯菊 0.198 被降级反而丢了对的类）。
    if cls_cat == "plant_flower" and flower_out is not None and flower_out.ready \
            and flower_out.flower_conf is not None:
        if flower_out.flower_conf >= config.FLOWER_CONF:
            cls_cat = "flower"
            cls_conf = flower_out.flower_conf
            cls_label = "花朵"
            source_parts.append("flower-expert")

    # ================= 7. 食物专家（通道就绪且触发条件满足时接管） =================
    if food_out is not None and food_out.ready and food_out.food_conf is not None \
            and food_out.food_conf >= config.FOOD_CONF and cls_cat != "animal":
        source_parts.append("food-expert")
        return FinalResult(
            category="food",
            label=f"食物·{cls_label}" if cls_label else "食物",
            confidence=food_out.food_conf,
            top3=[{"category": "food", "label": "食物", "confidence": food_out.food_conf}],
            source="+".join(source_parts),
        )

    # ================= 8. 兜底分类路 =================
    top3 = [{"category": c, "label": lbl, "confidence": s} for c, s, lbl in cat_scores[:3]]
    sub = cls_out.sub_category
    label = cls_label
    # 温和修正：other 与食物得分接近且食物证据强（≥0.3）时归食物（ImageNet pot 类边缘误判）
    cat_scores_d = dict((c, s) for c, s, _ in cat_scores)
    if (
        cls_cat == "other"
        and cat_scores_d.get("food", 0.0) >= 0.3
        and cat_scores_d["other"] - cat_scores_d.get("food", 0.0) < 0.15
    ):
        cls_cat = "food"
        cls_conf = cat_scores_d["food"]
        top3 = sorted(
            ({"category": c, "label": lbl, "confidence": s} for c, s, lbl in cat_scores[:3]),
            key=lambda t: t["confidence"],
            reverse=True,
        )
        source_parts.append("cls")
    if cls_cat == "animal":
        # 校准：子类置信度低时不强判 dog/cat（白猫→白狗 #31）
        if sub and cls_conf < config.ANIMAL_SUB_CONF_MIN:
            sub = "其他动物"
        sub = sub or "其他动物"
        label = f"{config.ANIMAL_SUB_DESC.get(sub, sub)}·{cls_label}" if cls_label else sub
    elif cls_cat == "landscape_nature":
        label = f"自然·{cls_label}" if cls_label else "自然风景"
    elif cls_cat == "document":
        label = f"文档·{cls_label}" if cls_label else "文档"
    elif cls_cat == "flower":
        label = f"花朵·{cls_label}" if cls_label else "花朵"
    elif cls_cat == "plant_flower":
        # 专家低置信时保持原类；sub 区分 花/植物（mapping.plant_flower_sub）
        sub = sub or "植物花卉"
        label = f"植物花卉·{sub}" if cls_label else sub
    return FinalResult(
        category=cls_cat,
        sub_category=sub,
        label=label,
        confidence=cls_conf,
        top3=top3,
        source="+".join(source_parts) or "cls",
    )
