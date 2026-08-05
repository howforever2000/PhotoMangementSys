import type { Album } from "../types/album";

/**
 * 时间分组工具
 *
 * 将相册按「年 → 季节 → 月」三层目录结构分组，用于时间线视图。
 * 排序依据优先取拍摄时间（shoot_time, YYYY-MM-DD），
 * 无拍摄时间则回退到创建时间（created_at），仍无则归入「未分类」。
 */

/** 季节定义（北半球） */
const SEASONS = [
  { key: "spring", name: "春季", months: [3, 4, 5] },
  { key: "summer", name: "夏季", months: [6, 7, 8] },
  { key: "autumn", name: "秋季", months: [9, 10, 11] },
  { key: "winter", name: "冬季", months: [12, 1, 2] },
] as const;

/** 月份中文名 */
export const MONTH_NAMES = [
  "1月", "2月", "3月", "4月", "5月", "6月",
  "7月", "8月", "9月", "10月", "11月", "12月",
];

/** 单个相册的时间信息 */
interface AlbumTime {
  album: Album;
  /** 完整日期串，用于排序比较 */
  sortKey: string;
  /** 年份 */
  year?: number;
  /** 月份 1-12 */
  month?: number;
  /** 季节 key */
  season?: string;
}

/** 从相册提取时间信息，无法识别返回 null */
function getAlbumTime(album: Album): AlbumTime | null {
  // 优先拍摄时间
  if (album.shoot_time) {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(album.shoot_time);
    if (m) {
      const year = Number(m[1]);
      const month = Number(m[2]);
      return {
        album,
        sortKey: m[0],
        year,
        month,
        season: monthToSeason(month),
      };
    }
  }
  // 回退创建时间
  if (album.created_at > 0) {
    const d = new Date(album.created_at * 1000);
    const year = d.getFullYear();
    const month = d.getMonth() + 1;
    const pad = (n: number) => String(n).padStart(2, "0");
    return {
      album,
      sortKey: `${year}-${pad(month)}-01`,
      year,
      month,
      season: monthToSeason(month),
    };
  }
  return null;
}

/** 月份 → 季节 key */
function monthToSeason(month: number): string {
  for (const s of SEASONS) {
    if (s.months.some((m) => m === month)) return s.key;
  }
  return "winter";
}

/** 季节 key → 中文名 */
export function seasonName(key: string): string {
  return SEASONS.find((s) => s.key === key)?.name ?? key;
}

/** 季节排序优先级（春1 夏2 秋3 冬4） */
export function seasonOrder(key: string): number {
  const idx = SEASONS.findIndex((s) => s.key === key);
  return idx === -1 ? 99 : idx;
}

/** 月分组节点 */
export interface MonthGroup {
  month: number;
  albums: Album[];
}

/** 季节分组节点 */
export interface SeasonGroup {
  season: string;
  months: MonthGroup[];
}

/** 年分组节点 */
export interface YearGroup {
  year: number;
  seasons: SeasonGroup[];
  /** 未分类相册（无时间信息） */
  uncategorized: Album[];
}

/**
 * 将相册数组按「年 → 季节 → 月」分组，按时间倒序排列。
 */
export function groupByTime(albums: Album[]): YearGroup[] {
  // 有时间信息的相册（拍摄时间或创建时间），用于构建时间树
  const timed = albums
    .map(getAlbumTime)
    .filter((t): t is AlbumTime => t !== null && t.year !== undefined && t.month !== undefined)
    .sort((a, b) => (a.sortKey < b.sortKey ? 1 : a.sortKey > b.sortKey ? -1 : 0));

  // 连创建时间都无法识别的相册（理论极少），归入"未分类"区
  const uncategorized = albums.filter((a) => {
    const t = getAlbumTime(a);
    return t === null || t.year === undefined;
  });

  // 构建 年 → 季节 → 月 树
  const yearMap = new Map<number, YearGroup>();
  for (const t of timed) {
    const year = t.year!;
    const month = t.month!;
    const season = t.season!;

    if (!yearMap.has(year)) {
      yearMap.set(year, { year, seasons: [], uncategorized: [] });
    }
    const yg = yearMap.get(year)!;

    let sg = yg.seasons.find((s) => s.season === season);
    if (!sg) {
      sg = { season, months: [] };
      yg.seasons.push(sg);
    }

    let mg = sg.months.find((m) => m.month === month);
    if (!mg) {
      mg = { month, albums: [] };
      sg.months.push(mg);
    }
    mg.albums.push(t.album);
  }

  // 排序：年份降序；季节/月内按时间倒序
  const years = [...yearMap.values()].sort((a, b) => b.year - a.year);
  for (const yg of years) {
    for (const sg of yg.seasons) {
      sg.months.sort((a, b) => b.month - a.month);
    }
    yg.seasons.sort((a, b) => {
      const ma = Math.max(...a.months.map((m) => m.month));
      const mb = Math.max(...b.months.map((m) => m.month));
      return mb - ma;
    });
  }

  // 若存在连时间都无法识别的相册，额外追加一个"未分类"年组
  if (uncategorized.length > 0) {
    years.push({ year: 0, seasons: [], uncategorized });
  }

  return years;
}

/**
 * 生成年 → 季节 → 月的「路线图」字符串（如 2024 年 › 夏季 › 6月）
 */
export function breadcrumb(year: number, season: string, month?: number): string {
  const parts = [`${year} 年`, seasonName(season)];
  if (month !== undefined) parts.push(MONTH_NAMES[month - 1]);
  return parts.join(" › ");
}
