/**
 * 创意工坊 · 工具数据类型（FEAT-150）
 *
 * 与 `src-tauri/src/tools.rs` 的返回结构一一对应（serde `rename_all = "camelCase"`）。
 * 单独成文件而非塞进 `types/photo.ts`：这些类型只服务工坊工具，
 * 混进照片主类型会让「哪些字段是照片的」变模糊。
 */

/** 压缩参数（可选，缺省 = 默认质量 82 转 JPEG，不改尺寸） */
export interface CompressOptions {
  /** 目标文件大小（KB）。给了就凑大小，忽略 quality */
  targetKb?: number | null;
  /** 固定质量 1~95（targetKb 为空时生效） */
  quality?: number | null;
  /** 长边上限（px），0 / 不传 = 不缩放 */
  maxEdge?: number | null;
  /** 居中裁切比例：`1:1` / `4:3` / `3:4` / `16:9` / `9:16`；空 = 不裁 */
  cropAspect?: string | null;
}

/** 一张压缩成功的结果 */
export interface CompressedItem {
  src: string;
  dst: string;
  before: number;
  after: number;
  width: number;
  height: number;
  /** 实际使用的 JPEG 质量（凑大小时由后端搜索得出） */
  quality: number;
}

export interface CompressFailure {
  src: string;
  reason: string;
}

export interface CompressOutcome {
  items: CompressedItem[];
  failed: CompressFailure[];
  totalBefore: number;
  totalAfter: number;
  destDir: string;
}

/** 压缩后台任务状态（对应 src-tauri/src/compress_job.rs `CompressJobStatus`） */
export type CompressJobStatus =
  | "idle"
  | "running"
  | "done"
  | "failed"
  | "cancelled";

/**
 * 压缩后台任务快照（对应 src-tauri/src/compress_job.rs `CompressJobSnapshot`）
 *
 * 任务状态提升到后端进程后，前端任何时刻（含重新进入创意工坊）靠它恢复完整视图；
 * 取消时 `items` 保留已完成的部分结果。
 */
export interface CompressJobSnapshot {
  status: CompressJobStatus;
  destDir: string;
  /** 待压缩总张数 */
  total: number;
  /** 已完成张数（= items.length） */
  done: number;
  items: CompressedItem[];
  failed: CompressFailure[];
  totalBefore: number;
  totalAfter: number;
  /** 正在处理的源文件路径 */
  current: string;
  error: string;
  startedAt: number;
  finishedAt: number;
  /** 是否被用户中途停止 */
  cancelled: boolean;
}
