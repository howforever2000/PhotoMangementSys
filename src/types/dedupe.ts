/**
 * 重复照片清理（FEAT-143）前端类型
 *
 * 与后端 `src-tauri/src/dedupe.rs` 的 `DupItem` / `DupGroup` / `DedupeReport`
 * 逐字段对应（serde 默认 snake_case，故字段名保持一致）。
 */

/** 组内一张照片 */
export interface DupItem {
  path: string;
  album_id: number;
  file_name: string;
  size_bytes: number;
  width: number | null;
  height: number | null;
  /** 修改时间（Unix 秒）；保留策略的第三顺位 */
  modified_secs: number;
}

/** 一个重复组 */
export interface DupGroup {
  /** 稳定标识：`exact:<hash>` / `similar:<保留路径>` */
  key: string;
  /** exact = 内容哈希一致；similar = 向量相似 */
  kind: "exact" | "similar";
  /** exact 恒为 1；similar 为「保留那张 vs 其余」的最低余弦 */
  similarity: number;
  /** 默认保留的那张 */
  keep_path: string;
  /** 删掉除保留外全部可释放的字节数 */
  reclaimable_bytes: number;
  items: DupItem[];
}

/** HEIC 解码能力（缺 HEIF 扩展时界面提示） */
export interface HeicSupport {
  available: boolean;
  codec: string | null;
  reason: string | null;
}

/** 一次重复扫描结果 */
export interface DedupeReport {
  groups: DupGroup[];
  total_photos: number;
  similar_candidates: number;
  skipped_unencoded: number;
  exact_groups: number;
  similar_groups: number;
  threshold: number;
  embedding_model: string | null;
  similar_skipped_reason: string | null;
  elapsed_ms: number;
  heic: HeicSupport;
  hashed_files: number;
}

/** 扫描进度事件载荷（`dedupe-progress`） */
export interface DedupeProgress {
  stage: "collect" | "hash" | "similar" | "done" | string;
  done: number;
  total: number;
  message: string;
}

/** 删除结果（与后端 `PhotoDeleteOutcome` 对应） */
export interface DedupeDeleteOutcome {
  requested: number;
  deleted: number;
  failed: number;
  failed_paths: string[];
}

/** 删除档位：仅删记录（文件保留）/ 移入系统回收站 */
export type DedupeDeleteMode = "records" | "trash";
