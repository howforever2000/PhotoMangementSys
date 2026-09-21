// 照片 EXIF 扫描相关类型
//
// 严格对应 Rust 侧 `src-tauri/src/photo_scan.rs` 中的 `PhotoExif` 结构体，
// 保证前端 invoke 调用的返回值类型安全。

/** 单张照片的 EXIF 扫描结果 —— 对应 Rust `photo_scan::PhotoExif` */
export interface PhotoExif {
  /** 文件名（不含路径） */
  file_name: string;
  /** 完整路径（前端 tooltip 显示用） */
  path: string;
  /** ISO 感光度，如 "100"；缺失为 null */
  iso: string | null;
  /** 焦段，如 "50mm"；缺失为 null */
  focal_length: string | null;
  /** 光圈，如 "f/2.8"；缺失为 null */
  aperture: string | null;
  /** 快门速度，如 "1/200s"；缺失为 null */
  shutter_speed: string | null;
  /** 拍摄时间，如 "2023-01-15 10:30:00"；缺失为 null */
  shoot_time: string | null;
  /** 纬度（十进制度 WGS84）；无 GPS 为 null */
  lat: number | null;
  /** 经度（十进制度 WGS84）；无 GPS 为 null */
  lon: number | null;
  /** 纬度原始度分秒字符串，如 "31°55'16.61\"N" */
  lat_raw: string | null;
  /** 经度原始度分秒字符串 */
  lon_raw: string | null;
  /** 海拔（米） */
  alt_m: number | null;
  /** 地图链接（点开即定位） */
  map_url: string | null;
  /** 反向地理编码地名（with_place 扫描时填充） */
  place: string | null;
}

/** 影调类型 —— 对应 Rust `tone::ToneType` */
export type ToneType = "low-key" | "mid-key" | "high-key";

/** 图片扫描测试：单张照片 —— 对应 Rust `test_scan::TestPhoto` */
export interface TestPhoto {
  /** 文件名（不含路径） */
  file_name: string;
  /** 完整路径 */
  path: string;
  /** 拍摄时间 "YYYY-MM-DD HH:MM:SS"；缺失为 null */
  shoot_time: string | null;
  /** 年份 "2020"；缺失为 null */
  year: string | null;
  /** 纬度（十进制度）；无 GPS 为 null */
  lat: number | null;
  /** 经度 */
  lon: number | null;
  /** 地点（反编码简化，如 "达州市 · 萬源市"）；未解析为 null */
  place: string | null;
}

/** 组织移动报告 —— 对应 Rust `test_scan::OrganizeReport` */
export interface OrganizeReport {
  total: number;
  moved: number;
  conflict: number;
  no_time: number;
  no_place: number;
  failed: number;
  target_root: string;
  folders: string[];
  /** 用户中途取消（true 时上述计数为「已处理部分」的统计） */
  cancelled?: boolean;
}

// ---------------------------------------------------------------------------
// FEAT-064：扫描分组工具异步任务（退出页面不中断 + 并行扫描 + 进度可恢复）
// 对应 Rust `test_scan_job::JobSnapshot` / `JobProgress` / `OrganizeSummary`
// ---------------------------------------------------------------------------

/** 任务阶段 */
export type ScanJobPhase = "scan" | "resolve" | "organize";

/** 任务状态 */
export type ScanJobStatus = "idle" | "running" | "done" | "failed" | "cancelled";

/** 进度快照 —— 对应 Rust `test_scan_job::JobProgress` */
export interface ScanJobProgress {
  phase: ScanJobPhase;
  current: number;
  total: number;
  file_name: string;
  message: string;
  /** 每秒处理量（实时估算） */
  rate: number;
  /** 预计剩余秒数（无法估算为 null） */
  eta_sec: number | null;
}

/** 组织移动结果 —— 对应 Rust `test_scan_job::OrganizeSummary` */
export interface OrganizeSummary {
  total: number;
  moved: number;
  conflict: number;
  no_time: number;
  no_place: number;
  failed: number;
  target_root: string;
  folders: string[];
  cancelled?: boolean;
}

/**
 * 任务状态快照 —— 对应 Rust `test_scan_job::JobSnapshot`
 *
 * 页面挂载时 `get_scan_job` 拉取本结构即可**完整恢复**视图（进度/结果/错误），
 * 这是「退出页面任务不消失」在前端的落点。
 */
export interface ScanJobSnapshot {
  status: ScanJobStatus;
  phase: ScanJobPhase | null;
  /** 本次任务的目标目录 */
  dir: string;
  recursive: boolean;
  /** 任务使用的并行度（扫描阶段生效） */
  threads: number;
  progress: ScanJobProgress | null;
  /** 扫描结果照片数 */
  photo_count: number;
  /** 已解析出地名的张数 */
  place_count: number;
  /** 组织移动报告（仅 organize 完成时有值） */
  organize: OrganizeSummary | null;
  error: string;
  /** Unix 秒；0 = 未开始/未结束 */
  started_at: number;
  finished_at: number;
}

/** 扫描性能画像（CPU 拓扑 + 推荐线程数）—— 对应 Rust `scan_perf::CpuTopology` */
export interface ScanPerfTopology {
  /** 逻辑核数（含超线程） */
  logical: number;
  /** 物理核数 */
  physical: number;
  /** 大小核混合架构（Intel 12 代+） */
  hybrid: boolean;
  /** 物理核是否为推断值 */
  estimated: boolean;
  /** 磁盘介质类型（SSD / HDD / 未知） */
  disk_kind: string;
  /** 依据本机拓扑推荐的扫描线程数 */
  recommended: number;
  /** 推荐理由（一行说明） */
  reason: string;
  /** 可选档位 */
  options: number[];
  /** 用户已保存的自定义值（null = 跟随推荐） */
  saved: number | null;
  /** 实际生效值 */
  effective: number;
}

/** 实测校准的单个档位结果 —— 对应 Rust `calibrate_scan_threads.results[]` */
export interface ScanCalibEntry {
  threads: number;
  count?: number;
  ms?: number;
  per_sec?: number;
  per_file_ms?: number;
  error?: string;
}

/**
 * 实测校准结果 —— 对应 Rust `calibrate_scan_threads`
 *
 * `best_threads` 是「达峰值 95% 的最小档位」（性价比最优），
 * 与 `peak_threads`（绝对峰值）可能不同 —— UI 需同时展示并由用户决定采用哪个。
 */
export interface ScanCalibration {
  /** 实测样本张数 */
  sample: number;
  /** 目录内图片总数 */
  total: number;
  /** 样本平均文件大小（字节） */
  avg_bytes: number;
  /** 推荐采用的最优档（达峰值 95% 的最小值） */
  best_threads: number;
  /** 实测绝对峰值档 */
  peak_threads: number;
  /** 最优档吞吐（张/秒） */
  best_per_sec: number;
  /** 峰值档吞吐（张/秒） */
  peak_per_sec: number;
  /** 相对单线程的提速比 */
  speedup: number;
  /** 机检推荐值（拓扑推断） */
  recommended: number;
  logical: number;
  physical: number;
  disk_kind: string;
  /** 推荐理由（多段用「；」连接） */
  reason: string;
  results: ScanCalibEntry[];
}

/** 扫描进度事件 —— 对应 Rust `test_scan::ScanProgress` */
export interface ScanProgress {
  /** 阶段：scan=扫描 / resolve=解析地名 / organize=组织移动 */
  phase: "scan" | "resolve" | "organize";
  current: number;
  total: number;
  file_name: string;
  message: string;
}

/** 单张照片信息 —— 对应 Rust `photo_info::PhotoInfo`（按需实时读，不落库） */
export interface PhotoInfo {
  path: string;
  file_name: string;
  /** 格式（小写扩展名） */
  format: string;
  /** 原始宽度（px） */
  width: number;
  /** 原始高度（px） */
  height: number;
  /** 文件大小（字节） */
  file_size: number;
  /** R/G/B 三通道直方图，各 256 bin；解码失败为空数组 */
  hist_r: number[];
  hist_g: number[];
  hist_b: number[];
}

/** 照片批量删除结果 —— 对应 Rust `PhotoDeleteOutcome` */
export interface PhotoDeleteOutcome {
  requested: number;
  deleted: number;
  failed: number;
  failed_paths: string[];
}

/** 批量导出结果 —— 对应 Rust `ExportOutcome` */
export interface ExportOutcome {
  copied: number;
  skipped: number;
  failed: number;
  failed_paths: string[];
  dest_dir: string;
}

/** 照片移动结果 —— 对应 Rust `PhotoMoveOutcome` */
export interface PhotoMoveOutcome {
  requested: number;
  moved: number;
  failed: number;
  failed_paths: string[];
  target_id: number;
}

/** 打分录 —— (path, rating)，rating 0-5（0 表示未打分/已清除） */
export type PhotoRating = [string, number];

/** 人物照片条目 —— 对应 Rust `PersonPhotoItem`：
 *  - path 原图绝对路径
 *  - thumb 已算好的网格缩略图缓存路径（生成失败/未识别相册时为 null）
 *  - album_id 照片归属相册（解析失败为 null） */
export interface PersonPhotoItem {
  path: string;
  thumb: string | null;
  album_id: number | null;
}

/** 预热缩略图结果 —— 对应 Rust `PrewarmOutcome` */
export interface PrewarmOutcome {
  requested: number;
  hit: number;
  generated: number;
  failed: number;
}

/** 单张照片的影调分析结果 —— 对应 Rust `tone::PhotoTone` */
export interface PhotoTone {
  /** 文件名（不含路径） */
  file_name: string;
  /** 完整路径（前端 tooltip 显示用） */
  path: string;
  /** 灰度直方图，256 个 bin（索引 = 灰度值 0..255） */
  histogram: number[];
  /** 加权平均亮度 L̄（0..255）；解码失败为 null */
  avg_luma: number | null;
  /** 影调类型；无法统计为 null */
  tone_type: ToneType | null;
}

/** Top3 单项 —— 对应 Rust `vision::VisionTopItem` */
export interface VisionTopItem {
  /** 相册大类 */
  category: string;
  /** 最具体的 ImageNet 细类名 */
  label: string;
  /** 大类置信度（0~1） */
  confidence: number;
}

/** 单张图片的识别结果 —— 对应 Rust `vision::VisionResult` */
export interface VisionResult {
  /** 文件名（不含路径） */
  file_name: string;
  /** 完整路径 */
  path: string;
  /** 相册大类（portrait/street/animal/landscape_nature/architecture/...） */
  category: string;
  /** 子类（动物→狗/猫/鸟；可为空） */
  sub_category: string;
  /** 最具体的细类名（如 "golden retriever"） */
  label: string;
  /** 大类置信度（0~1） */
  confidence: number;
  /** Top3 候选 */
  top3: VisionTopItem[];
  /** 同人标号（如 ["P001","P003"]） */
  person_ids: string[];
  /** 检测到的人数 */
  person_count: number;
  /** 推理耗时（毫秒） */
  elapsed_ms: number;
  /** 单张失败原因 */
  error: string | null;
}

/** 批量识别进度事件载荷 —— 对应 Rust `vision::ClassifyProgress` */
export interface ClassifyProgress {
  current: number;
  total: number;
  done: number;
  failed: number;
}

/** 人物注册表条目 —— 对应 Rust `vision::PersonInfo` */
export interface PersonInfo {
  id: string;
  name: string;
  face_count: number;
  created_at: string;
}

/** 最近删除记录条目 —— 对应 Rust `RecentlyExcludedItem` */
export interface RecentlyExcludedItem {
  album_id: number;
  path: string;
  excluded_at: number;
  album_name: string;
}
