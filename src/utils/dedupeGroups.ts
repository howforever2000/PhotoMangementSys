/**
 * 重复照片分组的「清洗 · 折叠 · 显示」纯函数
 * ===================================================================
 *
 * ## 为什么卡片要按「文件夹」折叠（BUG-2026-1101-005）
 *
 * 实测用户的真实库：同一张照片在 `img\Pictures` 有 3 份、`img\手机照片\无地点` 也有 3 份
 * （内容逐字节相同，文件名 `mmexport…957 / …592 / …866` 不同）。后端的粒度是「文件」，
 * 于是这一组显出 6 张看上去一模一样的缩略图，且其中几张与「保留」那张**地址相同**
 * —— 用户会以为那是同一张照片被重复列出，更怕点删除会把想留的那张删掉。
 *
 * 但「按路径去重」在这里没用：6 条是 6 个不同文件，路径各不相同，不该被吞掉。
 * 真正的解法是换展示粒度：**一个文件夹 = 一张卡**，卡上标「N 份」。
 *   · 卡片数 6 → 2，视觉上不再重复；
 *   · 每张卡只对应一个地址，不存在「和保留同地址的另一张可删卡」；
 *   · 保留所在文件夹的卡整张不可勾选 —— 想留的那张永远不会出现在待删集合里。
 *
 * ## 刻意保留的保守取舍
 *
 * 保留文件夹里其余的副本**不参与删除**（卡片上如实写「另有 N 份」）。代价是清理得没那么彻底，
 * 换来的是「勾了也删不到想留的那张」这条硬保证。清理类工具宁可少删，不可多删。
 */
import type { DedupeReport, DupGroup, DupItem } from "../types/dedupe";

/** 一个文件夹里的全部重复副本（卡片的展示与勾选单位） */
export interface FolderCell {
  /** 文件夹完整路径（原样保存，界面上完整显示） */
  dir: string;
  /** 该文件夹里的副本（沿用后端排序，items[0] 是代表图） */
  items: DupItem[];
  /** 组内「保留」那张是否在这个文件夹里 */
  isKeepFolder: boolean;
  /** 该文件夹里被保留的那张（仅 isKeepFolder 时非空） */
  keepPath: string | null;
}

/** 文件夹身份键（Windows 大小写不敏感） */
function dirKey(dir: string): string {
  return (dir ?? "").toLowerCase();
}

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

/** 「保留」那张所在的文件夹键（找不到时返回空串） */
export function keepDirKey(group: DupGroup, keepPath: string): string {
  const kept = group.items.find((it) => it.path === keepPath);
  return dirKey(kept?.dir ?? "");
}

/**
 * 把一组的照片折成「每个文件夹一张卡」
 *
 * 保留文件夹排在最前（它是参照物），其余保持后端排序（分辨率 → 体积 → 时间）。
 */
export function folderCells(group: DupGroup, keepPath: string): FolderCell[] {
  const keepDir = keepDirKey(group, keepPath);
  const map = new Map<string, FolderCell>();
  for (const it of dedupeItemsByPath(group.items)) {
    const key = dirKey(it.dir);
    let cell = map.get(key);
    if (!cell) {
      cell = { dir: it.dir, items: [], isKeepFolder: key === keepDir, keepPath: null };
      map.set(key, cell);
    }
    cell.items.push(it);
    if (it.path === keepPath) cell.keepPath = it.path;
  }
  const cells = [...map.values()];
  cells.sort((a, b) => Number(b.isKeepFolder) - Number(a.isKeepFolder));
  return cells;
}

/**
 * 可删路径 = 除「保留那张」与「保留所在文件夹里的其它副本」之外的全部
 *
 * 两条例外都是硬规则：
 * - 保留那张：勾了也删不到（用户的最终目的就是留一张）；
 * - 保留文件夹里的其它副本：同址删除是最容易误删的形状，这条规则让它不可能发生。
 */
export function deletablePathsOf(group: DupGroup, keepPath: string): string[] {
  const keepDir = keepDirKey(group, keepPath);
  return dedupeItemsByPath(group.items)
    .filter((it) => it.path !== keepPath && dirKey(it.dir) !== keepDir)
    .map((it) => it.path);
}

/** 某个卡片（文件夹）里的全部路径 */
export function cellPaths(cell: FolderCell): string[] {
  return cell.items.map((it) => it.path);
}

/** 某个卡片（文件夹）的总体积 */
export function cellBytes(cell: FolderCell): number {
  return cell.items.reduce((n, it) => n + it.size_bytes, 0);
}

/** 组的「可释放体积」= 组内可删路径体积之和 */
export function reclaimableOf(group: DupGroup, keepPath: string): number {
  const del = new Set(deletablePathsOf(group, keepPath));
  return group.items.reduce((n, it) => (del.has(it.path) ? n + it.size_bytes : n), 0);
}

/**
 * 清洗一次扫描结果：
 * - 组内同路径只留一张（同一文件不许并列出现两次）；
 * - 折叠成文件夹后没有可删副本的组直接丢掉（一张都不该删就不算要清理的重复）；
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
    const next: DupGroup = { ...g, keep_path: keep, items, reclaimable_bytes: 0 };
    if (deletablePathsOf(next, keep).length === 0) continue;
    next.reclaimable_bytes = reclaimableOf(next, keep);
    groups.push(next);
  }

  return {
    ...report,
    groups,
    exact_groups: groups.filter((g) => g.kind === "exact").length,
    similar_groups: groups.filter((g) => g.kind === "similar").length,
    collapsed_same_file: collapsed,
  };
}
