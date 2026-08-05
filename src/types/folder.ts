/** 手动排序分组（文件夹）—— 对应 Rust folder::Folder */
export interface Folder {
  id: number;
  name: string;
  parent_id: number | null;
  level: number; // 1/2/3
  sort_order: number;
  description: string | null;
  tags: string[];
}

/** 文件夹 → 相册关联 */
export interface FolderAlbumEntry {
  folder_id: number;
  album_ids: number[];
}

/** 顶级相册（不属于任何文件夹） */
export interface RootAlbumEntry {
  album_id: number;
  sort_order: number;
}

/** 手动排序整体结构 —— 对应 Rust folder::ManualTree */
export interface ManualTree {
  folders: Folder[];
  folder_albums: FolderAlbumEntry[];
  root_albums: RootAlbumEntry[];
}
