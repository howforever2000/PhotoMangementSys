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
  /** FEAT-051：是否被用户强制 CPU（开关初始状态） */
  forced_cpu: boolean;
  /** FEAT-053：各通道会话实测 provider（未加载通道不出现）；与 use_gpu（请求值）对照验证 */
  sessions?: Record<string, string[]>;
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
  /** FEAT-SEM：语义命中余弦相似度（0~1）；纯关键词命中为 null（显示「AI 匹配」徽标用） */
  semantic_score: number | null;
}

/** FEAT-048：内容分类两级聚合行 —— 对应 Rust `db::CategoryGroupRow` */
export interface CategoryGroupRow {
  category: string;
  sub_category: string | null;
  count: number;
  /** 该大类封面（置信度最高的照片原图路径） */
  cover_path: string | null;
  /** 封面照片归属相册（get_photo_thumbs 复用真实相册缓存命名） */
  cover_album_id: number | null;
}

/** FEAT-049：地点聚合行 —— 对应 Rust `db::LocationGroupRow`（location null = 未记录地点组） */
export interface LocationGroupRow {
  location: string | null;
  count: number;
  cover_path: string | null;
  cover_album_id: number | null;
}

/** FEAT-051：分类模型候选项 —— 对应 model_registry.cls_models_info().models[] */
export interface VcrModelInfo {
  /** 模型文件名（如 yolov8x-cls.onnx） */
  name: string;
  /** 中文说明（准确率档位 + 推荐场景） */
  label: string;
  accuracy: string;
  speed: string;
  /** 模型文件是否已下载到 python/models/ */
  downloaded: boolean;
  /** 是否为当前生效模型 */
  active: boolean;
}

/** FEAT-053：cls 会话实测事实 —— 「确实换了模型 / 确实在用 GPU」的铁证 */
export interface VcrSessionFacts {
  /** 会话实际由哪个模型文件构建 */
  file: string;
  /** 模型文件字节数 */
  file_size: number | null;
  /** ORT 会话实际绑定的 provider（非请求值） */
  providers: string[];
  input_name: string | null;
  input_shape: (number | string)[];
  input_type: string | null;
  /** GPU 建会话失败后是否回退了 CPU */
  cpu_fallback: boolean;
}

/** FEAT-051：分类模型清单 */
export interface VcrModelsInfo {
  models: VcrModelInfo[];
  /** 当前生效模型文件名（候选中第一个已下载者，或用户指定项） */
  current: string | null;
  /** cls 会话是否已就绪（切换后台加载期间为 false，UI 据此提示加载中） */
  cls_ready?: boolean;
  /** FEAT-053：cls 会话实测事实（后台加载期间为 undefined）；与 current 对照确认切换生效 */
  loaded?: VcrSessionFacts;
}

/** FEAT-053：固定张量测速结果 */
export interface VcrBenchmarkResult {
  channel: string;
  runs: number;
  warmup: number;
  input_shape: number[];
  /** 本次测速会话实际绑定的 provider */
  providers: string[];
  avg_ms: number;
  min_ms: number;
  max_ms: number;
  total_ms: number;
  throughput_per_s: number | null;
}

/** FEAT-052：模型下载状态 —— 对应 Rust `model_dl::ModelDlStatus` */
export interface ModelDlStatus {
  name: string;
  file: string;
  required: boolean;
  running: boolean;
  done: boolean;
  stage: string; // downloading | exporting | done | error | idle
  bytes: number;
  total: number;
  error: string | null;
}
