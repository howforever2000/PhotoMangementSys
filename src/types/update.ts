/**
 * 应用内更新相关类型
 *
 * 这些字段与 `src-tauri/src/update.rs` 里 `#[derive(Serialize)]` 的结构**一一对应**
 * （serde 默认 snake_case，TS 侧也一律 snake_case，不做驼峰转换）。
 * 后端改名 / 增删字段时，必须同步改这里 —— 否则前端拿到的是 `undefined`，
 * 表现为「进度条不动 / 按钮永远停在初始态」这类不报错的静默故障。
 */

/** 一个可下载的 Release 资产（对应 Rust `UpdateAsset`） */
export interface UpdateAsset {
  /** 资产文件名，例如 `PhotoManagementSys_5.0.1_x64-setup.exe` */
  name: string;
  /** 字节数；`0` 表示未知（回退通道只知道 tag 时出现） */
  size: number;
  /** 官方 Release 直链 */
  url: string;
}

/** 一次检查的结果快照（对应 Rust `UpdateInfo`） */
export interface UpdateInfo {
  /** 当前运行版本（后端 `package_info().version`） */
  current: string;
  /** 远端最新版本（已去掉前导 `v`） */
  latest: string;
  has_update: boolean;
  /** ISO 时间字符串，可能为空（回退通道拿不到发布时间） */
  published_at: string;
  /** 更新说明（Release body 原文，可能是空串） */
  notes: string;
  /** 安装包资产；`null` = 该 Release 没有可自动安装的 .exe */
  asset: UpdateAsset | null;
  /** 本次检查时刻，**epoch 毫秒**（不是秒） */
  checked_at_ms: number;
  /** `"api"`（GitHub API 全量信息）或 `"redirect"`（重定向兜底，拿不到体积） */
  source: string;
}

/** 下载阶段（对应 Rust `UpdateDownloadStatus.stage` 的取值集合） */
export type UpdateStage =
  | "idle"
  | "probing"
  | "downloading"
  | "verifying"
  | "done"
  | "error"
  | "cancelled";

/** 下载进度（对应 Rust `UpdateDownloadStatus`） */
export interface UpdateDownloadStatus {
  /** 是否有下载任务正在跑（后端保证任何分支都会落回 false） */
  running: boolean;
  stage: UpdateStage;
  /** 已收到字节数 */
  received: number;
  /** 总字节数（`0` = 未知） */
  total: number;
  /** 瞬时速度（字节/秒；非下载阶段为 0） */
  bytes_per_sec: number;
  /** 失败原因；非 error 阶段为 null */
  error: string | null;
  /** 安装包落盘路径；只有 done 阶段有值 */
  path: string | null;
}

/** 前端一次性拿全的更新状态（对应 Rust `UpdateState`） */
export interface UpdateState {
  current: string;
  info: UpdateInfo | null;
  download: UpdateDownloadStatus;
}
