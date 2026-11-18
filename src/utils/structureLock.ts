/**
 * 结构保全中文（FEAT-169 → FEAT-171）
 *
 * 云端图生图会整幅重绘，形状纪律靠后端提示词硬段（`ai_studio/presets.py`
 * 的 STRUCTURE_LOCK）+ `critic.structure_compare` 校验兜底；界面要在出图前
 * 把「要绝对不变就用本机」讲清楚。原文与 Python 侧保持同一句，两侧都要改时
 * 以 Python 为准。
 */
export const STRUCTURE_LOCK_ZH = "颜色与质感可以变，几何形状一律不许变。";
