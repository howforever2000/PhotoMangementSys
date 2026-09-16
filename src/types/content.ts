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

/**
 * v5：语义模型档位候选项 —— 对应 model_registry.clip_models_info().models[]
 *
 * 与旧「分类模型」下拉同构（同样的下载/置灰/切换交互），只是换成了
 * Chinese-CLIP 档位：b16（fp16，512 维，默认）与 b16-fp32（可走 DirectML，快 2×）。
 * （L/14 档已实测否决下架，见 design/clip-accuracy-comparison.md）
 */
export interface VcrModelInfo {
  /** 档位标识：b16 / b16-fp32 */
  name: string;
  /** 中文说明（精度档位 + 推荐硬件） */
  label: string;
  accuracy: string;
  speed: string;
  /** 附加说明（模型体积 / 切换后需重建索引等） */
  note: string;
  /** 向量维度（512 / 768） */
  dim: number;
  /** 输入尺寸（224 / 336） */
  size: number;
  /** 模型体积（字节，UI 明示下载/切换成本；0 = 未知） */
  bytes: number;
  /** 模型文件是否已下载（含仅整图、待自动拆分的情况） */
  downloaded: boolean;
  /** 拆分件是否已就绪（可直接使用） */
  ready: boolean;
  /** 是否为当前生效档位 */
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
  /** v6：该会话实际绑定的 CPU 线程数（intra_op） */
  threads?: number | null;
}

/** v6：CPU 线程数现状 —— 对应 config.threads_info()（「⚙ 性能设置」展示） */
export interface VcrThreadsInfo {
  /** 当前生效线程数 */
  threads: number;
  /** 默认值（按物理核推测，夹在 4~8） */
  default: number;
  /** 物理核推测（逻辑核 ÷ 2） */
  physical_guess: number;
  /** 逻辑核数 */
  logical: number;
  min: number;
  max: number;
  /** 可选档位 */
  options: number[];
  /** 设置成功后服务端返回的生效值 */
  applied?: number;
}

/** v6：线程扫档单行 —— 对应 model_registry.benchmark_sweep() 的元素 */
export interface VcrSweepEntry {
  channel: string;
  threads: number;
  /** 实测平均/最快/最慢（ms/次推理） */
  avg_ms?: number;
  min_ms?: number;
  max_ms?: number;
  providers?: string[];
  /** 该档是否最快（服务端标记） */
  best?: boolean;
  /** 相对最慢档的提速倍数（前端计算，仅展示用） */
  speedup?: number;
  /** 该档测速失败原因 */
  error?: string;
}

/** v5：语义模型档位清单 */
export interface VcrModelsInfo {
  models: VcrModelInfo[];
  /** 当前生效档位标识 */
  current: string | null;
  /** CLIP 双塔会话是否已就绪（切换后台加载期间为 false，UI 据此提示加载中） */
  clip_ready?: boolean;
  /** FEAT-053：vision 塔会话实测事实（后台加载期间为 undefined） */
  loaded?: VcrSessionFacts;
  /** FEAT-053：text 塔会话实测事实 */
  loaded_text?: VcrSessionFacts;
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

/** FEAT-061：单源连通性自检结果（对应 Rust `model_dl::SourceProbe`） */
export interface ModelSourceProbe {
  url: string;
  host: string;
  builtin: boolean;
  ok: boolean;
  status: number;
  ms: number;
  error: string | null;
}

/** FEAT-061：下载源配置（内置模板 + 用户自定义模板） */
export interface ModelSourcesInfo {
  builtin: string[];
  custom: string[];
}

// ---------------------------------------------------------------------------
// v5 语义分类（Chinese-CLIP 关键词匹配）
// 对应 Rust `src-tauri/src/db/category.rs` / `src-tauri/src/category.rs`
// ---------------------------------------------------------------------------

/** 分类定义入参（新建/更新/预览共用） */
export interface CategoryInput {
  name: string;
  icon: string;
  /** 正向关键词（自然语言短语；命中取 max） */
  keywords: string[];
  /** 排除词（用于压制「热狗∈狗」这类误召回） */
  exclude_keywords: string[];
  /**
   * 匹配强度阈值 —— **内部净增益 rel（0.00~0.10，默认 0.03）**，不是 UI 数值。
   * UI 展示/编辑用 0~100 的「AI 匹配度」，换算见 `utils/matchScore`（rel × 1000，零迁移）。
   */
  threshold: number;
  sort_order: number;
  enabled: boolean;
}

/** 分类总览行（卡片） */
export interface CategoryOverview {
  id: number;
  name: string;
  icon: string;
  /** builtin（规则分类：人物/扫街/夜景/文档）| preset（内置预设）| user（用户自建） */
  source: string;
  /** builtin 规则键（portrait/street/night_scene/document），其余为空 */
  slug: string;
  threshold: number;
  enabled: boolean;
  keywords: string[];
  exclude_keywords: string[];
  count: number;
  cover_path: string | null;
  cover_album_id: number | null;
  cover_photo_hash: string | null;
}

/** 分类下的照片行（按语义强度降序） */
export interface CategoryPhoto {
  photo_hash: string;
  path: string;
  album_id: number | null;
  album_name: string | null;
  shoot_time: string | null;
  location: string | null;
  tone_type: string | null;
  person_ids: string[];
  category: string | null;
  sub_category: string | null;
  /** 语义匹配强度：内部净增益 rel（越大越像）；UI 用 relToMatch() 显示为 0~100 匹配度 */
  score: number;
  matched_keyword: string;
  category_id: number;
  category_name: string;
}

/** 预览样张 */
export interface CategoryPreviewSample {
  photo_hash: string;
  path: string;
  /** 归属相册（预览缩略图用；可能为 null） */
  album_id: number | null;
  score: number;
  matched_keyword: string;
}

/** 语义预览结果（改关键词/拖阈值实时看效果，不落库） */
export interface CategoryPreview {
  count: number;
  total: number;
  p50: number;
  p90: number;
  p99: number;
  max: number;
  samples: CategoryPreviewSample[];
  model: string;
}

/** 分类重建报告 */
export interface CategoryRebuildReport {
  categories: number;
  hits: number;
  ms: number;
  model: string;
  /** 参与匹配的照片数（当前档位下的向量数） */
  indexed: number;
  /** 索引为空 → 需先做一次「语义向量」扫描 */
  empty_index: boolean;
}

/** 语义索引覆盖统计（「已索引 N/M」+ 换档提醒） */
export interface CategoryIndexStats {
  /** 当前档位模型下的向量数（可参与匹配） */
  indexed: number;
  /** 其他档位留下的向量数（换档后需重建索引） */
  stale: number;
  /** 已入库照片总数 */
  known: number;
  model: string;
  /** 语义分类数（不含 builtin 规则分类） */
  semantic_categories: number;
}

/** v6：线程扫档结果包装 —— 对应 POST /benchmark_sweep */
export interface VcrSweepResult {
  results: VcrSweepEntry[];
}
