// 图片扫描（大组件）相关类型
//
// 严格对应 Rust 侧 `src-tauri/src/test_scan.rs` 中的结构体，
// 保证前端 invoke 调用的返回值类型安全。

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
