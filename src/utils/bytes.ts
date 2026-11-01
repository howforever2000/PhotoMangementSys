/**
 * 体积格式化（FEAT-150 工坊工具用）
 *
 * 为什么不复用现有组件里的写法：压缩结果与配方清理都要显示「2.4 MB → 480 KB」，
 * 同一个数值在两处必须显示成同一个样子，否则用户会怀疑两个数字不是一回事。
 */

/**
 * 字节数 → 人类可读体积。
 *
 * 规则（刻意保守）：
 * - ≥ 1 GB 用 GB（保留 1 位）
 * - ≥ 1 MB 用 MB（保留 1 位）
 * - 其余用 KB（整数）
 * - 0 显示 `0 KB`（不显示 `0 B`：这套 UI 里最小的单位就是 KB，B 只会制造噪声）
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB";
  const kb = bytes / 1024;
  if (kb >= 1024 * 1024) return `${(kb / 1024 / 1024).toFixed(1)} GB`;
  if (kb >= 1024) return `${(kb / 1024).toFixed(1)} MB`;
  return `${Math.round(kb)} KB`;
}

/**
 * 压缩率文案：`体积减少 82%`。
 *
 * 边界：总体积为 0（全失败）→ 返回空串，由调用方决定显示什么；
 * 不返回「减少 0%」——那是「什么都没发生」，不是结果。
 */
export function savedPercent(before: number, after: number): string {
  if (!before || before <= 0) return "";
  const pct = Math.round((1 - after / before) * 100);
  return pct > 0 ? `体积减少 ${pct}%` : "体积未减少";
}
