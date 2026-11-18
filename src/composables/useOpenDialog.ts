/**
 * 参考图 / 批量图片选择（FEAT-168）
 *
 * 原来「选参考图」的 openDialog 有 3 份实现（StyleTransferTool / ColorTransferTool /
 * AiWorkbench），各写一遍 filters 与多选取值。收敛到这一处，视图只管拿结果。
 */
import { open as openDialog } from "@tauri-apps/plugin-dialog";

export const IMAGE_EXTENSIONS = ["jpg", "jpeg", "png", "webp", "bmp", "gif", "heic", "heif"];

/** 选一张参考图；返回绝对路径或 null。 */
export async function pickReference(): Promise<string | null> {
  const picked = await openDialog({
    multiple: false,
    directory: false,
    title: "选择参考图",
    filters: [{ name: "图片", extensions: IMAGE_EXTENSIONS }],
  });
  return typeof picked === "string" ? picked : null;
}

/** 选多张照片；返回去重后的绝对路径数组。 */
export async function pickPhotos(title = "选择照片（可多选）"): Promise<string[]> {
  const picked = await openDialog({
    multiple: true,
    directory: false,
    title,
    filters: [{ name: "图片", extensions: IMAGE_EXTENSIONS }],
  });
  if (!picked) return [];
  const list = Array.isArray(picked) ? picked : [picked];
  return [...new Set(list.filter((p): p is string => typeof p === "string"))];
}

export async function pickDirectory(title = "选择文件夹"): Promise<string | null> {
  const picked = await openDialog({ multiple: false, directory: true, title });
  return typeof picked === "string" ? picked : null;
}

export { openDialog };
