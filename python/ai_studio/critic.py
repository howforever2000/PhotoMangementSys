"""本地零模型客观指标 critic（FEAT-168）

为什么需要它：本地网关没有 `/v1/chat/completions`（`knowledge/local-ai-gateway.md:28`），
纯本地部署时视觉 LLM critic 根本跑不了；而「中间过程可反思」在没有多模态模型的
机器上也要成立，所以先落地一组**零模型、零成本、可每步跑**的客观指标。

只做「一张图 → 客观读数 + 是否异常」，不做主观审美判断、不调用任何模型：
  - 过曝 / 死黑：高光、暗部像素占比
  - 噪声：高频能量（灰度拉普拉斯方差，缩到长边 512 上算，避免大图开销）
  - 锐度：同一张高频能量，与输入对照判断「是不是被磨糊了」
  - 肤色偏色：肤色候选区的 R/G/B 均值比

对照输入图的相对变化才是信号：单看结果图的绝对值无法区分「原图本来就暗」和
「这一步把图压暗了」。所以 `compare(source, result)` 同时返回两侧读数与 issues。
"""
from __future__ import annotations

from typing import Any, Dict, List, Optional

import numpy as np
from PIL import Image

# 采样长边：客观统计不需要全分辨率
_SAMPLE_LONG = 512
# 高光 / 暗部阈值（8bit sRGB）
_HI = 250
_LO = 5
# 截断恶化容忍度（百分点）。单通道更严：任一通道到顶即该区域层次不可恢复。
_CLIP_DELTA = 0.01
_CLIP_ANY_DELTA = 0.005
# 肤色 R/G 偏移容忍度。自然饱和度会把暖肤色推高约 0.18，阈值必须在其之上。
_SKIN_RG_DELTA = 0.25


def _sample(img: Image.Image) -> np.ndarray:
    im = img.convert("RGB")
    w, h = im.size
    m = max(w, h)
    if m > _SAMPLE_LONG:
        s = _SAMPLE_LONG / float(m)
        im = im.resize((max(1, int(w * s)), max(1, int(h * s))), Image.BILINEAR)
    return np.asarray(im, dtype=np.float32)


def metrics(img: Image.Image) -> Dict[str, float]:
    """单图客观读数。全是 0~1 或可直接比较的标量，不依赖任何模型。"""
    a = _sample(img)
    gray = a.mean(axis=2)
    total = float(gray.size)
    # 高频能量：Laplacian 响应绝对值均值（噪声与锐度共用一个量，靠对照判别）
    lap = (
        -4.0 * gray[1:-1, 1:-1]
        + gray[:-2, 1:-1]
        + gray[2:, 1:-1]
        + gray[1:-1, :-2]
        + gray[1:-1, 2:]
    )
    high_freq = float(np.abs(lap).mean()) if lap.size else 0.0
    # 肤色候选：R 明显大于 G、G 略大于 B，且亮度不极端
    r, g, b = a[..., 0], a[..., 1], a[..., 2]
    skin = (r > g + 12) & (g > b + 4) & (r > 95) & (gray > 60) & (gray < 245)
    if skin.any():
        skin_rg = float((r[skin] / np.maximum(g[skin], 1)).mean())
    else:
        skin_rg = 0.0
    # 单通道截断：任一通道顶到 250 即失去该通道层次。只看灰度均值会漏掉
    # 「天空只剩蓝、其余两通道已到顶」这类最常见的高光损失（BUG-2026-1117-002）。
    clip_any = float((a.max(axis=2) >= _HI).sum()) / total
    return {
        "highlight": float((gray >= _HI).sum()) / total,
        "clipAny": clip_any,
        "shadow": float((gray <= _LO).sum()) / total,
        "highFreq": high_freq,
        "skinRG": skin_rg,
        "luma": float(gray.mean()),
    }


def compare(source: Image.Image, result: Image.Image) -> Dict[str, Any]:
    """原图 ↔ 结果对照，返回 `{verdict, score, dims, issues}`。

    verdict 只用 pass | fail | unknown 三态：指标只能抓「明显退化」，
    抓不到的（构图、身份、审美）一律 unknown，绝不冒充通过。
    """
    s, r = metrics(source), metrics(result)
    issues: List[str] = []

    # 1) 过曝 / 死黑恶化。阈值 1 个百分点：实测出片经常在 0.1%~7% 之间恶化
    #    （DSC01493 单通道截断 0.015% → 26.4%），旧阈值 8% 永远抓不到（BUG-2026-1117-002）。
    if r["highlight"] - s["highlight"] > _CLIP_DELTA:
        issues.append(f"高光溢出增加（{s['highlight']:.1%} → {r['highlight']:.1%}）")
    if r["clipAny"] - s["clipAny"] > _CLIP_ANY_DELTA:
        issues.append(
            f"单通道截断增加（{s['clipAny']:.1%} → {r['clipAny']:.1%}，高光层次丢失）"
        )
    if r["shadow"] - s["shadow"] > _CLIP_DELTA:
        issues.append(f"暗部死黑增加（{s['shadow']:.1%} → {r['shadow']:.1%}）")

    # 2) 细节被抹掉：高频能量掉到原图的 55% 以下（磨皮过火 / 过度降噪）
    if s["highFreq"] > 2.0 and r["highFreq"] < s["highFreq"] * 0.55:
        issues.append(f"细节明显损失（高频 {s['highFreq']:.1f} → {r['highFreq']:.1f}）")

    # 3) 噪声暴涨：高频能量涨到原图 2.2 倍以上（颗粒/伪影过量）
    if s["highFreq"] > 0.5 and r["highFreq"] > s["highFreq"] * 2.2:
        issues.append(f"噪声/伪影增加（高频 {s['highFreq']:.1f} → {r['highFreq']:.1f}）")

    # 4) 肤色偏色：只在两侧都检到足够肤色时比较，避免误判风景照。
    #    阈值 0.25：normal 档 saturation 1.08 + vibrance 0.20 会把暖肤色自然推高
    #    约 0.18（实测 1.36 → 1.54），这是「自然饱和度」的正常效果而非偏色。
    #    旧阈值 0.18 恰好卡在正常波动上，导致每张人像都误报（BUG-2026-1117-007）。
    if s["skinRG"] > 0 and r["skinRG"] > 0 and abs(r["skinRG"] - s["skinRG"]) > _SKIN_RG_DELTA:
        issues.append(f"肤色偏移（R/G {s['skinRG']:.2f} → {r['skinRG']:.2f}）")

    # 5) 整体亮度漂移过大
    if abs(r["luma"] - s["luma"]) > 42:
        issues.append(f"整体亮度漂移（{s['luma']:.0f} → {r['luma']:.0f}）")

    return {
        "verdict": "fail" if issues else "unknown",
        "score": round(1.0 - min(1.0, len(issues) * 0.2), 2),
        "dims": {
            "highlight": round(r["highlight"], 4),
            "shadow": round(r["shadow"], 4),
            "highFreq": round(r["highFreq"], 2),
            "luma": round(r["luma"], 1),
        },
        "issues": issues,
    }


# ---------------- 结构保全校验（FEAT-169） ----------------
#
# 用户规则：「滤镜迁移不能改变每个物体的形状，包括人物、主体、背景」。提示词只是软
# 约束，网关照样会整体重绘，所以这里补一层**零模型、零成本**的硬验收：
#   - 尺寸 / 宽高比：改构图、加边框、裁切会在这一项立刻暴露；
#   - 结构一致性：原图与结果各自缩到同一小网格、转灰度、去均值后做归一化互相关。
#     只改颜色与影调时，亮度结构基本保持 → 相关仍很高；重绘、挪动主体、换构图
#     会让相关明显掉落；
#   - 人脸框宽高比：人脸可用时比对检测框比例 —— 「瘦脸」「放大眼睛」这类改形状
#     在几何指标上最敏感的就是脸框被压窄。
#
# 阈值刻意保守（宁可漏报也不误报）：误报会让用户看到一堆没必要的告警，
# 而漏报还有提示词那一层兜着。

_STRUCT_GRID = 48          # 结构比对用的网格边长（48×48 = 2304 采样点足够稳）
_STRUCT_MIN_CORR = 0.55    # 归一化互相关下限（低于此值认为结构被明显改动）
_ASPECT_TOLERANCE = 0.02   # 尺寸宽高比允许的相对误差


def _grid_gray(img: Image.Image, size: int = _STRUCT_GRID) -> np.ndarray:
    """缩到 size×size 灰度网格并去均值：只保留结构，抹掉整体亮度差异。"""
    g = img.convert("L").resize((size, size), Image.BILINEAR)
    a = np.asarray(g, dtype=np.float32)
    return a - float(a.mean())


def _corr(a: np.ndarray, b: np.ndarray) -> float:
    """归一化互相关（-1~1）。任一图无结构（全平）时返回 1.0，避免误报。"""
    denom = float(np.sqrt((a * a).sum() * (b * b).sum()))
    if denom <= 1e-6:
        return 1.0
    return float((a * b).sum() / denom)


def structure_compare(source: Image.Image, result: Image.Image) -> Dict[str, Any]:
    """结构保全验收：`{ok, score, dims, issues}`。

    `ok=False` 表示**几何被改动的证据成立**；调用方据此决定告警还是回退。
    本函数不调用任何模型、不写盘，可在每一张、每一步之后跑。
    """
    issues: List[str] = []
    dims: Dict[str, float] = {}

    sw, sh = source.size
    rw, rh = result.size
    dims["srcAspect"] = round(sw / float(sh), 4) if sh else 0.0
    dims["resAspect"] = round(rw / float(rh), 4) if rh else 0.0

    # 1) 宽高比：改构图 / 裁切 / 加边框的第一信号
    if sh and rh:
        s_ar, r_ar = sw / float(sh), rw / float(rh)
        rel = abs(r_ar - s_ar) / max(s_ar, 1e-6)
        dims["aspectDiff"] = round(rel, 4)
        if rel > _ASPECT_TOLERANCE:
            issues.append(f"画幅比例被改动（{s_ar:.3f} → {r_ar:.3f}）")

    # 2) 结构一致性：只改颜色时灰度结构应基本重合
    corr = _corr(_grid_gray(source), _grid_gray(result))
    dims["structureCorr"] = round(corr, 4)
    if corr < _STRUCT_MIN_CORR:
        issues.append(f"画面结构发生大幅位移或被重绘（结构相关度 {corr:.2f}）")

    # 3) 人脸框宽高比：有无「瘦脸 / 放大眼睛」这类改形状的显式证据
    face = _face_aspect_delta(source, result)
    if face is not None:
        dims["faceAspectDiff"] = round(face, 4)
        if face > 0.10:
            issues.append(f"人脸比例被改动（宽高比变化 {face:.0%}）")

    return {
        "ok": not issues,
        "score": round(max(0.0, 1.0 - len(issues) * 0.35), 2),
        "dims": dims,
        "issues": issues,
    }


def _face_aspect_delta(source: Image.Image, result: Image.Image) -> Optional[float]:
    """两侧都能检到人脸时，返回最大的人脸框宽高比相对变化；否则 None。

    人脸检测器是惰性导入的（onnxruntime 只在真用到时才加载），
    模型缺失 / 检测异常一律返回 None —— 结构校验不能因为可选依赖而失败。
    """
    try:
        from . import localmodels

        if not localmodels.face_det_ready():
            return None
        a = localmodels.detect_faces(source)
        b = localmodels.detect_faces(result)
    except Exception:  # noqa: BLE001 —— 校验器必须是「绝不抛」的
        return None
    if not a or not b:
        return None

    def _ratio(f) -> float:
        x1, y1, x2, y2 = f["bbox"]
        h = max(1.0, float(y2 - y1))
        return float(x2 - x1) / h

    # 取面积最大的脸做对照：小脸的比例读数噪声大，不适合当判据
    def _biggest(faces):
        return max(faces, key=lambda f: (f["bbox"][2] - f["bbox"][0]) * (f["bbox"][3] - f["bbox"][1]))

    ra, rb = _ratio(_biggest(a)), _ratio(_biggest(b))
    return abs(rb - ra) / max(ra, 1e-6)


__all__ = ["metrics", "compare", "structure_compare"]
