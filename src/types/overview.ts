// 全局概览快照（FEAT-123）
//
// 对应 Rust 侧 `src-tauri/src/db/overview.rs` 的 `OverviewStats` / `OverviewCard`。
//
// 为什么单独一个类型：概览页只需要十来个聚合数，改前却要拉 `list_timeline`
// 的 24,980 行（14,438 KB）再在主线程遍历 6 遍。现在一次 `overview_stats`
// 返回 2~4 KB 快照，且照片库没变化时后端直接读缓存（不跑聚合）。

/** 最近导入 / 那年今日的卡片（只够渲染封面与一行副标题） */
export interface OverviewCard {
  path: string;
  album_id: number | null;
  album_name: string | null;
  shoot_time: string | null;
  location: string | null;
}

export interface OverviewStats {
  total_scanned: number;
  no_time: number;
  with_face: number;
  with_gps: number;
  year_min: number | null;
  year_max: number | null;
  /** `["2024-06", 952]` 形式，不含未记录拍摄时间 */
  months: Array<[string, number]>;
  recent: OverviewCard[];
  on_this_day: OverviewCard | null;
  on_this_day_count: number;
  indexed: number;
  stale: number;
  known: number;
  semantic_categories: number;
  model: string;
  signature: string;
  built_at: number;
}
