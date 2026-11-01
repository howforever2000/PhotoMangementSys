// 入库体检类型定义
//
// 严格对应 Rust 侧 `src-tauri/src/album_health.rs` 的结构体字段，
// 保证 invoke 调用的参数与返回值类型安全（改任一字段两侧必须同步）。

/** 单个相册的体检事实（后端只给事实，结论由 utils/albumHealth.ts 推导） */
export interface AlbumHealthRow {
  album_id: number;
  name: string;
  path: string;
  /** `normal` / `archived` / `hidden`（虚拟相册不参与体检，正常不会出现） */
  kind: string;
  /** 磁盘上的照片数（相册统计缓存口径，不触发重扫） */
  photo_count: number;
  /** 磁盘占用字节 */
  size_bytes: number;
  /** 已入库行数（按目录前缀在全库路径上匹配） */
  scanned: number;
  /** 有缩略图缓存的照片数 */
  thumbs: number;
  /** 有图像向量的照片数 */
  embeddings: number;
  /** 缩略图缓存占用字节 */
  thumb_bytes: number;
  /** 相册根目录是否还在 */
  root_exists: boolean;
  /** 抽样检查了多少条已入库路径 */
  sampled: number;
  /** 抽样里文件已不存在的条数 */
  missing_files: number;
}

/** 一次体检的整份报告（对应 Rust `HealthReport`） */
export interface HealthReport {
  albums: AlbumHealthRow[];
  album_count: number;
  total_photos: number;
  total_scanned: number;
  total_thumbs: number;
  total_embeddings: number;
  thumb_bytes: number;
  /** 每册抽样上限（前端据此解释"抽样"含义） */
  sampled_per_album: number;
  /** 根目录已不存在的相册数 */
  missing_roots: number;
  elapsed_ms: number;
}
