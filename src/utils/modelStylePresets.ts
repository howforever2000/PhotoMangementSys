/**
 * 模型风格预设（FEAT-198）· 前端镜像
 * ===================================================================
 * 真相源在 `python/ai_studio/model_styles.json`（6 条影调风格：中文原文 + 英文要点），
 * 由 `python/ai_studio/modelstyle.py` 载入并拼进图像编辑提示词。
 * 本文件**只镜像 id 与中文标签**，供 chips 渲染；提示词正文一个字都不抄进来 ——
 * 抄一份就是两处漂移，漂移后的症状是「点了没反应」（后端 `compose()` 认不到 id
 * 会返回空串，静默不加风格）。守卫测试 `modelStylePresets.test.ts` 双向钉住 id/标签/顺序。
 *
 * ## 为什么是「追加」而不是「替换」
 *
 * 截图里的原文每条约 150~250 字，写的是**影调配方**，不是「我要什么」。它必须接在
 * 用户自己的画面描述后面（截图原话：「用法：接在你的画面描述后面」）。所以选中
 * 预设 ≠ 覆盖输入框，而是把风格段落追加到末尾 —— 用户在界面上看得见、能继续编辑。
 */
import stylesJson from "../../python/ai_studio/model_styles.json" with { type: "json" };

export interface ModelStylePreset {
  /** 与后端 `modelstyle.style_ids()` 同域 */
  id: string;
  label: string;
  /** 中文原文：选中时追加进**可见**输入框（用户可继续编辑） */
  zh: string;
}

/** 全部风格预设（声明顺序即 chips 顺序） */
export const MODEL_STYLES: ModelStylePreset[] = (
  stylesJson as unknown as { styles: Array<{ id: string; label: string; zh: string }> }
).styles.map((s) => ({ id: s.id, label: s.label, zh: s.zh }));

export function modelStyleLabel(id: string): string {
  return MODEL_STYLES.find((s) => s.id === id)?.label ?? id;
}

export function modelStyleZh(id: string): string {
  return MODEL_STYLES.find((s) => s.id === id)?.zh ?? "";
}

/** 切换某款风格：再点一次取消（用户改了主意不该被迫「重开一个输入框」） */
export function toggleStyle(current: string, id: string): string {
  return current === id ? "" : id;
}

/**
 * 把某款风格的中文原文接在用户描述后面（Q4(a)：可见、可编辑）。
 *
 * 三条纪律：
 *   1. **不覆盖**用户已写的意图 —— 截图原文就是「接在你的画面描述后面」；
 *   2. **幂等** —— 已包含这段就不再追加（连点两次不该出现两份）；
 *   3. 取消选中时**不回删**：用户可能已经手改了这段文字，替他删掉属于破坏输入。
 *      要清掉就自己删，或点「清空风格」。
 */
export function appendStyleText(prompt: string, styleId: string): string {
  const zh = modelStyleZh(styleId).trim();
  if (!zh) return prompt;
  const base = (prompt || "").trim();
  if (!base) return zh;
  if (base.includes(zh)) return base;
  return `${base}\n${zh}`;
}

/**
 * 把**之前追加过**的风格段落从提示词里摘掉（换风格 / 取消时用）。
 *
 * 只在用户没改过这段原文时才生效：`includes` 比对的是原文，用户动过就不再匹配，
 * 此时保留用户的编辑（宁可留文字，也不能悄悄删掉用户写的东西）。
 */
export function stripStyleText(prompt: string, styleId: string): string {
  const zh = modelStyleZh(styleId).trim();
  if (!zh) return prompt;
  const base = (prompt || "").trim();
  if (!base.includes(zh)) return base;
  return base.split(zh).join("").trim();
}

/** 换风格：先摘掉旧段落，再接上新段落（空 id = 只摘不加） */
export function switchStyleText(prompt: string, fromId: string, toId: string): string {
  const cleaned = fromId ? stripStyleText(prompt, fromId) : (prompt || "").trim();
  return toId ? appendStyleText(cleaned, toId) : cleaned;
}
