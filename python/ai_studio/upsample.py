"""上采样（超分）编排（FEAT-197）

「模型理解」路线的收尾增强：把 1K 出图再放大一次。产品口径来自实测取舍：

  - **默认不勾**：上采样是最贵的一步（每张多一次云端调用），而且很多照片在 1K
    输出上已经够用，不该默认替用户花钱。
  - **默认 2×**：2× 之后画面明显更锐，再加就没那么多可见收益了。
  - **4× = 2× 跑两遍**：网关没有倍率参数（`scale` 只进提示词），一次写 4× 得到的
    是「模型自己猜的 4 倍」，两次 2× 则是「先补细节、再补细节」，实测细节更实。
    这不是可选项而是实现选择，所以放在这里一次性说清，不在界面上暴露「跑几遍」。

本模块只做「计划形态」的编排（追加 / 替换超分步），不碰图像与 provider ——
真正的放大在 `ops.op_upscale`，执行顺序在 `pipeline.execute_plan`。
"""
from __future__ import annotations

from typing import Any, Dict, List

# 可选的输出倍率（与前端 `UPSCALE_SCALES` 同口径）
UPSAMPLE_SCALES = (2, 4)
# 每次调用固定 2×：4× 由两次 2× 叠加（见模块头注）
PASS_SCALE = 2
SUPERRES_OPS = frozenset({"upscale", "local_upscale"})


class UpsampleError(ValueError):
    """上采样参数非法（接口层/入口层转成可读中文后上报）。"""


def normalize_upsample_scale(raw: Any) -> int:
    """任意来源的倍率 → 2 或 4；缺省/非法一律 2（界面默认档）。"""
    try:
        v = int(raw)
    except (TypeError, ValueError):
        return 2
    return 4 if v == 4 else 2


def passes_for(scale: int) -> int:
    """倍率 → 需要跑几遍 2×（2→1，4→2）。"""
    return 2 if normalize_upsample_scale(scale) == 4 else 1


def plan_with_upsample(plan: Dict[str, Any], enabled: bool, scale: Any = 2) -> Dict[str, Any]:
    """按上采样选项改写计划：摘掉规划器自带的超分步，按需补上 N 遍 2×。

    为什么要摘掉规划器自带的那一步：模型偶尔会自己排 `upscale`，此时再叠加用户
    勾的两遍就是 3 个超分步（被 `validate_plan` 拒）；而用户勾的是**显式意图**，
    优先于模型的猜测。不勾时也一并摘掉 —— 「没勾上采样」就该没有超分步。
    """
    steps: List[Dict[str, Any]] = [
        st for st in (plan.get("steps") or []) if st.get("op") not in SUPERRES_OPS
    ]
    if enabled:
        passes = passes_for(scale)
        for _ in range(passes):
            steps.append({"op": "upscale", "params": {"scale": PASS_SCALE}})
    return {**plan, "steps": steps}


def upsample_meta(enabled: bool, scale: Any = 2) -> Dict[str, Any]:
    """写进 job meta 的口径说明（前端据此显示「上采样 2× / 4×（两遍 2×）」）。"""
    s = normalize_upsample_scale(scale)
    return {
        "upsample": bool(enabled),
        "upsampleScale": s if enabled else 0,
        "upsamplePasses": passes_for(s) if enabled else 0,
    }


__all__ = [
    "UPSAMPLE_SCALES",
    "PASS_SCALE",
    "SUPERRES_OPS",
    "UpsampleError",
    "normalize_upsample_scale",
    "passes_for",
    "plan_with_upsample",
    "upsample_meta",
]
