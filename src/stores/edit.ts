import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import * as engine from "../composables/useEditEngine";
import type { ExportFormat } from "../composables/useEditEngine";
import type {
  AdjustmentRecipe,
  EditHistogram,
  EditRenderMeta,
  EditSchema,
  MaskInfo,
} from "../types/edit";
import {
  cloneRecipe,
  emptyRecipe,
  isPristineRecipe,
  normalizeRecipe,
  withoutAdjustments,
} from "../types/edit";

/**
 * 创意工坊 · 基准调色的状态中枢（FEAT-099）
 * ===================================================================
 * 负责四件事，每件都是「容易出错所以必须集中到一处」的：
 *
 * 1. **会话**：当前打开的图片（id / 尺寸 / 预览 blob URL）；
 * 2. **渲染调度**：拖动滑块时的高频重渲。核心是**帧合并**与**草稿档降级**
 *    （见 `scheduleRender` 的注释）；
 * 3. **撤销栈**：配方的快照序列；
 * 4. **直方图**：节流更新（每帧重算直方图会白跑一整遍渲染）。
 *
 * ## 渲染调度为什么必须在这里做
 *
 * 前端各处直接 `invoke` 会出现两个经典问题：
 *
 * - **请求堆积**：拖动滑块每 16ms 触发一次，而一帧渲染 30~80ms。
 *   10 个请求会串行排队 800ms，表现为「松手后画面还在慢慢变」——
 *   用户看到的是一连串滞后的画面，而不是自己拖到的位置。
 * - **旧结果覆盖新结果**：请求乱序返回时，第 5 帧可能后于第 9 帧返回。
 *
 * 解法是 `renderToken`：每次调度 +1，`await` 回来后**丢弃 token 已过期
 * 的结果**；并且同一时刻只允许一个 `invoke` 在飞（`rendering` 标志），
 * 飞行期间的调度合并成一次「飞行结束后立刻补一帧」。
 * 后端另有一层序号校验（`session.rs`），两层配合才能同时挡住
 * 排队与乱序。
 */

/** 渲染档位。 */
export type RenderQuality = "draft" | "preview";

/** 撤销栈上限。超过后丢弃最旧的快照。 */
const HISTORY_LIMIT = 60;

/** 直方图节流间隔（毫秒）。直方图要多跑一遍渲染，太频繁会拖慢拖动。 */
const HISTOGRAM_THROTTLE_MS = 180;

/** 拖动结束后触发高画质重渲的静默期（毫秒）。 */
const SETTLE_DELAY_MS = 140;

export const useEditStore = defineStore("workshopEdit", () => {
  /* ---------------- schema ---------------- */
  const schema = ref<EditSchema | null>(null);
  const schemaError = ref("");
  const schemaReady = ref(false);

  /* ---------------- 会话 ---------------- */
  const sessionId = ref(0);
  const sourcePath = ref("");
  const originalWidth = ref(0);
  const originalHeight = ref(0);
  const previewWidth = ref(0);
  const previewHeight = ref(0);
  const subsampled = ref(false);
  const hasSidecar = ref(false);
  const decodeMs = ref(0);

  /** 当前显示的预览（object URL）。用 shallowRef：Blob 不需要深层响应。 */
  const previewUrl = ref("");
  const previewBlob = shallowRef<Blob | null>(null);
  const previewSize = ref({ w: 0, h: 0 });

  /* ---------------- 配方与历史 ---------------- */
  const recipe = ref<AdjustmentRecipe>(emptyRecipe());
  const undoStack = ref<AdjustmentRecipe[]>([]);
  const redoStack = ref<AdjustmentRecipe[]>([]);
  /**
   * 配方变化回调（FEAT-099 融合）。
   *
   * 调色参数一变，Python 算子链的输入就变了 → 结果作废。store 不该知道
   * 算子的存在（那是前端编排的事），因此用回调让宿主决定要不要重跑。
   *
   * 只在「提交后」触发（commit=true）：拖动中的每一帧都重跑整条链不可行
   * ——一次算子 100~300ms，拖一次滑块就是几十次调用。
   */
  const onRecipeCommitted = ref<(() => void) | null>(null);
  /** 正在拖动的滑块：拖动期间不往撤销栈塞快照（否则一次拖动 = 50 步撤销） */
  const interacting = ref(false);

  /* ---------------- 渲染状态 ---------------- */
  const loading = ref(false);
  const rendering = ref(false);
  const renderMs = ref(0);
  const renderError = ref("");
  const lastMeta = ref<EditRenderMeta | null>(null);
  /** 当前预览的实际档位（用于显示「预览为降采样画质」提示） */
  const quality = ref<RenderQuality>("preview");

  /* ---------------- 对比 ---------------- */
  /** 对比模式：编辑 / 原图（整幅显示原图）/ 并排 / 分屏 */
  const compareMode = ref<"off" | "original" | "side" | "split">("off");
  /** 分屏位置 0..1 */
  const splitX = ref(0.5);

  /**
   * 「零调整」渲染结果（对比视图的左格）。
   *
   * 懒加载 + 缓存：它只取决于**图像本身和裁剪框**，与调色/算子无关，
   * 所以同一张图最多渲染一次。放在打开照片时渲染是浪费 —— 大部分人
   * 根本不会点对比（实测这一帧要 1s 上下）。
   *
   * 不走 `drainPending*`：那套会写 `previewUrl`、抢 `renderToken`，
   * 对比用的这帧是旁路数据，串进去会把主预览顶掉。
   */
  const originalUrl = ref("");
  const originalBusy = ref(false);
  /** 缓存这份「零调整」时的裁剪签名，裁剪一变就得重渲染 */
  let originalCropSig = "";
  /** 丢弃在飞的「零调整」渲染 */
  let originalToken = 0;

  function dropOriginal(): void {
    originalToken++;
    originalBusy.value = false;
    originalCropSig = "";
    if (originalUrl.value) URL.revokeObjectURL(originalUrl.value);
    originalUrl.value = "";
  }

  /** 取「零调整」帧；没有就渲染一帧并缓存。返回 object URL（可能为空串）。 */
  async function ensureOriginal(): Promise<string> {
    if (!hasImage.value || disposed) return "";
    const sig = JSON.stringify(recipe.value.crop);
    if (originalUrl.value && originalCropSig === sig) return originalUrl.value;
    if (originalBusy.value) return originalUrl.value;

    originalBusy.value = true;
    const token = ++originalToken;
    try {
      const [blob] = await engine.renderFrame(
        sessionId.value,
        withoutAdjustments(recipe.value),
        false,
      );
      // 期间换图 / 又发起一次：作废，别把上一张的帧贴到这一张上
      if (token !== originalToken || disposed || !hasImage.value) return "";
      dropOriginalUrlOnly();
      originalUrl.value = URL.createObjectURL(blob);
      originalCropSig = sig;
      return originalUrl.value;
    } catch {
      // 对比是辅助功能，取不到就退回普通视图，不该弹错误打断编辑
      return "";
    } finally {
      if (token === originalToken) originalBusy.value = false;
    }
  }

  /** 只回收 URL，不动 token/签名（新帧即将填上）。 */
  function dropOriginalUrlOnly(): void {
    if (originalUrl.value) URL.revokeObjectURL(originalUrl.value);
    originalUrl.value = "";
  }

  /* ---------------- 裁剪交互 ---------------- */
  const cropping = ref(false);

  /* ---------------- 局部蒙版 ---------------- */
  /** 是否处于蒙版绘制模式（与裁剪模式互斥：两者都要独占画布手势） */
  const masking = ref(false);
  /** 蒙版是否存在（与 recipe.mask.hasImage 同步的便捷镜像） */
  const hasMask = computed(() => recipe.value.mask.hasImage);
  /** 蒙版参数改动中（拖动羽化/强度时不记撤销） */
  const maskInfo = ref<MaskInfo | null>(null);
  const maskError = ref("");

  /* ---------------- 直方图 ---------------- */
  const histogram = ref<EditHistogram | null>(null);
  let histogramTimer = 0;

  /* ---------------- 调度内部状态 ---------------- */
  let renderToken = 0;
  let pendingRecipe: AdjustmentRecipe | null = null;
  let pendingDraft = false;
  let settleTimer = 0;
  let disposed = false;

  /* ================================================================
   * 派生状态
   * ================================================================ */

  const hasImage = computed(() => sessionId.value !== 0);
  const pristine = computed(() => isPristineRecipe(recipe.value));
  const canUndo = computed(() => undoStack.value.length > 0);
  const canRedo = computed(() => redoStack.value.length > 0);

  /** 与「原图」是否一致 —— 用于裁剪/对比按钮的可用态 */
  const differsFromOriginal = computed(() => !pristine.value);

  /* ================================================================
   * schema
   * ================================================================ */

  async function ensureSchema(): Promise<void> {
    if (schemaReady.value && schema.value) return;
    schemaError.value = "";
    try {
      schema.value = await engine.fetchSchema();
      schemaReady.value = true;
    } catch (e) {
      schemaError.value = String((e as Error)?.message ?? e);
    }
  }

  /* ================================================================
   * 会话
   * ================================================================ */

  /** 打开图片。返回是否成功（失败时已写入 `renderError`）。 */
  async function open(path: string): Promise<boolean> {
    loading.value = true;
    renderError.value = "";
    // 先释放上一个会话：不清的话预览平面 + 工作区会占着 40MB+
    await releaseSession();

    try {
      const r = await engine.openSession(path);
      sessionId.value = r.sessionId;
      sourcePath.value = path;
      originalWidth.value = r.width;
      originalHeight.value = r.height;
      previewWidth.value = r.previewWidth;
      previewHeight.value = r.previewHeight;
      subsampled.value = r.subsampled;
      hasSidecar.value = r.hasRecipe;
      decodeMs.value = r.decodeMs;
      recipe.value = normalizeRecipe(r.recipe);
      undoStack.value = [];
      redoStack.value = [];
      compareMode.value = "off";
      cropping.value = false;
      masking.value = false;
      histogram.value = null;
      maskError.value = "";
      await renderNow("preview");
      await refreshHistogram();
      // 蒙版单独取：它在 sidecar 之外（独立 PNG），不进配方
      await loadMaskInfo();
      return true;
    } catch (e) {
      renderError.value = String((e as Error)?.message ?? e);
      return false;
    } finally {
      loading.value = false;
    }
  }

  /** 关闭会话并清理 object URL。 */
  async function close(): Promise<void> {
    await releaseSession();
    sourcePath.value = "";
    recipe.value = emptyRecipe();
    undoStack.value = [];
    redoStack.value = [];
    histogram.value = null;
    previewSize.value = { w: 0, h: 0 };
  }

  async function releaseSession(): Promise<void> {
    // 丢弃在飞的渲染结果，避免它回来后把已释放的 URL 又填上
    renderToken++;
    if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
    previewUrl.value = "";
    previewBlob.value = null;
    dropOriginal();
    if (sessionId.value !== 0) {
      const id = sessionId.value;
      sessionId.value = 0;
      try {
        await engine.closeSession(id);
      } catch {
        /* 会话可能已因淘汰而不存在，忽略 */
      }
    }
  }

  /* ================================================================
   * 渲染
   * ================================================================ */

  /**
   * 调度一帧渲染。
   *
   * @param q 草稿档用于拖动中；正常档用于静止后
   *
   * 合并规则：若已有一帧在飞，则只更新 `pendingRecipe`，
   * 由飞行结束后的 `drainPending` 补渲 —— 飞行中的请求无法取消，
   * 但它结束后**必定**会补一帧最新的，所以不会丢用户最后一次操作。
   */
  async function scheduleRender(q: RenderQuality): Promise<void> {
    if (!hasImage.value || disposed) return;
    pendingRecipe = cloneRecipe(recipe.value);
    pendingDraft = q === "draft";
    if (rendering.value) return;
    await drainPending();
  }

  async function drainPending(): Promise<void> {
    if (rendering.value || !pendingRecipe || disposed) return;
    const target = pendingRecipe;
    const draft = pendingDraft;
    pendingRecipe = null;

    rendering.value = true;
    const token = ++renderToken;
    try {
      const [blob, meta] = await engine.renderFrame(sessionId.value, target, draft);
      // 后序号校验（前端）：期间又调度了更新的结果，丢弃本次
      if (token !== renderToken || disposed || !hasImage.value) return;
      // 后端也可能判定过期（与前端 flight 号不是同一套，互为兜底）
      if (meta.stale) return;
      if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
      const url = URL.createObjectURL(blob);
      previewUrl.value = url;
      previewBlob.value = blob;
      previewSize.value = { w: meta.width, h: meta.height };
      renderMs.value = meta.elapsedMs;
      lastMeta.value = meta;
      quality.value = draft ? "draft" : "preview";
      renderError.value = "";
    } catch (e) {
      if (token !== renderToken) return;
      renderError.value = String((e as Error)?.message ?? e);
    } finally {
      rendering.value = false;
      // 飞行期间来了新调度 → 立刻补上
      if (pendingRecipe && !disposed) {
        // 不 await：让它在后台继续，避免拖动事件里堆积微任务
        void drainPending();
      }
    }
  }

  /** 同步渲染一帧（打开图片 / 导入配方后用，不走合并逻辑）。 */
  async function renderNow(q: RenderQuality): Promise<void> {
    pendingRecipe = null;
    await drainPendingForce(q);
  }

  async function drainPendingForce(q: RenderQuality): Promise<void> {
    if (!hasImage.value || disposed) return;
    rendering.value = true;
    const token = ++renderToken;
    try {
      const [blob, meta] = await engine.renderFrame(
        sessionId.value,
        cloneRecipe(recipe.value),
        q === "draft",
      );
      if (token !== renderToken || disposed || !hasImage.value) return;
      if (meta.stale) return;
      if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
      previewUrl.value = URL.createObjectURL(blob);
      previewBlob.value = blob;
      previewSize.value = { w: meta.width, h: meta.height };
      renderMs.value = meta.elapsedMs;
      lastMeta.value = meta;
      quality.value = q;
      renderError.value = "";
    } catch (e) {
      if (token === renderToken) renderError.value = String((e as Error)?.message ?? e);
    } finally {
      rendering.value = false;
    }
  }

  /**
   * 配方变更入口：所有 UI 都调它，不直接改 `recipe`。
   *
   * @param commit `false` 表示「拖动中」（不记撤销、走低画质）；
   *                `true` 表示「一次离散操作结束」（记撤销、走高画质）
   */
  async function applyRecipe(next: AdjustmentRecipe, commit: boolean): Promise<void> {
    const prev = recipe.value;
    recipe.value = next;
    if (commit) {
      if (!interacting.value) pushHistory(prev);
      redoStack.value = [];
      interacting.value = false;
    }
    await scheduleRender(commit ? "preview" : "draft");
    if (commit) {
      // 拖动结束后稍等一下再出高画质图：用户往往连续拖几个滑块，
      // 每次都出一次 1600px 会浪费；静默期内合并成一次
      window.clearTimeout(settleTimer);
      settleTimer = window.setTimeout(() => {
        void scheduleRender("preview");
        void scheduleHistogram();
      }, SETTLE_DELAY_MS);
      // 通知宿主：算子链的输入变了，结果应作废并（静默期后）重跑
      onRecipeCommitted.value?.();
    }
  }

  /** 注册「配方提交后」回调（宿主用它触发算子链重跑）。返回取消函数。 */
  function onCommit(fn: () => void): () => void {
    onRecipeCommitted.value = fn;
    return () => {
      if (onRecipeCommitted.value === fn) onRecipeCommitted.value = null;
    };
  }

  /** 拖动开始：冻结历史记录。 */
  function beginInteraction(): void {
    interacting.value = true;
  }

  /** 拖动结束：把整段拖动记成一步撤销。 */
  async function endInteraction(next: AdjustmentRecipe): Promise<void> {
    const prev = recipe.value;
    recipe.value = next;
    pushHistory(prev);
    redoStack.value = [];
    interacting.value = false;
    await scheduleRender("preview");
    void scheduleHistogram();
  }

  function pushHistory(snapshot: AdjustmentRecipe): void {
    undoStack.value.push(snapshot);
    if (undoStack.value.length > HISTORY_LIMIT) undoStack.value.shift();
  }

  async function undo(): Promise<void> {
    const prev = undoStack.value.pop();
    if (!prev) return;
    redoStack.value.push(cloneRecipe(recipe.value));
    recipe.value = prev;
    await renderNow("preview");
    void scheduleHistogram();
  }

  async function redo(): Promise<void> {
    const next = redoStack.value.pop();
    if (!next) return;
    undoStack.value.push(cloneRecipe(recipe.value));
    recipe.value = next;
    await renderNow("preview");
    void scheduleHistogram();
  }

  /** 重置为原图（只改内存中的配方；删除 sidecar 由 `resetPersisted` 做）。 */
  async function resetToOriginal(): Promise<void> {
    const prev = cloneRecipe(recipe.value);
    recipe.value = emptyRecipe();
    pushHistory(prev);
    redoStack.value = [];
    await renderNow("preview");
    void scheduleHistogram();
  }

  /* ================================================================
   * 其它配方操作
   * ================================================================ */

  async function autotune(): Promise<void> {
    if (!hasImage.value) return;
    try {
      const prev = cloneRecipe(recipe.value);
      const next = await engine.autotune(sessionId.value);
      recipe.value = next;
      pushHistory(prev);
      redoStack.value = [];
      await renderNow("preview");
      void scheduleHistogram();
    } catch (e) {
      renderError.value = String((e as Error)?.message ?? e);
    }
  }

  async function persist(): Promise<string> {
    const path = await engine.saveRecipe(sessionId.value, recipe.value);
    hasSidecar.value = true;
    return path;
  }

  async function resetPersisted(): Promise<void> {
    await engine.resetRecipe(sessionId.value);
    hasSidecar.value = false;
  }

  async function importText(text: string): Promise<void> {
    const next = await engine.recipeFromText(text);
    const prev = cloneRecipe(recipe.value);
    recipe.value = next;
    pushHistory(prev);
    redoStack.value = [];
    await renderNow("preview");
    void scheduleHistogram();
  }

  async function exportText(): Promise<string> {
    return await engine.recipeToText(recipe.value);
  }

  async function queryExportSize(
    format: ExportFormat,
    quality: number,
    maxLongEdge: number,
  ): Promise<[number, number, Blob]> {
    const [blob, w, h] = await engine.encodeExport(sessionId.value, recipe.value, {
      format,
      quality,
      maxLongEdge,
    });
    return [w, h, blob];
  }

  /* ================================================================
   * 直方图
   * ================================================================ */

  function scheduleHistogram(): void {
    if (!hasImage.value) return;
    window.clearTimeout(histogramTimer);
    histogramTimer = window.setTimeout(() => void refreshHistogram(), HISTOGRAM_THROTTLE_MS);
  }

  async function refreshHistogram(): Promise<void> {
    if (!hasImage.value || disposed) return;
    try {
      histogram.value = await engine.renderHistogram(sessionId.value, recipe.value);
    } catch {
      /* 直方图失败不影响主流程（只影响那一条装饰性的图） */
    }
  }

  /* ================================================================
   * 局部蒙版
   * ================================================================ */

  /**
   * 提交一份蒙版 PNG。
   *
   * 先落盘/入库再改配方：反过来的话，一旦渲染失败，配方里已经标了
   * `hasImage` 却拿不到像素，选区会静默消失。
   *
   * @param png 预览分辨率的灰度 PNG
   * @param keepDisabled 清除后仍保持「有蒙版」语义（内部用）
   */
  async function commitMask(png: ArrayBuffer): Promise<void> {
    if (!hasImage.value) return;
    maskError.value = "";
    try {
      const info = await engine.setMask(sessionId.value, png);
      maskInfo.value = info;
      const prev = cloneRecipe(recipe.value);
      const next = cloneRecipe(recipe.value);
      next.mask = { ...next.mask, hasImage: true, enabled: true };
      recipe.value = next;
      pushHistory(prev);
      redoStack.value = [];
      await renderNow("preview");
      void scheduleHistogram();
    } catch (e) {
      maskError.value = String((e as Error)?.message ?? e);
    }
  }

  /** 清除蒙版（删文件 + 退回整图）。 */
  async function clearMask(): Promise<void> {
    if (!hasImage.value) return;
    maskError.value = "";
    try {
      await engine.clearMask(sessionId.value);
      maskInfo.value = null;
      const prev = cloneRecipe(recipe.value);
      const next = cloneRecipe(recipe.value);
      next.mask = { ...next.mask, hasImage: false, enabled: false };
      recipe.value = next;
      pushHistory(prev);
      redoStack.value = [];
      await renderNow("preview");
      void scheduleHistogram();
    } catch (e) {
      maskError.value = String((e as Error)?.message ?? e);
    }
  }

  /**
   * 改蒙版参数（羽化 / 强度 / 反相 / 开关）。
   *
   * 与滑块同一套约定：`commit=false` 表示拖动中（不记撤销、走低画质）。
   */
  async function applyMaskParams(patch: Partial<AdjustmentRecipe["mask"]>, commit: boolean): Promise<void> {
    if (!hasImage.value) return;
    const prev = recipe.value;
    const next = cloneRecipe(recipe.value);
    next.mask = { ...next.mask, ...patch };
    recipe.value = next;
    if (commit) {
      if (!interacting.value) pushHistory(prev);
      redoStack.value = [];
      interacting.value = false;
    }
    await scheduleRender(commit ? "preview" : "draft");
    if (commit) void scheduleHistogram();
  }

  /** 打开图片后把已有蒙版取回来（供画布恢复）。 */
  /** 从磁盘读回的蒙版 PNG 字节，供画布恢复。没有则 null。 */
  const maskPng = ref<ArrayBuffer | null>(null);

  async function loadMaskInfo(): Promise<void> {
    if (!hasImage.value) {
      maskInfo.value = null;
      maskPng.value = null;
      return;
    }
    const r = await engine.getMask(sessionId.value);
    maskInfo.value = r.info;
    maskPng.value = r.png;
  }

  /* ================================================================
   * 生命周期
   * ================================================================ */

  function dispose(): void {
    disposed = true;
    window.clearTimeout(settleTimer);
    window.clearTimeout(histogramTimer);
    if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
    previewUrl.value = "";
    previewBlob.value = null;
    void engine.closeAllSessions();
  }

  /** 供组件 onMounted 时恢复可编辑（dispose 后重新打开图片） */
  function revive(): void {
    disposed = false;
  }

  return {
    // schema
    schema,
    schemaError,
    schemaReady,
    ensureSchema,
    // 派生状态（模板里通过 store.xxx 访问，故必须显式导出）
    hasImage,
    pristine,
    canUndo,
    canRedo,
    differsFromOriginal,
    // 会话
    sessionId,
    sourcePath,
    originalWidth,
    originalHeight,
    previewWidth,
    previewHeight,
    subsampled,
    hasSidecar,
    decodeMs,
    previewUrl,
    previewBlob,
    previewSize,
    // 配方
    recipe,
    onCommit,
    // 渲染
    loading,
    rendering,
    renderNow,
    renderMs,
    renderError,
    quality,
    lastMeta,
    // 对比
    compareMode,
    splitX,
    originalUrl,
    originalBusy,
    ensureOriginal,
    cropping,
    // 局部蒙版
    masking,
    hasMask,
    maskInfo,
    maskPng,
    maskError,
    commitMask,
    clearMask,
    applyMaskParams,
    loadMaskInfo,
    // 直方图
    histogram,
    // 动作
    open,
    close,
    applyRecipe,
    beginInteraction,
    endInteraction,
    undo,
    redo,
    resetToOriginal,
    autotune,
    persist,
    resetPersisted,
    importText,
    exportText,
    queryExportSize,
    refreshHistogram,
    scheduleHistogram,
    dispose,
    revive,
  };
});