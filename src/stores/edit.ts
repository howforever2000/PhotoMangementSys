import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import * as engine from "../composables/useEditEngine";
import type { ExportFormat } from "../composables/useEditEngine";
import type {
  AdjustmentRecipe,
  EditHistogram,
  EditRenderMeta,
  EditSchema,
} from "../types/edit";
import { cloneRecipe, emptyRecipe, isPristineRecipe, normalizeRecipe } from "../types/edit";

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
  /** 对比模式：编辑 / 原图（整幅显示原图）/ 分屏 */
  const compareMode = ref<"off" | "original" | "split">("off");
  /** 分屏位置 0..1 */
  const splitX = ref(0.5);

  /* ---------------- 裁剪交互 ---------------- */
  const cropping = ref(false);

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
      histogram.value = null;
      await renderNow("preview");
      await refreshHistogram();
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
    }
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
    cropping,
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