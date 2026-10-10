import { invoke } from "@tauri-apps/api/core";
import type {
  AdjustmentRecipe,
  EditHistogram,
  EditOpenResult,
  EditRenderMeta,
  EditSchema,
  MaskInfo,
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
 *
 * ## 为什么需要 unpackFrame
 *
 * Tauri v2 的 `IpcResponse` 只有 `Serialize` 与 `Response` 两个 impl，
 * **没有元组的 impl**（v1 的 `impl_tuple!` 在 v2 移除了）。所以 Rust 侧
 * 无法返回 `(Response, Meta)` 这种「裸字节 + 结构体」的组合 —— 那样编译
 * 不过（E0599）。后端因此把元信息与字节打包进同一个响应体，帧格式：
 *
 * ```text
 * [0..4)      u32 LE = meta JSON 字节长度 L
 * [4..4+L)    meta 的 UTF-8 JSON
 * [4+L..)     图片字节
 * ```
 *
 * 对端实现见 `src-tauri/src/edit/commands.rs` 的 `pack_frame`，
 * 两处必须同步修改。
 */

/** 拆开后端 `pack_frame` 打包的帧，返回 `[图片字节, 元信息]`。 */
function unpackFrame<M>(buf: ArrayBuffer): [Uint8Array, M] {
  const metaLen = new DataView(buf).getUint32(0, true);
  const metaJson = new Uint8Array(buf, 4, metaLen);
  const meta = JSON.parse(new TextDecoder().decode(metaJson)) as M;
  return [new Uint8Array(buf, 4 + metaLen), meta];
}

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
  const buf = await invoke<ArrayBuffer>("edit_render", {
    sessionId,
    recipe,
    draft,
  });
  const [bytes, meta] = unpackFrame<EditRenderMeta>(buf);
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
  const buf = await invoke<ArrayBuffer>("edit_encode", {
    sessionId,
    recipe,
    format: opts.format,
    quality: opts.quality,
    maxLongEdge: opts.maxLongEdge,
  });
  const [bytes, meta] = unpackFrame<{ width: number; height: number }>(buf);
  return [
    new Blob([bytes], { type: opts.format === "png" ? "image/png" : "image/jpeg" }),
    meta.width,
    meta.height,
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

/* ================================================================
 * 局部蒙版
 * ================================================================
 *
 * 蒙版像素**不进配方**：配方是可读可搬运的 JSON，而蒙版是位图。
 * 像素走独立的三个命令（存 / 取 / 清），配方里只留参数与
 * 「有没有蒙版」这一个事实。
 */

/**
 * 上传蒙版 PNG（覆盖既有蒙版）。
 *
 * 蒙版应画在**预览分辨率**上：这样文件不随原图尺寸膨胀，导出时
 * 由后端按需缩放（最近邻，见 `mask.rs`）。
 */
export async function setMask(sessionId: number, png: ArrayBuffer): Promise<MaskInfo> {
  return await invoke<MaskInfo>("edit_mask_set", {
    sessionId,
    png: Array.from(new Uint8Array(png)),
  });
}

/**
 * 取回当前蒙版的 PNG 字节与信息（重新打开图片后恢复画布用）。
 *
 * 后端用 `pack_frame` 打包（`[长度][JSON][PNG]`）：蒙版 PNG 约 2.5MB，
 * 若走 JSON 字节数组会膨胀到 10MB 且解析极慢。
 *
 * 没有蒙版时返回 `{ info: null, png: null }` —— 「还没蒙版」是正常状态。
 */
export async function getMask(
  sessionId: number,
): Promise<{ info: MaskInfo | null; png: ArrayBuffer | null }> {
  const buf = await invoke<ArrayBuffer>("edit_mask_get", { sessionId });
  const [payload, meta] = unpackFrame<MaskInfo>(buf);
  // width=0 是后端给的「无蒙版」哨兵
  if (!meta || meta.width === 0) return { info: null, png: null };
  return { info: meta, png: payload.buffer as ArrayBuffer };
}

/** 清除蒙版（删文件 + 清会话缓存）。幂等。 */
export async function clearMask(sessionId: number): Promise<void> {
  await invoke<void>("edit_mask_clear", { sessionId });
}