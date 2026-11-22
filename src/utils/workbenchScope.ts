/**
 * 工作台的「处理范围」语义（BUG-2026-1122-008）
 * ===================================================================
 * 抽成纯函数是为了能钉死语义：这套规则直接决定「用户点了按钮之后，
 * 哪些照片会被送去处理、会不会花钱」，靠组件里的 getter 表达时
 * 没法直接单测。
 *
 * ## 规则
 *
 *   1. **导入不自动勾选**。程序替用户勾选，等于替用户对「哪些照片要处理、
 *      要不要花钱」做了表态。导入 200 张就默认勾 200 张，是把一个花钱动作
 *      的决定权悄悄拿走。
 *   2. **没勾选 = 全量**。这条路没断，只是不该由程序替他选。
 *   3. **两种情况说法必须不同**：界面要说「已选 N 张」还是「全部 N 张」。
 *      两者都只写「N 张」的话，用户看到「批量套用到 12 张」会以为
 *      只处理了勾选的那些 —— 而实际是一张没勾的全量 12 张。
 *
 * ## 为什么「全量」这个默认仍然安全
 *
 * 因为出片前有确认框，确认框里写明范围（`scopeText`），
 * 而主按钮的文案也带「全部 / 已选」前缀 —— 花钱动作的范围是显式可见的。
 */
/**
 * ## 为什么只依赖 { path }
 *
 * 这几个函数只关心「哪些路径」，不关心条目的状态（待处理/完成/失败）。
 * 把入参类型收窄成 `{ path: string }` 而不是 store 的 `WbItem`，
 * 一来测试里不必构造完整条目，二来 store 改字段不会连带改这里。
 */

/** 本次要处理的条目：有勾选就是勾选的，否则全量 */
export function targetItems<T extends { path: string }>(
  items: T[],
  selected: string[],
): T[] {
  if (!selected.length) return items;
  const set = new Set(selected);
  return items.filter((i) => set.has(i.path));
}

/** 本次要处理的张数 */
export function targetCount(items: { path: string }[], selected: string[]): number {
  return targetItems(items, selected).length;
}

/**
 * 范围说法：「已选 N 张」还是「全部 N 张」。
 * selected 为空 = 没勾选 = 全量。
 */
export function scopeWord(selectedCount: number): "已选" | "全部" {
  return selectedCount > 0 ? "已选" : "全部";
}

/**
 * 确认框里的范围说明句。
 * 没勾选时必须点明「没勾选 = 全量」，否则用户可能以为只处理了一部分。
 */
export function scopeText(selectedCount: number, total: number): string {
  if (selectedCount > 0) {
    return `其中 ${selectedCount} 张是你勾选的`;
  }
  return `工作集里的全部 ${total} 张（没勾选 = 全量）`;
}
