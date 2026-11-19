/**
 * 媒体类型判定 —— 前端唯一真相点（与后端 `src-tauri/src/media_kind.rs` 规则对齐）
 *
 * ## 为什么前端也要有一份
 *
 * 相册内要分「图片 / 视频 / 动图」三栏、缩略图上要挂播放角标，这些都需要**在渲染时**
 * 立刻知道类型，不可能每张照片都回头调一次后端。所以前端保留一份只读判定。
 *
 * ## 与后端的一致性怎么保证
 *
 * 扩展名表必须与 `media_kind.rs` 的 `STILL_IMAGE_EXTS` / `ANIMATED_EXTS` /
 * `VIDEO_EXTS` 逐字一致；两边都有单测钉住同一批样例（`mediaKind.test.ts` 与
 * Rust 侧 `media_kind::tests`）。任一边加扩展名时**必须同步改另一边**。
 *
 * 为什么不再多存一个 `media_type` 数据库字段：路径已经唯一决定了类型，
 * 再存一列就要在「改名 / 移动 / 换扩展名」时同步维护，是纯粹的一致性负债。
 */

/** 静态图片扩展名（小写，不含点） */
export const STILL_IMAGE_EXTS = ["jpg", "jpeg", "png", "bmp", "heic", "heif", "tif", "tiff"] as const;

/** 动图扩展名（gif 与 webp） */
export const ANIMATED_EXTS = ["gif", "webp"] as const;

/**
 * 视频扩展名
 *
 * 故意不含 mkv / avi / 3gp：Tauri asset 协议嗅探不出 MIME 时会退回 text/html，
 * 浏览器拿到 text/html 直接拒播，收进来只会「能导入但打不开」。
 */
export const VIDEO_EXTS = ["mp4", "mov", "m4v", "wmv", "webm"] as const;

/** 媒体类型（字符串值与后端 `MediaKind::as_str()` 一致） */
export type MediaKind = "image" | "animated" | "video";

/** 取小写扩展名（不含点）；没有扩展名返回空串 */
export function extOf(path: string): string {
  const base = path.replace(/\\/g, "/").split("/").pop() ?? "";
  const i = base.lastIndexOf(".");
  return i <= 0 ? "" : base.slice(i + 1).toLowerCase();
}

/** 判定媒体类型；不是可视媒体返回 null */
export function mediaKindOf(path: string | null | undefined): MediaKind | null {
  if (!path) return null;
  const e = extOf(path);
  if ((VIDEO_EXTS as readonly string[]).includes(e)) return "video";
  if ((ANIMATED_EXTS as readonly string[]).includes(e)) return "animated";
  if ((STILL_IMAGE_EXTS as readonly string[]).includes(e)) return "image";
  return null;
}

/** 是否视频 */
export function isVideo(path: string | null | undefined): boolean {
  return mediaKindOf(path) === "video";
}

/** 是否动图 */
export function isAnimated(path: string | null | undefined): boolean {
  return mediaKindOf(path) === "animated";
}

/** 是否静态图片（严格） */
export function isStillImage(path: string | null | undefined): boolean {
  return mediaKindOf(path) === "image";
}

/** 相册内的媒体分栏 */
export type MediaFilter = "all" | MediaKind;

/** 分栏中文名 */
export const MEDIA_FILTER_LABELS: Record<MediaFilter, string> = {
  all: "全部",
  image: "图片",
  animated: "动图",
  video: "视频",
};

/** 按分栏过滤（"all" 原样返回，调用方不要就地改原数组） */
export function filterByMedia<T extends { path: string }>(
  items: readonly T[],
  filter: MediaFilter,
): T[] {
  if (filter === "all") return items as T[];
  return items.filter((it) => mediaKindOf(it.path) === filter);
}
