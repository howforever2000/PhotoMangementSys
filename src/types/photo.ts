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
}

/** 扫描进度事件 —— 对应 Rust `test_scan::ScanProgress` */
export interface ScanProgress {
  /** 阶段：resolve=解析地名 / organize=组织移动 */
  phase: "resolve" | "organize";
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
