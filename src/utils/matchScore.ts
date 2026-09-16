/**
 * 语义「AI 匹配度」换算与档位（FEAT-056）—— 唯一真相点
 *
 * 背景：语义匹配内部用的是「净增益」rel（0~0.10，见 python/vsc… / src-tauri/src/category.rs）：
 *     rel = max_kw cos(图片, 关键词) − mean_b cos(图片, 中性描述)
 * 这个量对用户不直观（0.03 是什么鬼？）。UI 统一展示为 **0~100 的整数「AI 匹配度」**：
 *     AI 匹配度 = round(rel × 1000)      例：rel 0.030 → 30
 *
 * 为什么换算放在前端、内部仍存 rel：
 *   - 后端 DB / 命令 / 预览 API 全部保持 rel，**零迁移**（用户已有分类的 0.01/0.03 自动显示为 10/30）；
 *   - 换算只有一处（本文件），前后端语义边界清晰，不会出现"两边各写一次 ×1000"的漂移。
 *
 * 注意：搜索页结果卡上的「✨ AI 匹配 N%」是**原始余弦相似度**（FEAT-SEM），
 * 与这里的相对匹配度不是同一个量，故本文件的文案统一用「匹配度」而非「匹配 N%」。
 */

/** 内部 rel → UI 匹配度 的换算倍数（rel 的可分辨范围约 0~0.10） */
export const MATCH_SCALE = 1000;

/** 匹配度上限（= rel 0.10；后端 THRESHOLD_MAX 同值） */
export const MATCH_MAX = 100;

/** 新建分类的默认匹配度（= rel 0.03，P0 实测推荐值） */
export const MATCH_DEFAULT = 30;

/** rel（净增益）→ 匹配度整数（0~100） */
export function relToMatch(rel: number | null | undefined): number {
  if (rel == null || Number.isNaN(rel)) return 0;
  return Math.round(Math.max(0, Math.min(1, rel)) * MATCH_SCALE);
}

/** 匹配度（0~100）→ rel（净增益），用于写回后端 */
export function matchToRel(match: number): number {
  return Math.max(0, Math.min(MATCH_MAX, match)) / MATCH_SCALE;
}

/** 匹配度档位（对应用户直觉：宽松一点多召回 / 严格一点更准） */
export const MATCH_PRESETS = [
  { label: "宽松", value: 10, hint: "召回更多，可能混入误命中" },
  { label: "标准", value: 30, hint: "推荐默认（P0 实测：抽查精度 70~90%）" },
  { label: "严格", value: 50, hint: "只留最像的，命中会明显变少" },
] as const;

/** 一句话解释（对话框与提示共用，避免各处文案不一致） */
export const MATCH_HELP =
  "AI 匹配度（0~100）：图片与你关键词的匹配强度，已扣除与「一张照片」这类中性描述的相似度。" +
  "数值越高越严格——命中更少但更准。默认 30。";
