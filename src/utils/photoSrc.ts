/**
 * 可显示图片来源解析（FEAT-142）
 *
 * ## 为什么需要
 *
 * 大图/预览是直接把原文件路径交给 WebView2（`convertFileSrc`）渲染的。
 * WebView2 是 Chromium，**不认识 HEIC/HEIF** —— 相册里有 iPhone 照片时，
 * 网格（走缩略图）正常，点开大图却是一片空白。
 *
 * 因此 HEIC/HEIF 改走「后端渲染缓存 JPEG」这条路（`ensure_photo_preview`），
 * 其余格式仍直连原文件（零额外开销）。
 *
 * 结果按原图路径缓存，同一张照片反复打开只 invoke 一次。
 */
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

/** WebView 不认识的格式（需要后端渲染预览） */
const NEEDS_PREVIEW = /\.(heic|heif|hif)$/i;

const urlCache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

/**
 * HEIC 预览缓存条数上限：只缓存 HEIC（其余格式零开销直连），但长会话里
 * 「打开过多少张 HEIC」仍是无界的；按最近使用淘汰，命中时刷新顺序。
 */
const URL_CACHE_MAX = 200;

function putCached(path: string, url: string): void {
  if (urlCache.has(path)) urlCache.delete(path); // 重新插入 → 变成最新的
  urlCache.set(path, url);
  if (urlCache.size <= URL_CACHE_MAX) return;
  const oldest = urlCache.keys().next().value;
  if (oldest !== undefined) urlCache.delete(oldest);
}

/** 该路径是否必须走后端预览 */
export function needsPreview(path: string): boolean {
  return NEEDS_PREVIEW.test(path);
}

/** 解析出可直接放进 `<img src>` 的 URL；HEIC 走缓存预览。失败会抛错（调用方展示提示） */
export async function resolvePhotoSrc(path: string): Promise<string> {
  if (!path) return "";
  if (!needsPreview(path)) return convertFileSrc(path);
  const cached = urlCache.get(path);
  if (cached) {
    putCached(path, cached); // 命中刷新为最近使用
    return cached;
  }
  const running = inflight.get(path);
  if (running) return running;

  const task = invoke<string>("ensure_photo_preview", { path })
    .then((preview) => {
      const url = convertFileSrc(preview);
      putCached(path, url);
      return url;
    })
    .finally(() => {
      inflight.delete(path);
    });
  inflight.set(path, task);
  return task;
}

/** 清掉某个路径的缓存（原图被替换/删除后调用） */
export function invalidatePhotoSrc(path: string): void {
  urlCache.delete(path);
}
