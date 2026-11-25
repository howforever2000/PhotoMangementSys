"""胶片风格注册表：内置 LUT 风格 + 参数化渲染风格（单一真相源）。

分层（与「方案_参数化自适应胶片模拟」§4 / §8 对应）：
  LUT 风格     config.LUTS 里的 7 套 .cube，**行为一个字节都不动**；
  参数化风格   本文件的 RECIPES，纯标量参数，由 film_engine 渲染。

冲突策略（用户明确要求「新给的滤镜有重合的话保留老的」）：
  场景化 4 款里，Velvia / Classic Chrome 与内置 LUT 同名同 id，**保留老的那套**
  （id 仍指向 .cube，默认引擎仍是 lut）；只有不与老风格重合的
  Kodak Portra 400 / Kodak 2383 作为**新条目**加进目录，且只有 film 引擎。
  参数化引擎里仍保留这两套的 faithful 配方，供显式 `engine=film` 时使用。

为什么 id 不另起前缀：老 id 是已落盘的产物名（`_胶片_velvia`）与配方里写死的值，
改名会让历史产物被当成「未知风格」；重合时保持 id 不变、默认引擎不变，才是兼容。
"""
from __future__ import annotations

from typing import Any, Dict, List, Optional

from . import config

# 新风格（目录里新增的条目）：没有 .cube，只能走 film 引擎
NEW_STYLES: tuple[str, ...] = ("portra400", "kodak_2383")

# 场景标签：给 UI 与「按场景推荐」用；老风格也标注它们对应的场景位
SCENES: Dict[str, Dict[str, str]] = {
    "portra400": {"scene": "人像 / 自拍 / 日常", "why": "肤色最稳：低反差、暖高光、天空转灰蓝"},
    "velvia": {"scene": "山 / 海 / 森林 / 旅行", "why": "蓝绿爆发力最强（老版 .cube，保持原样）"},
    "classic_chrome": {"scene": "扫街 / 城市 / 建筑 / 纪实", "why": "低饱和高反差、冷静叙事（老版 .cube，保持原样）"},
    "kodak_2383": {"scene": "复古 / 怀旧 / 电影氛围", "why": "青橙分离 + 抬黑，印片质感"},
    "provia": {"scene": "通用基准", "why": "中性、轻微提亮，适合大多数日光片"},
    "astia": {"scene": "柔和人像 / 阴天", "why": "低对比、奶雾黑位，肤色不脏"},
    "classic_neg": {"scene": "街拍 / 逆光", "why": "青绿阴影 + 暖中调，负片味"},
    "eterna": {"scene": "电影 / 夜景", "why": "抬黑、最寡淡，光比大的场景最稳"},
    "nostalgic_neg": {"scene": "怀旧 / 黄昏", "why": "全调琥珀、强光晕"},
}

# ---------------------------------------------------------------------------
# 参数化配方（字段与 film_engine / lab 的 adaptive_film 完全一致）
#
# 前 7 套 = 内置 LUT 的 faithful 复刻配方（指纹来自 lut_fingerprint.json）。
# 后 2 套 = 场景化新增（与 Velvia / Classic Chrome 重合的两套**故意不在这里重复定义**，
#           避免「同名两版」在同一 id 下打架）。
# ---------------------------------------------------------------------------
RECIPES: Dict[str, Dict[str, Any]] = {
    "provia": {
        "name": "Provia 标准",
        "gamma": 2.56, "knee": 0.805, "shoulder": 1.70, "fog": 0.004,
        "crosstalk": 0.026, "ch_gamma": (1.000, 1.005, 1.015), "ch_pivot": (0.00, 0.00, 0.01),
        "sat": 1.04, "sat_hi": 0.45, "vibrance": 0.12, "s_base": 1.30,
        "chroma": [(0, 1.04), (60, 1.04), (120, 1.10), (180, 1.00), (240, 0.96), (300, 1.00)],
        "hue_shift": [(15, 3), (55, 1), (90, 2), (130, 5), (205, -4), (250, -8), (290, -7), (335, -1)],
        "hue_strength": 0.90,
        "shadow_tint": (-0.004, 0.002, -0.003), "high_tint": (0.003, 0.001, -0.001),
        "halation": 0.08, "grain": 0.052, "pivot": 0.16, "ev_bias": 0.04,
    },
    "velvia": {
        "name": "Velvia 浓郁",
        "gamma": 2.60, "knee": 0.800, "shoulder": 1.95, "fog": 0.002,
        "crosstalk": 0.022, "ch_gamma": (1.000, 1.012, 1.035), "ch_pivot": (0.00, 0.00, 0.02),
        "sat": 1.30, "sat_hi": 0.26, "vibrance": 0.00, "s_base": 1.52,
        "chroma": [(0, 1.14), (60, 1.18), (120, 1.34), (180, 0.95), (240, 0.90), (300, 0.98)],
        "hue_shift": [(15, 9), (55, 3), (90, 4), (130, 10), (205, -8), (250, -16), (290, -14), (335, -3)],
        "hue_strength": 1.20,
        "shadow_tint": (-0.005, 0.002, -0.001), "high_tint": (0.006, 0.001, -0.002),
        "halation": 0.10, "grain": 0.062, "pivot": 0.16, "ev_bias": 0.06,
    },
    "astia": {
        "name": "Astia 柔和",
        "gamma": 2.84, "knee": 0.775, "shoulder": 1.35, "fog": 0.018,
        "crosstalk": 0.030, "ch_gamma": (1.010, 1.000, 0.996), "ch_pivot": (0.02, 0.00, 0.00),
        "sat": 0.90, "sat_hi": 0.52, "vibrance": 0.20, "s_base": 1.16,
        "chroma": [(0, 1.00), (60, 1.02), (120, 1.02), (180, 0.98), (240, 0.98), (300, 1.00)],
        "hue_shift": [(15, 3), (55, 1), (90, 2), (130, 4), (205, -5), (250, -10), (290, -10), (335, -3)],
        "hue_strength": 0.80,
        "shadow_tint": (-0.003, 0.001, -0.003), "high_tint": (0.003, 0.002, 0.002),
        "halation": 0.14, "grain": 0.042, "pivot": 0.16, "ev_bias": 0.02,
    },
    "classic_chrome": {
        "name": "Classic Chrome",
        "gamma": 2.52, "knee": 0.810, "shoulder": 1.75, "fog": 0.005,
        "crosstalk": 0.036, "ch_gamma": (0.950, 1.000, 1.060), "ch_pivot": (0.00, 0.00, 0.01),
        "sat": 0.88, "sat_hi": 0.58, "vibrance": 0.22, "s_base": 1.34,
        "chroma": [(0, 1.00), (60, 0.94), (120, 0.86), (180, 0.78), (240, 0.74), (300, 0.82)],
        "hue_shift": [(15, 8), (55, -5), (90, -2), (130, 7), (205, -7), (250, -14), (290, -15), (335, -4)],
        "hue_strength": 1.20,
        "shadow_tint": (-0.008, 0.002, 0.005), "high_tint": (0.015, 0.009, -0.009),
        "halation": 0.06, "grain": 0.055, "pivot": 0.16, "ev_bias": 0.04,
    },
    "classic_neg": {
        "name": "Classic Negative",
        "gamma": 2.80, "knee": 0.785, "shoulder": 1.55, "fog": 0.026,
        "crosstalk": 0.046, "ch_gamma": (1.025, 1.012, 0.960), "ch_pivot": (0.04, -0.01, -0.05),
        "sat": 0.78, "sat_hi": 0.40, "vibrance": 0.18, "s_base": 1.18,
        "chroma": [(0, 0.90), (60, 0.88), (120, 0.94), (180, 0.82), (240, 0.76), (300, 0.86)],
        "hue_shift": [(15, 10), (55, -4), (90, -6), (130, 16), (205, -6), (250, -7), (290, -8), (335, 3)],
        "hue_strength": 1.15,
        "shadow_tint": (-0.018, 0.009, -0.006), "high_tint": (0.015, -0.002, -0.009),
        "halation": 0.15, "grain": 0.056, "pivot": 0.16, "ev_bias": 0.0,
    },
    "eterna": {
        "name": "Eterna 电影",
        "gamma": 2.88, "knee": 0.765, "shoulder": 1.24, "fog": 0.042,
        "crosstalk": 0.044, "ch_gamma": (1.012, 1.000, 0.975), "ch_pivot": (0.03, 0.00, -0.03),
        "sat": 0.70, "sat_hi": 0.58, "vibrance": 0.06, "s_base": 1.12,
        "chroma": [(0, 1.00), (60, 0.94), (120, 0.86), (180, 0.82), (240, 0.74), (300, 0.86)],
        "hue_shift": [(15, 4), (55, 2), (90, 3), (130, 5), (205, -8), (250, -16), (290, -18), (335, -6)],
        "hue_strength": 1.10,
        "shadow_tint": (-0.005, 0.005, 0.003), "high_tint": (0.007, 0.002, -0.004),
        "halation": 0.13, "grain": 0.050, "pivot": 0.16, "ev_bias": 0.0,
    },
    "nostalgic_neg": {
        "name": "Nostalgic Neg",
        "gamma": 2.64, "knee": 0.795, "shoulder": 1.55, "fog": 0.018,
        "crosstalk": 0.038, "ch_gamma": (1.035, 1.000, 0.960), "ch_pivot": (0.06, -0.01, -0.05),
        "sat": 0.96, "sat_hi": 0.45, "vibrance": 0.12, "s_base": 1.28,
        "chroma": [(0, 1.06), (60, 1.00), (120, 0.94), (180, 0.84), (240, 0.80), (300, 0.88)],
        "hue_shift": [(15, 14), (55, -1), (90, -2), (130, 4), (205, -6), (250, -13), (290, -17), (335, 0)],
        "hue_strength": 1.20,
        "shadow_tint": (0.008, -0.001, -0.011), "high_tint": (0.018, -0.002, -0.026),
        "halation": 0.17, "grain": 0.060, "pivot": 0.16, "ev_bias": 0.0,
    },
    # ---------------- 场景化新增（没有同名 LUT，不与老风格冲突）
    "portra400": {
        "name": "Kodak Portra 400（人像）",
        "gamma": 2.80, "knee": 0.775, "shoulder": 1.32, "fog": 0.022,
        "crosstalk": 0.034, "ch_gamma": (1.016, 1.000, 0.980), "ch_pivot": (0.02, 0.00, -0.02),
        "sat": 0.90, "sat_hi": 0.44, "vibrance": 0.22, "s_base": 1.12,
        "chroma": [(0, 1.02), (30, 1.08), (60, 1.04), (120, 0.92), (180, 0.90), (240, 0.88), (300, 0.94)],
        "hue_shift": [(15, 4), (40, 2), (60, 0), (90, -2), (130, 2), (205, -4), (250, -8), (290, -6), (335, 0)],
        "hue_strength": 0.70,
        "shadow_tint": (0.002, 0.005, 0.003), "high_tint": (0.026, 0.007, -0.017),
        "halation": 0.16, "grain": 0.045, "pivot": 0.16, "ev_bias": 0.06, "skin_protect": 0.55,
    },
    "kodak_2383": {
        "name": "Kodak 2383（电影）",
        "gamma": 2.58, "knee": 0.800, "shoulder": 2.05, "fog": 0.018,
        "crosstalk": 0.052, "ch_gamma": (1.020, 1.000, 0.960), "ch_pivot": (0.03, 0.00, -0.03),
        "sat": 0.84, "sat_hi": 0.50, "vibrance": 0.10, "s_base": 1.34,
        "chroma": [(0, 1.04), (60, 1.00), (120, 0.90), (180, 0.88), (240, 0.86), (300, 0.94)],
        "hue_shift": [(15, 6), (60, -2), (90, 2), (130, 4), (205, -6), (250, -9), (290, -7), (335, 0)],
        "hue_strength": 0.90,
        "shadow_tint": (-0.018, 0.018, 0.034), "high_tint": (0.048, 0.012, -0.034),
        "halation": 0.22, "grain": 0.052, "pivot": 0.16, "ev_bias": 0.0,
    },
}

# L2 后调层（「滤镜 + 后调」的后调）：在风格渲染结果上叠的分色调 / 降饱和 / 对比 / 抬黑
GRADES: Dict[str, Dict[str, Any]] = {
    "teal_orange": {
        "name": "青橙",
        "shadow": (0.000, 0.010, 0.032), "mid": (0.004, 0.002, 0.004),
        "high": (0.048, 0.006, -0.022), "sat": 0.92, "contrast": 1.12, "fog": 0.000,
    },
    "vintage_amber": {
        "name": "复古琥珀",
        "shadow": (0.020, 0.008, -0.020), "mid": (0.018, 0.004, -0.012),
        "high": (0.035, 0.010, -0.030), "sat": 0.84, "contrast": 1.06, "fog": 0.028,
    },
    "cold_cyan": {
        "name": "冷青灰",
        "shadow": (-0.016, 0.004, 0.026), "mid": (-0.008, 0.002, 0.014),
        "high": (-0.010, 0.002, 0.018), "sat": 0.72, "contrast": 1.22, "fog": 0.010,
    },
}


def lut_entry(style_id: str) -> Optional[dict]:
    return next((l for l in config.LUTS if l["id"] == style_id), None)


def has_lut(style_id: str) -> bool:
    return lut_entry(style_id) is not None


def known(style_id: str) -> bool:
    return style_id in RECIPES


def resolve(style_id: str, engine: str = "auto") -> tuple[str, dict]:
    """把 (风格 id, 引擎) 解析成 (实际引擎, 配方)。

    规则：
      - auto：有 LUT 走 lut（**老行为优先**），没有 LUT 才走 film；
      - lut ：必须真有 .cube，否则明确报错（不悄悄换引擎）；
      - film：必须有配方（9 套都有），老 id 显式走 film = 用户主动选参数化版。
    """
    sid = (style_id or "provia").strip() or "provia"
    if not known(sid):
        raise ValueError(f"未知的胶片风格: {sid}")
    eng = (engine or "auto").strip().lower() or "auto"
    if eng == "auto":
        eng = "lut" if has_lut(sid) else "film"
    if eng == "lut":
        if not has_lut(sid):
            raise ValueError(
                f"「{RECIPES[sid]['name']}」没有对应的 .cube，只能用参数化引擎（engine=film）"
            )
        return "lut", RECIPES[sid]
    if eng == "film":
        return "film", RECIPES[sid]
    raise ValueError(f"未知的引擎: {engine}（可选 auto / lut / film）")


def catalog() -> List[dict]:
    """给 /local/models 的风格清单：老的 7 套原样在前，新增 2 套排在其后。"""
    out: List[dict] = []
    for lut in config.LUTS:
        sid = lut["id"]
        out.append(
            {
                "id": sid,
                "name": lut["name"],
                "file": lut["file"],
                "exists": True,
                "engine": "lut",
                "hasLut": True,
                "film": sid in RECIPES,
                "scene": SCENES.get(sid, {}).get("scene", ""),
                "why": SCENES.get(sid, {}).get("why", ""),
                "new": False,
            }
        )
    for sid in NEW_STYLES:
        out.append(
            {
                "id": sid,
                "name": RECIPES[sid]["name"],
                "file": "",
                "exists": True,
                "engine": "film",
                "hasLut": False,
                "film": True,
                "scene": SCENES.get(sid, {}).get("scene", ""),
                "why": SCENES.get(sid, {}).get("why", ""),
                "new": True,
            }
        )
    return out
