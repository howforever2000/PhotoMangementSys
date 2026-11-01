/**
 * 入库体检的**判定口径**（纯函数，无 IO、无 Vue 依赖）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么把"结论"放前端而不放后端
 * ──────────────────────────────────────────────────────────────────────────
 * 后端只回事实（多少张、有没有缩略图、抽样里丢了几张）。判定是**产品表述**：
 * 差 3 张算不算"不完整"、没有向量该算提示还是正常，这类阈值随时会改，
 * 而它们改起来不该动 Rust、不该重启后端、不该碰 DB 口径。
 *
 * 放成纯函数还有一个直接好处：可以直接单测（`albumHealth.test.ts`）钉死
 * 几种最容易写错的语义 —— 空相册不是问题、根目录失联盖过一切、抽样不是全量。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 三档严重度（前端只用这三个词，别再造同义词）
 * ──────────────────────────────────────────────────────────────────────────
 *   bad  严重：数据已经不对，且用户能修（整册未入库 / 根目录没了 / 记录指向空气）
 *   warn 提示：能用但有缺口（部分未入库 / 缺缩略图 / 缺向量）
 *   ok   正常：没有需要动作的问题
 */
import type { AlbumHealthRow } from "../types/health";

export type HealthLevel = "ok" | "warn" | "bad";

/** 一条体检问题 */
export interface HealthIssue {
  /** 稳定键（前端用于分组/去重，不参与展示） */
  key: string;
  level: HealthLevel;
  /** 短标签（一行内读得完） */
  label: string;
  /** 补充说明（写清"差多少 / 抽样了多少"，不含建议动作） */
  hint: string;
}

/** 覆盖率（0~1）；分母为 0 时按 0 处理，由调用方用 `photo_count === 0` 单独判空册 */
export interface Coverage {
  scan: number;
  thumb: number;
  embed: number;
}

function ratio(part: number, whole: number): number {
  if (whole <= 0) return 0;
  return Math.max(0, Math.min(1, part / whole));
}

/** 入库 / 缩略图 / 向量三条覆盖率 */
export function albumCoverage(row: AlbumHealthRow): Coverage {
  return {
    scan: ratio(row.scanned, row.photo_count),
    thumb: ratio(row.thumbs, row.scanned),
    embed: ratio(row.embeddings, row.scanned),
  };
}

/** 入库缺口张数（磁盘有、库里没有）。只用 `photo_count - scanned`，不允许为负 */
export function unscannedCount(row: AlbumHealthRow): number {
  return Math.max(0, row.photo_count - row.scanned);
}

/**
 * 入库完整度阈值：缺 10% 以上才算"不完整"
 *
 * 为什么不是 0：相册目录里随时可能有刚拖进去的照片，扫描又是手动触发的，
 * 差个位数就报警会让整页永远是红的，用户反而学会忽略所有提示。
 */
export const INCOMPLETE_RATIO = 0.9;

/**
 * 推导一册的问题清单（按严重度降序，同一严重度保持业务顺序）
 *
 * 语义边界（测试里钉死）：
 *  - 空相册（`photo_count === 0` 且零入库）→ 没有问题，它是"还没放照片"，不是故障；
 *  - 根目录不存在 → 只报这一条（其余覆盖率数字都失去意义，不再叠加噪音）；
 *  - 抽样缺失 → 措辞必须带"抽样"，不许冒充全量结论。
 */
export function albumIssues(row: AlbumHealthRow): HealthIssue[] {
  const issues: HealthIssue[] = [];

  if (!row.root_exists) {
    issues.push({
      key: "root-missing",
      level: "bad",
      label: "相册目录不存在",
      hint: "磁盘上已找不到这个目录：相册可能被改名 / 移动 / 删除，建议移出相册库后重新导入",
    });
    return issues;
  }

  if (row.photo_count === 0 && row.scanned === 0) {
    return issues;
  }

  const missing = unscannedCount(row);
  if (row.scanned === 0 && row.photo_count > 0) {
    issues.push({
      key: "not-scanned",
      level: "bad",
      label: "整册未入库",
      hint: `磁盘 ${row.photo_count} 张，库里 0 张：这批照片进不了时间线 / 智慧相册 / 重复检测`,
    });
  } else if (missing > 0 && row.scanned < row.photo_count * INCOMPLETE_RATIO) {
    issues.push({
      key: "partial-scan",
      level: "warn",
      label: `入库不完整（差 ${missing} 张）`,
      hint: `磁盘 ${row.photo_count} 张 · 已入库 ${row.scanned} 张`,
    });
  }

  if (row.missing_files > 0) {
    issues.push({
      key: "files-missing",
      level: "bad",
      label: `记录指向的文件已不存在（抽样 ${row.sampled} 条中 ${row.missing_files} 条）`,
      hint: "入库记录还在，但文件已被移动或删除：这些照片会以空白缩略图出现在列表里",
    });
  }

  if (row.scanned > 0 && row.thumbs < row.scanned) {
    issues.push({
      key: "thumbs",
      level: "warn",
      label: `缺缩略图（${row.scanned - row.thumbs} 张）`,
      hint: "首次打开这些照片时要现场生成，列表滚动会卡一下",
    });
  }

  if (row.scanned > 0 && row.embeddings === 0) {
    issues.push({
      key: "no-embeddings",
      level: "warn",
      label: "没有图像向量",
      hint: "画面相似去重、以图搜图、语义搜索对这册无效",
    });
  } else if (row.scanned > 0 && row.embeddings < row.scanned * INCOMPLETE_RATIO) {
    issues.push({
      key: "partial-embeddings",
      level: "warn",
      label: `向量覆盖不全（${row.embeddings}/${row.scanned}）`,
      hint: "通常是换模型档位后没有重建：语义检索会漏掉这批照片",
    });
  }

  return issues;
}

/** 一册的整体严重度 = 它最严重的那条问题 */
export function albumLevel(row: AlbumHealthRow): HealthLevel {
  const issues = albumIssues(row);
  if (issues.some((i) => i.level === "bad")) return "bad";
  if (issues.some((i) => i.level === "warn")) return "warn";
  return "ok";
}

/** 整页结论：三档计数 + 需要动作的关键统计 */
export interface HealthSummary {
  bad: number;
  warn: number;
  ok: number;
  /**
   * 有照片但一张都没入库的相册数（最该点的入口）
   *
   * 根目录失联的相册不算进来：它的修法是「移出相册库」，与「去扫描」是两件事，
   * 混在一个数字里会让入口按钮指向错误的动作。
   */
  unscannedAlbums: number;
  /** 根目录失联的相册数 */
  missingRootAlbums: number;
  /** 抽样里丢文件的相册数 */
  brokenAlbums: number;
  /** 磁盘总张数与已入库总张数（覆盖率的分母/分子） */
  totalPhotos: number;
  totalScanned: number;
  /** 入库覆盖率（0~1） */
  scanCoverage: number;
}

export function healthSummary(rows: AlbumHealthRow[]): HealthSummary {
  const sum: HealthSummary = {
    bad: 0,
    warn: 0,
    ok: 0,
    unscannedAlbums: 0,
    missingRootAlbums: 0,
    brokenAlbums: 0,
    totalPhotos: 0,
    totalScanned: 0,
    scanCoverage: 0,
  };
  for (const row of rows) {
    const level = albumLevel(row);
    sum[level] += 1;
    if (row.root_exists && row.photo_count > 0 && row.scanned === 0) sum.unscannedAlbums += 1;
    if (!row.root_exists) sum.missingRootAlbums += 1;
    if (row.missing_files > 0) sum.brokenAlbums += 1;
    sum.totalPhotos += row.photo_count;
    sum.totalScanned += row.scanned;
  }
  sum.scanCoverage = ratio(sum.totalScanned, sum.totalPhotos);
  return sum;
}

/** 列表排序：严重在前，同档按缺口张数降序（影响面大的先看） */
export function sortBySeverity(rows: AlbumHealthRow[]): AlbumHealthRow[] {
  const rank: Record<HealthLevel, number> = { bad: 0, warn: 1, ok: 2 };
  return [...rows].sort((a, b) => {
    const d = rank[albumLevel(a)] - rank[albumLevel(b)];
    if (d !== 0) return d;
    const da = unscannedCount(a) + a.missing_files;
    const db = unscannedCount(b) + b.missing_files;
    if (db !== da) return db - da;
    return a.name.localeCompare(b.name, "zh");
  });
}
