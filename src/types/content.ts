// 内容扫描（AI 识别入库 + 照片智能搜索）相关类型
//
// 严格对应 Rust 侧 `src-tauri/src/db/content.rs` / `src-tauri/src/content.rs`
// 的结构体字段，保证前端 invoke 调用的参数与返回值类型安全。

import type { VisionResult } from "./photo";

/** Top3 单项 —— 对应 Rust `vision::VisionTopItem` */
export interface VisionTopItem {
  category: string;
  label: string;
  confidence: number;
}

/** GPU 加速可行性状态 —— 对应 Rust `vision::VcrGpuStatus` */
export interface VcrGpuStatus {
  /** 服务是否在运行 */
  running: boolean;
  /** 当前是否实际走 GPU 推理 */
  use_gpu: boolean;
  /** 当前选中提供方 */
  provider: string;
  /** 检测到的 GPU 提供方列表 */
  gpu: string[];
  /** 全部可用提供方 */
  available: string[];
  /** 批次安全上限 */
  batch_max: number;
}

/** 内容扫描进度事件 —— 对应 Rust `content::ContentScanProgress` */
export interface ContentScanProgress {
  current: number;
  total: number;
  file_name: string;
}

/** 一次内容扫描报告 —— 对应 Rust `content::ScanReport` */
export interface ScanReport {
  /** 本次识别到的图片数（含识别失败） */
  total: number;
  /** 成功写入/更新的记录数 */
  written: number;
  /** 识别失败（未落库）数 */
  failed: number;
}

/** 内容扫描命令返回值 —— 对应 Rust `content::ScanOutcome` */
export interface ScanOutcome {
  report: ScanReport;
  /** 本次识别的照片明细（复用 `vision` 结果展示） */
  results: VisionResult[];
}

/** 内容搜索命中 —— 对应 Rust `db::ContentSearchHit` */
export interface ContentSearchHit {
  id: number;
  /** 照片绝对路径 */
  path: string;
  /** 父目录绝对路径 */
  parent_dir: string;
  /** 归属相册 ID（可能为 null） */
  album_id: number | null;
  /** 归属相册名称（可能为 null） */
  album_name: string | null;
  /** 归属相册路径 */
  album_path: string | null;
  /** 聚合可搜索文本（大类+细类+label+人物标号） */
  content: string;
  category: string | null;
  sub_category: string | null;
  label: string | null;
  confidence: number | null;
  /** 人物标号，如 ["P001","P003"] */
  person_ids: string[];
  shoot_time: string | null;
  location: string | null;
  iso: string | null;
  aperture: string | null;
  shutter_speed: string | null;
  focal_length: string | null;
}

// ---- FEAT-026：组合扫描 + 读表 + 条件搜索 ----

/** 组合扫描统一展示行 —— 对应 Rust `content::UnifiedScanRow` */
export interface UnifiedScanRow {
  file_name: string;
  path: string;
  // EXIF
  iso: string | null;
  aperture: string | null;
  shutter_speed: string | null;
  focal_length: string | null;
  shoot_time: string | null;
  iso_num: number | null;
  focal_num: number | null;
  aperture_num: number | null;
  shutter_num: number | null;
  // 影调
  tone_type: string | null;
  avg_luma: number | null;
  // AI
  category: string | null;
  sub_category: string | null;
  label: string | null;
  confidence: number | null;
  top3: VisionTopItem[];
  person_ids: string[];
  person_count: number;
}

/** 组合扫描结果 —— 对应 Rust `content::CombinedScanOutcome` */
export interface CombinedScanOutcome {
  report: ScanReport;
  rows: UnifiedScanRow[];
}

/** 内容搜索过滤条件 —— 对应 Rust `content::ContentScanFilters` */
export interface ContentScanFilters {
  iso_min: number | null;
  iso_max: number | null;
  shutter_min: number | null;
  shutter_max: number | null;
  aperture_min: number | null;
  aperture_max: number | null;
  focal_min: number | null;
  focal_max: number | null;
  tone_type: string | null;
}

/** 相册内容读表行 —— 对应 Rust `db::AlbumContentRow` */
export interface AlbumContentRow {
  id: number;
  path: string;
  parent_dir: string;
  album_id: number | null;
  album_name: string | null;
  album_path: string | null;
  iso: string | null;
  aperture: string | null;
  shutter_speed: string | null;
  focal_length: string | null;
  shoot_time: string | null;
  iso_num: number | null;
  focal_num: number | null;
  aperture_num: number | null;
  shutter_num: number | null;
  tone_type: string | null;
  avg_luma: number | null;
  content: string;
  category: string | null;
  sub_category: string | null;
  label: string | null;
  confidence: number | null;
  top3_json: string | null;
  person_ids: string[];
  person_count: number;
}

/** 智能搜索结果行 —— 对应 Rust `db::SmartHit`（FEAT-034） */
export interface SmartHit {
  id: number;
  path: string;
  album_id: number | null;
  album_name: string | null;
  category: string | null;
  sub_category: string | null;
  label: string | null;
  location: string | null;
  shoot_time: string | null;
  tone_type: string | null;
  person_ids: string[];
}
