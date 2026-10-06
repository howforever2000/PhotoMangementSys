// 相册相关类型定义
//
// 严格对应 Rust 侧 `src-tauri/src/db/mod.rs` 中的结构体字段，
// 保证前端 invoke 调用的参数与返回值类型安全。

/** FEAT-095：相册类型 —— 对应 Rust `db::ALBUM_KIND_*` */
export type AlbumKind = "normal" | "archived" | "hidden";

/** FEAT-095：普通相册类型（`kind` 缺省值，历史行为不变） */
export const ALBUM_KIND_NORMAL: AlbumKind = "normal";

/** 判断是否为虚拟相册（归档 / 隐藏）：无真实文件夹，读照片走标记表 */
export function isVirtualAlbum(album: Pick<Album, "kind">): boolean {
  return album.kind === "archived" || album.kind === "hidden";
}

/**
 * FEAT-095：相册管理可见性过滤（决策 D3 + 隐含需求 I8）
 *
 * 后端 `get_albums` 会把归档 / 隐藏两个系统相册一并返回（前端无法靠「有没有这张卡」
 * 判断该不该显示，因为权限判断在前端）。这里做唯一入口的过滤：
 * 1. **隐藏相册**：`hiddenUnlocked` 为 false 时不可见（决策 D3：解锁后本次会话一直可见）；
 * 2. **空虚拟相册**：归档 / 隐藏里 0 张照片时不展示（隐含需求 I8）——
 *    否则相册管理一进来就杵着两个空壳卡片，用户会以为是 bug。
 *
 * 普通相册永远不受影响，历史行为零变更。
 */
export function filterVisibleAlbums(albums: Album[], hiddenUnlocked: boolean): Album[] {
  return albums.filter((a) => {
    if (a.kind === "hidden" && !hiddenUnlocked) return false;
    if (isVirtualAlbum(a) && a.photo_count <= 0) return false;
    return true;
  });
}

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
  /** FEAT-095：相册类型
   *  - `"normal"`：普通相册，绑定真实本地文件夹
   *  - `"archived"`：归档相册（虚拟，存被归档的照片，相册管理中显示为「归档」）
   *  - `"hidden"`：隐藏相册（虚拟，存被隐藏的照片，需个人信息输密码解锁后才可见） */
  kind: AlbumKind;
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
