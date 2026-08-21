// 内容扫描（AI 识别入库 + 照片智能搜索）相关类型
//
// 严格对应 Rust 侧 `src-tauri/src/db/content.rs` / `src-tauri/src/content.rs`
// 的结构体字段，保证前端 invoke 调用的参数与返回值类型安全。

import type { VisionResult } from "./photo";

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
