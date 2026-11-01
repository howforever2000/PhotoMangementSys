/**
 * 重复照片分组的「清洗与显示」纯函数
 * ===================================================================
 *
 * 后端已经把「同一真实路径」的文件归并过了（`dedupe.rs` 的 `Entry::ident`
 * 与组内 `collapse_same_target`）。这里再兜一层，理由有两条：
 *
 * 1. **旧构建兼容**：升级前装的后端不会做这件事，前端拿到什么就得扛得住；
 * 2. **不变量守卫**：同一路径在同一个组里出现两次，会让「勾选删除」变成
 *    「对同一个文件发两条指令」，删除计数与可释放体积也全跟着错。
 *
 * 去重口径是**小写路径字符串**：同一台 Windows 上同一个文件不可能有大小写
 * 不同的两条有效路径，而真实路径解析（junction/短名）已经在后端做过。
 */
import type { DedupeReport, DupGroup, DupItem } from "../types/dedupe";

/** 组内按路径去重（保留首次出现的那条；大小写不敏感） */
export function dedupeItemsByPath(items: DupItem[]): DupItem[] {
  const seen = new Set<string>();
  const out: DupItem[] = [];
  for (const it of items) {
    const key = it.path.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(it);
  }
  return out;
}

/** 组的「可释放体积」= 除保留那张之外全部体积之和 */
export function reclaimableOf(items: DupItem[], keepPath: string): number {
  return items.reduce((n, it) => (it.path === keepPath ? n : n + it.size_bytes), 0);
}

/**
 * 清洗一次扫描结果：
 * - 组内同路径只留一张；
 * - 清洗后不足 2 张的组直接丢掉（只有一张就不叫重复）；
 * - 保留哪张若已被丢掉，回落到排第一的那张；
 * - 组数 / 可释放体积 / 同址合并计数跟着重算，保证界面读数自洽。
 */
export function sanitizeReport(report: DedupeReport): DedupeReport {
  let collapsed = report.collapsed_same_file ?? 0;
  const groups: DupGroup[] = [];

  for (const g of report.groups) {
    const items = dedupeItemsByPath(g.items);
    collapsed += g.items.length - items.length;
    if (items.length < 2) continue;
    const keep = items.some((it) => it.path === g.keep_path) ? g.keep_path : items[0].path;
    groups.push({
      ...g,
      keep_path: keep,
      items,
      reclaimable_bytes: reclaimableOf(items, keep),
    });
  }

  return {
    ...report,
    groups,
    exact_groups: groups.filter((g) => g.kind === "exact").length,
    similar_groups: groups.filter((g) => g.kind === "similar").length,
    collapsed_same_file: collapsed,
  };
}

/**
 * 地址的紧凑写法：只留末尾两级目录
 *
 * 卡片只有 148px 宽，完整路径（`D:\YUAN HAO\Pictures\img\手机照片\2026\…`）
 * 无论怎么截都只剩盘符那段噪声；用户认得出的是「最后两级」。
 * 完整路径挂在 `title` 上，需要时 hover 可看。
 */
export function shortDir(dir: string): string {
  const raw = (dir ?? "").trim();
  if (!raw) return "地址未知";
  const parts = raw.split(/[\\/]/).filter((p) => p.length > 0);
  if (parts.length <= 2) return raw;
  return `…\\${parts.slice(-2).join("\\")}`;
}
