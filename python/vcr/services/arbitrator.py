"""服务层：规则仲裁器（v5 降级版，已移除分类模型/场景/专家通道）

输入只剩三条规则通道 + 元信息：
  det   人物检测（YOLOv8n-det）→ 人物特写 / 合影 / 扫街
  tone  影调自算              → 夜景
  ocr   文档检测（PaddleOCR）  → 文档
  meta  格式/EXIF/尺寸         → 截图启发式

产出 5 个系统类别之一（与宿主 category.rs 的 builtin slug 一致）：
  portrait / street / night_scene / document / other

优先级（保持 F20/F21 实测校准过的顺序）：
  1. 人物规则（最高优先；taxonomy 决策 Q3：证件照归特写可接受）
  2. OCR 强证据（文字框面积达标 + ≥2 框）→ document/paper
  3. 截图启发式（PNG + 无 EXIF + 屏幕比例 + 最小尺寸）→ document/screenshot
  4. 影调夜景（avg_luma < NIGHT_LUMA）→ night_scene
  5. 兜底 other

图像内容分类（动物/美食/花卉/风景/建筑/车辆…）不再由本服务产出，
改由宿主「语义分类」（Chinese-CLIP 关键词匹配）承担。
"""
from dataclasses import dataclass, field

from .. import config


@dataclass
class FinalResult:
    category: str
    sub_category: str = ""
    label: str = ""
    confidence: float = 0.0
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


def is_screenshot(img, meta: dict, src_size: tuple[int, int] | None = None) -> bool:
    """截图启发式：PNG 格式 + 无 EXIF + 分辨率符合屏幕比例 + 短边达标。

    v5：分类模型下线后不再有「弱分类证据」这一条件；PNG + 无 EXIF + 屏幕比例
    本身已是强特征（相机/手机导出的 PNG 极少同时满足三者）。

    src_size：**真实原图**尺寸。P-DEC1 引入解码降采样后，img.size 可能小于
    原图（draft 只对 JPEG 生效，所以 PNG 截图实际不受影响，但口径要统一）。
    """
    fmt = (meta.get("format") or "").upper()
    if fmt != "PNG":
        return False
    if meta.get("has_exif", True):
        return False
    w, h = src_size if src_size else img.size
    if min(w, h) < config.SCREEN_MIN_SIZE:
        return False
    return _near_screen_ratio(w, h)


def _night_hit(tone_out) -> bool:
    """夜景判定（v5 降级版）：影调主导 —— avg_luma < NIGHT_LUMA(45)。

    旧链路在 45~60 档还需要 Places365 语义佐证，分类模型下线后该证据不存在，
    故收敛到 45（对齐旧链路"luma<45 且 cls 置信度<0.5"的实际行为）。
    """
    return tone_out is not None and tone_out.ready and tone_out.avg_luma is not None \
        and tone_out.avg_luma < config.NIGHT_LUMA


def person_verdict(det_out) -> str | None:
    """人物规则结论（最高优先）：'street' / 'closeup' / 'group' / None。

    抽成独立函数的目的：**OCR 通道要做前置门**（P-OPT3）—— OCR 在仲裁里排在
    人物规则之后，一旦人物规则命中，OCR 结果永远不会被采纳，那一次串行 GPU 推理
    就是纯浪费。前置门必须与仲裁一模一样，否则会出现「门说不命中、仲裁却命中」
    或反之的隐形偏差，所以两边共用本函数，改规则只改这一处。

    实测依据（245K + DML，global lock 已开）：P=6 跑 24 张图共 55 次加锁，
    持锁时间占墙钟 **99.0%**；ocr 每张无条件跑，占了其中约 44% 的加锁次数。
    """
    if not det_out.ready or det_out.count <= 0:
        return None
    n, max_area = det_out.count, det_out.max_area_ratio
    # 校准（2026-08-13 §7.3.1）：密集车流中 det 会把车头/车窗误检为人，
    # 且假框置信度 0.48~0.73 高于提高 PERSON_CONF_MIN 的任何实用取值；
    # 车辆 ≥ VEHICLE_HEAVY_N 且人框全为小框时跳过 street。
    heavy_traffic = (
        getattr(det_out, "vehicle_count", 0) >= config.VEHICLE_HEAVY_N
        and max_area < config.VEHICLE_PERSON_AREA_MAX
    )
    if n >= config.STREET_PERSON_N and max_area < config.STREET_MAX_AREA and not heavy_traffic:
        return "street"
    if max_area >= config.PORTRAIT_AREA:
        return "closeup"
    if n >= 2 and max_area >= config.GROUP_AREA:
        return "group"
    return None


def person_rule_hits(det_out) -> bool:
    """人物规则是否会**直接给出结论**（供 OCR 前置门使用）。"""
    return person_verdict(det_out) is not None


def arbitrate(img, det_out, meta: dict, face_hits: list[dict],
              tone_out=None, ocr_out=None,
              src_size: tuple[int, int] | None = None) -> FinalResult:
    """src_size：真实原图尺寸（P-DEC1）—— 不传时回落 img.size（旧行为）。"""
    source_parts: list[str] = []

    # ================= 1. 人物规则（最高优先） =================
    if det_out.ready and det_out.count > 0:
        n, max_area = det_out.count, det_out.max_area_ratio
        person_ids = [h["person_id"] for h in face_hits]
        verdict = person_verdict(det_out)
        if verdict == "street":
            return FinalResult(
                category="street",
                sub_category="street",
                label=f"扫街·{n}人",
                confidence=det_out.max_conf,
                person_ids=person_ids,
                person_count=n,
                source="det",
            )
        if verdict == "closeup":
            label = f"人物特写·{len(person_ids) or n}人"
            return FinalResult(
                category="portrait",
                sub_category="closeup",
                label=label,
                confidence=det_out.max_conf,
                person_ids=person_ids,
                person_count=n,
                source="det+face" if person_ids else "det",
            )
        if verdict == "group":
            label = f"合影·{len(person_ids) or n}人"
            return FinalResult(
                category="portrait",
                sub_category="group",
                label=label,
                confidence=det_out.max_conf,
                person_ids=person_ids,
                person_count=n,
                source="det+face" if person_ids else "det",
            )
        # 单人小框 → 路人，不覆盖分类
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
            source="ocr",
        )

    # ================= 3. 截图启发式 → document =================
    if is_screenshot(img, meta, src_size):
        return FinalResult(
            category="document",
            sub_category="screenshot",
            label="截图",
            confidence=0.8,
            source="heuristic",
        )

    # ================= 4. 影调夜景 =================
    if _night_hit(tone_out):
        return FinalResult(
            category="night_scene",
            sub_category="night",
            label="夜景",
            confidence=0.85,
            source="tone",
        )

    # ================= 5. 兜底 =================
    return FinalResult(
        category="other",
        sub_category="other",
        label="其他",
        confidence=0.0,
        source="+".join(source_parts) or "none",
    )
