// 时间线（FEAT-122）按月分页的数据结构
//
// 严格对应 Rust 侧 `src-tauri/src/db/content.rs` 的
// `TimelineIndex` / `TimelineMonthIndex` / `TimelineCard`。
//
// 为什么要和 `ContentSearchHit` 分开：后者 18 列、实测约 630 B/行，
// 25,000 行 = 15.09 MB，一次进页面就把主线程堵住（BUG-2026-1027-003）。
// 时间线只需要 7 个字段 + 缩略图路径，约 190 B/行。

/** 一个「年·月」分组的计数（`year = "未记录"` 表示无拍摄时间那一组） */
export interface TimelineMonthIndex {
  year: string;
  /** "01"~"12"；未记录为空串 */
  month: string;
  count: number;
}

/** 时间线骨架：按月计数 + 首屏统计读数（约 3 KB） */
export interface TimelineIndex {
  months: TimelineMonthIndex[];
  total: number;
  no_time: number;
  no_location: number;
}

/** 时间线单张卡片 */
export interface TimelineCard {
  id: number;
  /** 照片绝对路径 */
  path: string;
  /** 归属相册（Lightbox 的 ensure_photo_scanned / 缩略图兜底用） */
  album_id: number | null;
  album_name: string | null;
  shoot_time: string | null;
  location: string | null;
  label: string | null;
  /**
   * 网格缩略图缓存绝对路径（后端 LEFT JOIN `photo_thumb_cache` 随行下发）。
   *
   * `null` = 该照片还没生成缩略图（实测 25,000 张里只有 67 张），
   * 前端此时才走 `get_photo_thumbs` 兜底 —— 而不是像改前那样
   * 每张卡都发一次 invoke。
   */
  thumb_path: string | null;
}
