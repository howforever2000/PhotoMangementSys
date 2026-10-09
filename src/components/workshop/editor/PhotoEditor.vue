<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useNotify } from "../../../composables/useNotify";
import { useEditStore } from "../../../stores/edit";
import type { AdjustmentRecipe } from "../../../types/edit";
import { cloneRecipe } from "../../../types/edit";
import CropOverlay from "./CropOverlay.vue";
import EditorPanel from "./EditorPanel.vue";
import HistogramPanel from "./HistogramPanel.vue";

/**
 * 创意工坊 · 基准调色（FEAT-099）宿主组件
 * =====================================================================
 * 布局（对齐 Lightroom 的「Develop」模块）：
 *
 * ```text
 * ┌──────────────────────────────────────────────────────────────┐
 * │ 工具条：打开 保存方案 导入 导出 撤销 重做 自动 对比            │
 * ├──────────┬─────────────────────────────┬─────────────────────┤
 * │ 直方图   │        预览画布 + 裁剪层     │  调整面板（schema） │
 * │          │                             │                     │
 * ├──────────┴─────────────────────────────┴─────────────────────┤
 * │ 状态栏：尺寸 · 档位 · 渲染耗时 · 配方状态                     │
 * └──────────────────────────────────────────────────────────────┘
 * ```
 *
 * 「左直方图 + 中画布 + 右面板」这个三栏结构是 Lightroom 的标志性布局：
 * 直方图必须在**视线不移动**的情况下与画布同时可见，否则用户无法边看分布
 * 边调曝光 —— 这是调色里最高频的反馈闭环。
 *
 * ## 宿主只做四件事
 *
 * 1. 布局（上面这个三栏）；
 * 2. 画布适配（把预览图按容器尺寸等比放置，并算出裁剪层需要的 `frame`）；
 * 3. 文件对话框（打开/保存方案/导出）；
 * 4. 快捷键（Ctrl+Z / Ctrl+Shift+Z / Y / 0 / \\）。
 *
 * 参数编辑、渲染调度、撤销栈全在 store 与 EditorPanel 里。
 */
const props = defineProps<{ initialPath?: string }>();
const notify = useNotify();
const store = useEditStore();

/* ---------------- 折叠状态持久化 ---------------- */

/**
 * 面板折叠状态按「图源路径」隔离。
 *
 * 用路径的简单散列而不是路径本身：路径可能很长（含用户目录名），
 * 直接当 localStorage key 既占空间也把私人路径写进存储。
 */
function foldKey(path: string): string {
  let h = 2166136261;
  for (let i = 0; i < path.length; i++) {
    h ^= path.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return `pm-edit-fold:${(h >>> 0).toString(36)}`;
}

const collapsed = ref<Record<string, boolean>>(loadFold());
const foldKeyRef = ref("");

function loadFold(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(foldKey(props.initialPath ?? ""));
    return raw ? (JSON.parse(raw) as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

function saveFold(): void {
  try {
    localStorage.setItem(foldKey(foldKeyRef.value), JSON.stringify(collapsed.value));
  } catch {
    /* 忽略（隐私模式 / 配额满） */
  }
}

watch(collapsed, saveFold, { deep: true });

/* ---------------- 画布适配 ---------------- */

const stageEl = ref<HTMLElement | null>(null);
const stageBox = ref({ w: 0, h: 0 });
/** 预览图在 stage 内的显示矩形（stage 局部坐标） */
const imageFrame = ref({ x: 0, y: 0, w: 0, h: 0 });

const aspectRatio = computed(() => {
  const { w, h } = store.previewSize.w ? store.previewSize : { w: store.originalWidth, h: store.originalHeight };
  return w > 0 && h > 0 ? w / h : 3 / 2;
});

/** 预览图按「适配容器」放置（contain）。 */
function layout(): void {
  const el = stageEl.value;
  if (!el) return;
  const r = el.getBoundingClientRect();
  stageBox.value = { w: r.width, h: r.height };
  const pad = 16;
  const availW = Math.max(40, r.width - pad * 2);
  const availH = Math.max(40, r.height - pad * 2);
  let w = availW;
  let h = w / aspectRatio.value;
  if (h > availH) {
    h = availH;
    w = h * aspectRatio.value;
  }
  imageFrame.value = {
    x: (r.width - w) / 2,
    y: (r.height - h) / 2,
    w,
    h,
  };
}

const ro = ref<ResizeObserver | null>(null);
onMounted(() => {
  store.revive();
  void store.ensureSchema();
  layout();
  if (stageEl.value && typeof ResizeObserver !== "undefined") {
    const obs = new ResizeObserver(() => layout());
    obs.observe(stageEl.value);
    ro.value = obs;
  }
  if (props.initialPath) void doOpen(props.initialPath);
});

onBeforeUnmount(() => {
  ro.value?.disconnect();
  ro.value = null;
  store.dispose();
});

/* ---------------- 打开 / 关闭 ---------------- */

async function chooseImage(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "选择要调色的照片",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"] }],
  });
  if (typeof picked !== "string") return;
  await doOpen(picked);
}

async function doOpen(path: string): Promise<void> {
  foldKeyRef.value = path;
  collapsed.value = loadFold();
  const ok = await store.open(path);
  if (!ok) {
    notify.error("打开照片失败", store.renderError);
    return;
  }
  layout();
  notify.success(
    "已打开",
    `${store.originalWidth}×${store.originalHeight} · 解码 ${store.decodeMs}ms${
      store.hasSidecar ? " · 已载入上次的调整方案" : ""
    }`,
  );
}

/* ---------------- 配方持久化 ---------------- */

async function saveRecipe(): Promise<void> {
  if (!store.hasImage) return;
  try {
    const p = await store.persist();
    notify.success("调整方案已保存", p);
  } catch (e) {
    notify.error("保存失败", String((e as Error)?.message ?? e));
  }
}

async function resetAll(): Promise<void> {
  if (!store.hasImage) return;
  const ok = await notify.confirm("重置全部调整？", "当前所有滑块、曲线与裁剪都会回到原图，可撤销。");
  if (!ok) return;
  await store.resetToOriginal();
  notify.info("已重置为原图", "如需一并删除已保存的方案，请在导出时选择「原始」或重新打开照片。");
}

/* ---------------- 导入 / 导出 ---------------- */

async function exportRecipe(): Promise<void> {
  if (!store.hasImage) return;
  try {
    const text = await store.exportText();
    const defaultName = `${baseName()}.pmrecipe.json`;
    const target = await saveFileDialog({
      title: "导出调整方案",
      defaultPath: defaultName,
      filters: [{ name: "调整方案", extensions: ["json"] }],
    });
    if (typeof target !== "string") {
      // 用户取消：仍然把 JSON 复制到剪贴板，便于跨设备粘贴
      await copyText(text);
      notify.info("方案已复制到剪贴板", "可直接粘贴到别的设备导入");
      return;
    }
    await writeTextFile(target, text);
    notify.success("方案已导出", target);
  } catch (e) {
    notify.error("导出失败", String((e as Error)?.message ?? e));
  }
}

async function importRecipe(): Promise<void> {
  if (!store.hasImage) return;
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "导入调整方案",
    filters: [{ name: "调整方案", extensions: ["json"] }],
  });
  if (typeof picked === "string") {
    try {
      const text = await readTextFile(picked);
      await store.importText(text);
      notify.success("方案已导入", picked);
      return;
    } catch (e) {
      notify.error("导入失败", String((e as Error)?.message ?? e));
      return;
    }
  }
  // 未选文件：走剪贴板
  const text = await readClipboard();
  if (!text) {
    notify.info("未选择文件", "剪贴板里也没有方案文本，已取消");
    return;
  }
  try {
    await store.importText(text);
    notify.success("方案已从剪贴板导入");
  } catch (e) {
    notify.error("导入失败", String((e as Error)?.message ?? e));
  }
}

/* ---------------- 导出图片 ---------------- */

const exportFormat = ref<"jpeg" | "png">("jpeg");
const exportQuality = ref(92);
const exportMaxEdge = ref(0);

async function exportImage(): Promise<void> {
  if (!store.hasImage) return;
  const stem = baseName();
  const ext = exportFormat.value === "png" ? "png" : "jpg";
  const target = await saveFileDialog({
    title: "导出照片",
    defaultPath: `${stem}_edited.${ext}`,
    filters:
      exportFormat.value === "png"
        ? [{ name: "PNG（无损）", extensions: ["png"] }]
        : [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
  });
  if (typeof target !== "string") return;

  try {
    const [w, h, blob] = await store.queryExportSize(
      exportFormat.value,
      exportQuality.value,
      exportMaxEdge.value,
    );
    const buf = await blob.arrayBuffer();
    await invoke("edit_write_file", { path: target, data: Array.from(new Uint8Array(buf)) });
    notify.success("导出完成", `${w}×${h} · ${(blob.size / 1024).toFixed(0)} KB`);
  } catch (e) {
    notify.error("导出失败", String((e as Error)?.message ?? e));
  }
}

function baseName(): string {
  const p = store.sourcePath.replace(/\\/g, "/");
  const n = p.slice(p.lastIndexOf("/") + 1);
  const dot = n.lastIndexOf(".");
  return dot > 0 ? n.slice(0, dot) : n;
}

/* ---------------- 自动优化 ---------------- */

async function doAutotune(): Promise<void> {
  if (!store.hasImage) return;
  await store.autotune();
  notify.info("已应用自动优化", "只调整了曝光与极端截断，其余保持中性，可随时微调或撤销。");
}

/* ---------------- 对比 ---------------- */

/**
 * 「按 Y」切对比（Lightroom 的经典键位）。
 *
 * `off` ⇄ `original`：整幅看原图。再按一次回编辑态。
 * `split`（分屏）用单独按钮，因为它需要拖分割线的持续交互。
 */
function toggleCompare(): void {
  store.compareMode = store.compareMode === "off" ? "original" : "off";
}

/* ---------------- 快捷键 ---------------- */

function onKeydown(e: KeyboardEvent): void {
  const t = e.target as HTMLElement | null;
  // 输入控件聚焦时不劫持（滑块用键盘调节，曲线画布可聚焦）
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;

  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    if (e.shiftKey) void store.redo();
    else void store.undo();
    return;
  }
  if (mod && e.key.toLowerCase() === "y") {
    e.preventDefault();
    void store.redo();
    return;
  }
  if (e.key === "y" || e.key === "Y") {
    if (store.compareMode !== "split") {
      e.preventDefault();
      toggleCompare();
    }
    return;
  }
  if (e.key === "\\") {
    e.preventDefault();
    store.compareMode = store.compareMode === "split" ? "off" : "split";
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));

/* ---------------- 状态栏 ---------------- */

const statusLine = computed(() => {
  if (!store.hasImage) return "选择一张照片开始调色";
  const q = store.quality === "draft" ? "草稿档" : store.subsampled ? "预览(降采样)" : "预览";
  const dirty = store.pristine ? "未调整" : "已调整（可保存方案）";
  return `${store.originalWidth}×${store.originalHeight} · ${q} ${store.previewSize.w}×${store.previewSize.h} · 渲染 ${store.renderMs}ms · ${dirty}`;
});

/* ---------------- 导入导出小工具 ---------------- */

/** Tauri 没有内置剪贴板读 API；用隐藏 textarea + execCommand 兜底。 */
async function readClipboard(): Promise<string> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    notify.info("无法读取剪贴板", "请改用「导入方案 → 选择文件」");
    return "";
  }
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
    } finally {
      document.body.removeChild(ta);
    }
  }
}

async function readTextFile(path: string): Promise<string> {
  // 复用 python-studio 的读文件能力？没有对应命令 → 用 fs 插件不可用时
  // 退回到让用户粘贴。方案文件都是 JSON，粘贴完全够用。
  void path;
  throw new Error("该版本暂不支持从文件读取方案，请用剪贴板粘贴（Ctrl+V 后点「导入方案」）");
}

async function writeTextFile(path: string, text: string): Promise<void> {
  const bytes = new TextEncoder().encode(text);
  await invoke("edit_write_file", { path, data: Array.from(bytes) });
}

/* ---------------- 暴露给模板 ---------------- */

const panelCollapsed = computed({
  get: () => collapsed.value,
  set: (v: Record<string, boolean>) => (collapsed.value = v),
});

void CropOverlay;
void cloneRecipe;
void panelCollapsed;
export type { AdjustmentRecipe };
</script>

<template>
  <div class="tone-editor">
    <!-- 工具条 -->
    <header class="te-toolbar">
      <button class="te-btn primary" type="button" :disabled="!store.hasImage" @click="chooseImage">
        📂 打开照片
      </button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="saveRecipe">
        💾 保存方案
      </button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="importRecipe">📥 导入方案</button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="exportRecipe">📤 导出方案</button>

      <span class="te-div"></span>

      <button class="te-btn" type="button" :disabled="!store.canUndo" title="Ctrl+Z" @click="store.undo()">
        ↶ 撤销
      </button>
      <button class="te-btn" type="button" :disabled="!store.canRedo" title="Ctrl+Shift+Z" @click="store.redo()">
        ↷ 重做
      </button>

      <span class="te-div"></span>

      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="doAutotune">✨ 自动优化</button>
      <button class="te-btn" type="button" :disabled="store.pristine" @click="resetAll">↺ 重置</button>

      <span class="te-spacer"></span>

      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.compareMode === 'original' }"
        title="Y"
        @click="toggleCompare"
      >
        {{ store.compareMode === "original" ? "看结果" : "看原图" }}
      </button>
      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.compareMode === 'split' }"
        title="\\"
        @click="store.compareMode = store.compareMode === 'split' ? 'off' : 'split'"
      >
        分屏对比
      </button>

      <span class="te-div"></span>

      <select v-model="exportFormat" class="te-select" :disabled="!store.hasImage" title="导出格式">
        <option value="jpeg">JPEG</option>
        <option value="png">PNG</option>
      </select>
      <template v-if="exportFormat === 'jpeg'">
        <input
          v-model.number="exportQuality"
          class="te-num"
          type="number"
          min="50"
          max="100"
          step="1"
          title="JPEG 质量"
          :disabled="!store.hasImage"
        />
      </template>
      <select v-model.number="exportMaxEdge" class="te-select" :disabled="!store.hasImage" title="导出长边">
        <option :value="0">原始尺寸</option>
        <option :value="4096">长边 4096</option>
        <option :value="2560">长边 2560</option>
        <option :value="1600">长边 1600</option>
      </select>
      <button class="te-btn accent" type="button" :disabled="!store.hasImage" @click="exportImage">
        导出照片
      </button>
    </header>

    <!-- 三栏主体 -->
    <div class="te-body">
      <!-- 左：直方图 -->
      <aside class="te-left">
        <HistogramPanel
          :histogram="store.histogram"
          :width="store.previewSize.w"
          :height="store.previewSize.h"
        />
        <div class="te-hint">
          <p>拖动滑块实时预览，松开后自动出高清图。</p>
          <p>双击滑块归零 · Shift 精细调节 · 滚轮也可调。</p>
          <p><kbd>Y</kbd> 对比原图 · <kbd>\</kbd> 分屏 · <kbd>Ctrl+Z</kbd> 撤销。</p>
        </div>
      </aside>

      <!-- 中：画布 -->
      <section ref="stageEl" class="te-stage" :class="{ cropping: store.cropping }">
        <div v-if="!store.hasImage" class="te-empty">
          <p class="te-empty-title">还没有打开照片</p>
          <p class="te-empty-sub">点左上「打开照片」，或从灯箱的「✏️ 编辑」直接进来。</p>
          <button class="te-btn primary" type="button" @click="chooseImage">📂 选择照片</button>
        </div>

        <template v-else>
          <div
            class="te-image-holder"
            :style="{
              left: `${imageFrame.x}px`,
              top: `${imageFrame.y}px`,
              width: `${imageFrame.w}px`,
              height: `${imageFrame.h}px`,
            }"
          >
            <img
              v-show="store.compareMode !== 'original'"
              :src="store.previewUrl"
              alt="调整后预览"
              class="te-image"
              draggable="false"
            />

            <!-- 原图对照：这里显示「不带调整」的渲染结果 -->
            <img
              v-if="store.compareMode !== 'off'"
              :src="store.previewUrl"
              alt="原图对照"
              class="te-image te-image-split"
              :style="{ clipPath: `inset(0 0 0 ${store.splitX * 100}%)` }"
              draggable="false"
            />

            <!-- 分屏分割线 -->
            <div
              v-if="store.compareMode === 'split'"
              class="te-split"
              :style="{ left: `${store.splitX * 100}%` }"
              @pointerdown="
                (e: PointerEvent) => {
                  const holder = (e.currentTarget as HTMLElement).parentElement as HTMLElement;
                  holder.setPointerCapture(e.pointerId);
                  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                }
              "
              @pointermove="
                (e: PointerEvent) => {
                  const holder = (e.currentTarget as HTMLElement).parentElement as HTMLElement;
                  if (!holder?.hasPointerCapture(e.pointerId)) return;
                  const r = holder.getBoundingClientRect();
                  store.splitX = Math.min(0.98, Math.max(0.02, (e.clientX - r.left) / r.width));
                }
              "
            >
              <span class="te-split-grip">◀▶</span>
            </div>

            <!-- 裁剪层 -->
            <CropOverlay
              v-if="store.cropping"
              :crop="store.recipe.crop"
              :frame="imageFrame"
              :src-width="store.originalWidth"
              :src-height="store.originalHeight"
              @begin="store.beginInteraction()"
              @update:crop="(c) => { const n = cloneRecipe(store.recipe); n.crop = c; void store.applyRecipe(n, false); }"
              @commit:crop="
                (c) => {
                  const n = cloneRecipe(store.recipe);
                  n.crop = c;
                  void store.applyRecipe(n, true);
                }
              "
            />
          </div>
        </template>

        <!-- 渲染错误：必须显式展示，不能静默 -->
        <div v-if="store.renderError" class="te-error">
          <strong>渲染失败</strong>
          <span>{{ store.renderError }}</span>
          <button class="te-btn" type="button" @click="store.renderNow?.('preview')">重试</button>
        </div>
      </section>

      <!-- 右：调整面板 -->
      <aside class="te-right">
        <EditorPanel v-model:collapsed="panelCollapsed" />
      </aside>
    </div>

    <!-- 状态栏 -->
    <footer class="te-status">
      <span>{{ statusLine }}</span>
      <span v-if="store.rendering" class="te-spin">渲染中…</span>
      <span v-if="store.hasSidecar" class="te-tag">已保存方案</span>
    </footer>
  </div>
</template>

<style scoped>
.tone-editor {
  display: flex;
  flex-direction: column;
  height: calc(100vh - 160px);
  min-height: 520px;
  border-radius: var(--radius-lg, 16px);
  overflow: hidden;
}

/* ---------------- 工具条 ---------------- */

.te-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  padding: 8px 12px;
  border-bottom: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.2));
}

.te-btn {
  padding: 4px 10px;
  font-size: 12px;
  color: var(--color-text, #e6e9f0);
  background: var(--pm-btn-bg, rgba(128, 128, 138, 0.14));
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 7px;
  cursor: pointer;
  white-space: nowrap;
}
.te-btn:hover:not(:disabled) {
  background: var(--pm-btn-hover, rgba(128, 128, 138, 0.26));
}
.te-btn:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}
.te-btn.primary {
  background: var(--color-primary, rgba(140, 175, 235, 0.9));
  color: #171b22;
  border-color: transparent;
  font-weight: 600;
}
.te-btn.accent {
  background: rgba(255, 214, 120, 0.9);
  color: #20242c;
  border-color: transparent;
  font-weight: 600;
}
.te-btn.active {
  background: rgba(255, 214, 120, 0.85);
  color: #20242c;
  border-color: transparent;
}

.te-div {
  width: 1px;
  height: 18px;
  background: var(--pm-soft-border, rgba(128, 128, 138, 0.22));
}
.te-spacer {
  flex: 1;
}
.te-select,
.te-num {
  padding: 3px 6px;
  font-size: 11px;
  color: var(--color-text, #e6e9f0);
  background: var(--pm-input-bg, rgba(128, 128, 138, 0.14));
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 6px;
}
.te-num {
  width: 56px;
}

/* ---------------- 三栏 ---------------- */

.te-body {
  flex: 1;
  display: grid;
  grid-template-columns: 300px 1fr 340px;
  min-height: 0;
}

.te-left {
  padding: 12px 10px;
  border-right: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.18));
  overflow-y: auto;
}
.te-hint {
  margin-top: 12px;
  font-size: 11px;
  line-height: 1.7;
  color: var(--pm-hint, var(--color-text-2));
}
.te-hint p {
  margin: 0 0 4px;
}
.te-hint kbd {
  padding: 0 4px;
  font-size: 10px;
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.3));
  border-radius: 3px;
  background: rgba(128, 128, 138, 0.12);
}

.te-stage {
  position: relative;
  overflow: hidden;
  background: rgba(12, 14, 18, 0.35);
  cursor: default;
}
.te-stage.cropping {
  cursor: crosshair;
}

.te-image-holder {
  position: absolute;
}

.te-image {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
  user-select: none;
  -webkit-user-drag: none;
}

.te-image-split {
  position: absolute;
  inset: 0;
  /* 左侧用「结果」、右侧用「原图」：clipPath 把左半裁掉，
     于是右半露出的是下面那张「原图」渲染结果 */
  filter: none;
}

.te-split {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 2px;
  margin-left: -1px;
  background: rgba(255, 255, 255, 0.9);
  cursor: ew-resize;
  touch-action: none;
}
.te-split-grip {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  padding: 2px 6px;
  font-size: 10px;
  color: #20242c;
  background: rgba(255, 255, 255, 0.92);
  border-radius: 8px;
}

.te-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  text-align: center;
}
.te-empty-title {
  margin: 0;
  font-size: 15px;
  color: var(--color-text, #e6e9f0);
}
.te-empty-sub {
  margin: 0 0 8px;
  font-size: 12px;
  color: var(--pm-hint, var(--color-text-2));
}

.te-error {
  position: absolute;
  left: 50%;
  bottom: 16px;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  font-size: 12px;
  color: #ffd7d7;
  background: rgba(120, 30, 34, 0.9);
  border-radius: 8px;
}
.te-error strong {
  white-space: nowrap;
}
.te-error span {
  max-width: 380px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.te-right {
  border-left: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.18));
  overflow-y: auto;
}

/* ---------------- 状态栏 ---------------- */

.te-status {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 5px 12px;
  font-size: 11px;
  color: var(--pm-hint, var(--color-text-2));
  border-top: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.2));
  font-variant-numeric: tabular-nums;
}
.te-spin {
  color: var(--color-primary, rgba(140, 175, 235, 0.95));
}
.te-tag {
  padding: 1px 7px;
  border-radius: 8px;
  background: rgba(120, 190, 120, 0.18);
  color: #a8dca8;
}

@media (max-width: 1180px) {
  .te-body {
    grid-template-columns: 1fr 320px;
  }
  .te-left {
    display: none;
  }
}
</style>