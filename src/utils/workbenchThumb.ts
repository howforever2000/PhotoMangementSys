/**
 * 胶片条 / 选片网格共用的缩略图状态文案（FEAT-180）
 * ===================================================================
 * 两处曾各写一份 `statusText`，已经分叉：胶片条用紧凑标记（`50%` / `✓` / `!`），
 * 网格用完整文案（`处理中 50%` / `完成`）。文案是产品口径、不在这里改，只把
 * 「状态 → 文案」的映射收成一份，避免以后一处改了另一处忘。
 */
import type { WbThumbItem } from "../types/workbench";

/** compact = 胶片条角标；verbose = 网格格子底部说明 */
export type ThumbStatusFormat = "compact" | "verbose";

export function thumbStatusText(
  it: Pick<WbThumbItem, "status" | "progress">,
  format: ThumbStatusFormat,
): string {
  switch (it.status) {
    case "queued":
      return "排队";
    case "running":
      return format === "compact" ? `${it.progress}%` : `处理中 ${it.progress}%`;
    case "done":
      return format === "compact" ? "✓" : "完成";
    case "failed":
      return format === "compact" ? "!" : "失败";
    default:
      return format === "compact" ? "" : "待处理";
  }
}
