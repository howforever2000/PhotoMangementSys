import { invoke } from "@tauri-apps/api/core";
import type {
  AdjustmentRecipe,
  EditHistogram,
  EditOpenResult,
  EditRenderMeta,
  EditSchema,
} from "../types/edit";
import { normalizeRecipe } from "../types/edit";

/**
 * 非破坏图像编辑引擎的 Tauri 命令封装（FEAT-099）
 * ===================================================================
 * 本文件是前端访问 Rust 编辑引擎的**唯一**通道。不做状态管理、不做重试、
 * 不做缓存 —— 那是 `stores/edit.ts` 的职责。两层分开的理由：
 * 命令签名（字段名、类型）变化时只会波及这一个文件。
 *
 * ## 二进制返回的处理
 *
 * 后端的渲染/编码结果走 `tauri::ipc::Response`（裸字节），在 JS 侧是
 * `ArrayBuffer`。这里统一转成 `Blob` + `objectURL`，由调用方负责
 * `URL.revokeObjectURL` —— 见 `stores/edit.ts` 的 `render`。
 */

/** 拉取参数表（面板渲染的唯一依据）。 */
export async function fetchSchema(): Promise<EditSchema> {
  return await invoke<EditSchema>("edit_schema");
}

/** 打开图片并建立编辑会话。 */
export async function openSession(
  path: string,
  previewLongEdge?: number,
): Promise<EditOpenResult> {
  const r = await invoke<EditOpenResult>("edit_open", {
    path,
    previewLongEdge: previewLongEdge ?? null,
  });
  // 配方必须归一化：后端可能来自旧版 sidecar，字段未必齐全
  return { ...r, recipe: normalizeRecipe(r.recipe) };
}

/** 关闭会话（切换/关闭图片时调用，释放预览平面与工作区内存）。 */
export async function closeSession(sessionId: number): Promise<boolean> {
  return await invoke<boolean>("edit_close", { sessionId });
}

/** 一次关闭全部会话（编辑器整体卸载时调用）。 */
export async function closeAllSessions(): Promise<number> {
  return await invoke<number>("edit_close_all");
}

/**
 * 渲染一帧。
 *
 * @param draft 拖动滑块过程中传 `true`（长边 800）；静止后传 `false`（长边 1600）
 * @returns `[Blob, meta]`
 */
export async function renderFrame(
  sessionId: number,
  recipe: AdjustmentRecipe,
  draft: boolean,
): Promise<[Blob, EditRenderMeta]> {
  const [bytes, meta] = await invoke<[ArrayBuffer, EditRenderMeta]>("edit_render", {
    sessionId,
    recipe,
    draft,
  });
  return [new Blob([bytes], { type: "image/jpeg" }), meta];
}

/** 渲染并取直方图。 */
export async function renderHistogram(
  sessionId: number,
  recipe: AdjustmentRecipe,
): Promise<EditHistogram> {
  const h = await invoke<EditHistogram>("edit_histogram", { sessionId, recipe });
  return {
    rgb: h.rgb ?? [],
    luma: h.luma ?? [],
    clippedLow: h.clippedLow ?? 0,
    clippedHigh: h.clippedHigh ?? 0,
    width: h.width,
    height: h.height,
  };
}

/** 自动优化（从直方图给出保守的起始值）。 */
export async function autotune(sessionId: number): Promise<AdjustmentRecipe> {
  const r = await invoke<AdjustmentRecipe>("edit_autotune", { sessionId });
  return normalizeRecipe(r);
}

/** 保存配方到 sidecar，返回配方文件路径。 */
export async function saveRecipe(sessionId: number, recipe: AdjustmentRecipe): Promise<string> {
  return await invoke<string>("edit_save_recipe", { sessionId, recipe });
}

/** 读取 sidecar（外部改动后刷新）。 */
export async function loadRecipe(sessionId: number): Promise<AdjustmentRecipe | null> {
  const r = await invoke<AdjustmentRecipe | null>("edit_load_recipe", { sessionId });
  return r ? normalizeRecipe(r) : null;
}

/** 删除 sidecar（重置为原图）。 */
export async function resetRecipe(sessionId: number): Promise<void> {
  await invoke<void>("edit_reset_recipe", { sessionId });
}

/** 导出格式。 */
export type ExportFormat = "jpeg" | "png";

/** 导出参数。 */
export interface ExportOptions {
  format: ExportFormat;
  /** JPEG 质量 50..100 */
  quality: number;
  /** 长边限制，0 = 原始分辨率 */
  maxLongEdge: number;
}

/** 导出编码，返回 `[Blob, width, height]`。 */
export async function encodeExport(
  sessionId: number,
  recipe: AdjustmentRecipe,
  opts: ExportOptions,
): Promise<[Blob, number, number]> {
  const [bytes, w, h] = await invoke<[ArrayBuffer, number, number]>("edit_encode", {
    sessionId,
    recipe,
    format: opts.format,
    quality: opts.quality,
    maxLongEdge: opts.maxLongEdge,
  });
  return [
    new Blob([bytes], { type: opts.format === "png" ? "image/png" : "image/jpeg" }),
    w,
    h,
  ];
}

/** 查询导出尺寸（保存前让用户看到最终尺寸）。 */
export async function queryOutputSize(
  sessionId: number,
  recipe: AdjustmentRecipe,
  maxLongEdge: number,
): Promise<[number, number]> {
  return await invoke<[number, number]>("edit_output_size", { sessionId, recipe, maxLongEdge });
}

/** 把字节写到指定路径。 */
export async function writeFile(path: string, data: ArrayBuffer): Promise<string> {
  return await invoke<string>("edit_write_file", { path, data: Array.from(new Uint8Array(data)) });
}

/** 配方 → JSON 文本（导出/复制）。 */
export async function recipeToText(recipe: AdjustmentRecipe): Promise<string> {
  return await invoke<string>("edit_recipe_to_text", { recipe });
}

/** JSON 文本 → 配方（导入/粘贴）。 */
export async function recipeFromText(text: string): Promise<AdjustmentRecipe> {
  const r = await invoke<AdjustmentRecipe>("edit_recipe_from_text", { text });
  return normalizeRecipe(r);
}

/** 会话内存统计（排错用）。 */
export async function sessionStats(): Promise<{ sessions: number; bytes: number; maxSessions: number }> {
  return await invoke<{ sessions: number; bytes: number; maxSessions: number }>("edit_session_stats");
}