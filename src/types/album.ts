// 相册相关类型定义
//
// 严格对应 Rust 侧 `src-tauri/src/db/mod.rs` 中的结构体字段，
// 保证前端 invoke 调用的参数与返回值类型安全。

/** 相册实体 —— 对应 Rust `db::Album` / 需求 §3.1 */
export interface Album {
  id: number;
  /** 相册显示名称 */
  name: string;
  /** 绑定的本地文件夹绝对路径 */
  path: string;
  /** 相册简介，可空 */
  description: string | null;
  /** 封面图片绝对路径，可空 */
  cover_path: string | null;
  /** 创建时间戳（Unix 秒） */
  created_at: number;
  /** 最后更新时间戳（Unix 秒） */
  updated_at: number;
  /** 相册内照片数量（从文件系统统计） */
  photo_count: number;
  /** 相册拍摄时间（相册内图片 EXIF，格式 YYYY-MM-DD） */
  shoot_time: string | null;
  /** 相册文件夹总大小（字节） */
  size_bytes: number;
  /** FEAT-036：该相册已扫描入库的照片数（photo_content_scan 行数）。
   *  > 0 表示该相册「已入库」（已有照片被内容扫描识别）；0 表示未入库。 */
  scanned_photo_count: number;
  /** 相册地点标签（手动设置） */
  location: string | null;
  /** 相册标签（最多 5 个） */
  tags: string[];
  /** 所属分组 ID（手动排序） */
  folder_id: number | null;
  /** 所属分组完整路径（如 "旅行/欧洲/巴黎"） */
  folder_path: string;
  /** 合并来源相册列表（FEAT-A）
   *  记录该相册历史上被合并进来的源相册（id / name / path），
   *  用于相册卡片下显示「由 X 个相册合并而来」，每条路径可点击跳转。
   *  后端在 get_albums / get_album / search_albums 时填充；非合并产物为空数组。 */
  merged_sources: MergedSource[];
}

/** 单个合并来源条目（FEAT-A） */
export interface MergedSource {
  id: number;
  name: string;
  path: string;
}

/** 将字节数格式化为可读大小（KB/MB/GB） */
export function formatSize(bytes: number): string {
  if (bytes <= 0) return "0 MB";
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    return `${(mb / 1024).toFixed(2)} GB`;
  }
  return `${mb.toFixed(1)} MB`;
}

/** 创建相册输入 —— 对应 Rust `db::CreateAlbumInput` */
export interface CreateAlbumInput {
  name: string;
  path: string;
  description?: string | null;
}

/** 更新相册输入 —— 对应 Rust `db::UpdateAlbumInput` */
export interface UpdateAlbumInput {
  id: number;
  name?: string;
  description?: string;
  cover_path?: string;
  /** 地点标签（空字符串清除） */
  location?: string;
}

/** 批量整理结果 —— 对应 Rust `BatchAlbumOutcome` */
export interface BatchAlbumOutcome {
  requested: number;
  ok: number;
  failed: number;
  failed_ids: number[];
}

/** 相册合并结果 —— 对应 Rust `MergeAlbumOutcome` */
export interface MergeAlbumOutcome {
  requested: number;
  merged: number;
  files_moved: number;
  files_failed: number;
  skipped: number[];
  failed_ids: number[];
  target_id: number;
}
