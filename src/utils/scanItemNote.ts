/**
 * 全局扫描逐相册「备注」列文案（纯函数）
 *
 * 背景：BUG-2026-0922-008 —— 增量扫描本身工作正常（确实跳过了已入库照片），
 * 但前端一张张写着「共 0 张」。
 *
 * 根因是 `total` 语义漂移：P8~P10 引入增量差集后，后端的 `total` 从
 * 「该相册识别到的图片数」变成了「本次待处理的差集大小」。已入库相册差集为空
 * ⇒ total = 0 ⇒ 表格渲染出「共 0 张」。而「相册里明明有照片」与「本次没有要处理的」
 * 是**两件完全不同的事**，被同一个数字表达就成了自相矛盾的界面。
 *
 * 修复分两层：
 *   后端：`total` 恢复为「目录内图片总数」，另加 `processed`（本次处理）与
 *         `skipped`（已是最新而跳过），恒有 `total === processed + skipped`。
 *   本函数：把三种语义**分开措辞**，不再用一个 N 糊过去。
 *
 * 抽成独立纯函数的原因（沿用项目对 `confirmSummary` / `scanImport` 的一贯做法）：
 * 这些分支靠肉眼看表格很难覆盖全（尤其「空相册」与「全是跳过」在 UI 上
 * 只差一个数字），必须能用单测钉死。
 */

/** 备注列渲染所需的字段（与 `GlobalScanItem` 结构兼容，但不依赖 Pinia） */
export interface ScanItemNoteInput {
  status: string;
  /** 该相册目录内的照片总数 */
  total: number;
  /** 成功写入/更新的记录数 */
  written: number;
  /** 本次真正处理的张数 */
  processed: number;
  /** 因已入库且未变化而跳过的张数 */
  skipped: number;
}

export interface ScanItemNote {
  /** 表格里显示的短文案 */
  text: string;
  /** hover 提示（更完整的解释） */
  title: string;
}

/**
 * 生成备注列文案。
 *
 * 三种语义，措辞互不混用：
 * - `total === 0`            → 「空相册」：这才是真的 0，用户不会惊讶
 * - 有照片、全部跳过、零写入  → 「已是最新，跳过 M 张」：预期行为，不是异常
 * - 其余                     → 「共 T 张（本次处理 P 张 · 跳过 M 张）」
 *
 * 注意最后一条里 `共 T 张` 与后面的补充说明是并列的：T 永远是相册真实大小，
 * 补充项只在非零时才出现（避免「本次处理 0 张」这种噪音）。
 */
export function scanItemNote(it: ScanItemNoteInput): ScanItemNote {
  if (it.status !== "done") return { text: "—", title: "" };
  const { total, written, processed, skipped } = it;
  if (total === 0) {
    return { text: "空相册", title: "该相册目录下没有图片文件" };
  }
  // 「全跳过」必须与「处理了但没写进去」区分开：
  // 前者 written === 0 且 processed === 0（正常增量），后者 processed > 0（需排查）。
  if (written === 0 && processed === 0 && skipped > 0) {
    return {
      text: `已是最新，跳过 ${skipped} 张`,
      title: `目录内 ${total} 张全部已入库且文件未变化，本次无需处理`,
    };
  }
  const extras: string[] = [];
  if (processed > 0) extras.push(`本次处理 ${processed} 张`);
  if (skipped > 0) extras.push(`跳过 ${skipped} 张`);
  const text =
    extras.length > 0 ? `共 ${total} 张（${extras.join(" · ")}）` : `共 ${total} 张`;
  return {
    text,
    title: [`目录共 ${total} 张`, ...extras, `入库 ${written} 张`].join(" · "),
  };
}
