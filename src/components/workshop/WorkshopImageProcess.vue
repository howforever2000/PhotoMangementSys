<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useThemeStore } from "../../stores/theme";

/**
 * 创意工坊 · 传统图像处理（FEAT-066）
 * ===================================
 * 宿主组件：只负责与算法无关的壳子 —— 图片载入、画布与缩放、蒙版编辑、
 * 结果展示与保存。具体算子与参数表单由后端 `GET /api/ops` 返回的 schema 驱动，
 * 新增算子只改后端注册表，前端不需要加分支。
 *
 * 处理：独立 Python 微服务（python-studio，Rust studio_ensure 负责拉起），
 *       前端 fetch 直连 127.0.0.1，`POST /api/apply` 上传 op + params + 原图 + 可选蒙版。
 * 蒙版约定：独立离屏 canvas，与原图同分辨率；**画布内透明=未选、白=选中**
 *       （BUG-2026-0918-006：不透明黑底会让 overlay 的 source-in 红色叠加
 *       铺满全图——黑也是 alpha=255）。导出给后端时铺黑合成灰度 PNG。
 * 蒙版是可选的：不画蒙版 = 整图处理（模糊类默认整图）。
 */
const theme = useThemeStore();

/**
 * FEAT-065：可选的初始图片（由灯箱「✏️ 编辑」经 /workshop?photo=<path> 带入）。
 * 传了就自动载入，不传则保持原有「先选择一张图片」流程。
 */
const props = defineProps<{ initialPath?: string }>();

/* ---------------- 算子注册表（后端拉取） ---------------- */
type OpParam = {
  name: string;
  label: string;
  type: "number" | "bool" | "enum";
  min?: number;
  max?: number;
  step?: number;
  default: number | string | boolean;
  choices?: { value: string; label: string }[];
};
type OpDef = { id: string; label: string; supportsMask: boolean; params: OpParam[] };

/** 灯箱 ✏️ 直达的默认算子（FEAT-065 行为不变） */
const DEFAULT_OP = "region-equalize";

const ops = ref<OpDef[]>([]);
const opId = ref(DEFAULT_OP);
/** 当前算子的参数值（切换算子时按 schema 重置为默认值） */
const values = ref<Record<string, number | string | boolean>>({});

const currentOp = computed(() => ops.value.find((o) => o.id === opId.value) ?? null);
/** 模糊组：注册表里 id 以 blur- 开头的算子（新增模糊算子自动归入该组） */
const blurOps = computed(() => ops.value.filter((o) => o.id.startsWith("blur-")));
const eqOp = computed(() => ops.value.find((o) => o.id === DEFAULT_OP) ?? null);
/** 顶层分组：均衡化 / 模糊 */
const group = computed<"eq" | "blur">(() => (opId.value === DEFAULT_OP ? "eq" : "blur"));

/* ---------------- 状态 ---------------- */
type Tool = "rect" | "brush" | "eraser";
const tool = ref<Tool>("rect");
const brushSize = ref(24);

const srcPath = ref("");
const statusMsg = ref("先选择一张图片");
const processing = ref(false);
const hasResult = ref(false);
const showResult = ref(false);
/** 蒙版上是否有选中区域（无蒙版 = 整图处理，不再阻止「应用」） */
const hasMask = ref(false);

const imgEl = new Image();
const resultEl = new Image();
let resultBlob: Blob | null = null;
let objectUrls: string[] = [];
/** 微服务基址（studio_ensure 成功后缓存，后续请求直接复用） */
let base = "";

/** 蒙版离屏画布（原图分辨率，白=选中） */
const maskCanvas = document.createElement("canvas");
const maskCtx = maskCanvas.getContext("2d")!;

const displayCanvas = ref<HTMLCanvasElement | null>(null);
const overlayCanvas = ref<HTMLCanvasElement | null>(null);

/** 显示缩放：画布尺寸 / 原图尺寸 */
const scale = ref(1);
/** 拖拽框选的实时预览（显示坐标） */
const dragRect = ref<{ x: number; y: number; w: number; h: number } | null>(null);

const canApply = computed(() => !!srcPath.value && !processing.value && !!currentOp.value);
const panelStyle = computed(() => theme.cardStyle);

/* ---------------- 算子与参数 ---------------- */
async function ensureBase(): Promise<string> {
  if (!base) base = await invoke<string>("studio_ensure");
  return base;
}

/** 拉取算子注册表（schema 驱动参数表单） */
async function loadOps() {
  try {
    const resp = await fetch(`${await ensureBase()}/api/ops`);
    if (!resp.ok) throw new Error(`服务返回 ${resp.status}`);
    ops.value = ((await resp.json()) as { ops?: OpDef[] }).ops ?? [];
    applyDefaults();
  } catch (e) {
    statusMsg.value = `算子列表加载失败：${String(e)}`;
  }
}

/** 按 schema 默认值初始化当前算子的参数 */
function applyDefaults() {
  const o = currentOp.value;
  const init: Record<string, number | string | boolean> = {};
  if (o) for (const p of o.params) init[p.name] = p.default;
  values.value = init;
}

function selectOp(id: string) {
  opId.value = id;
  applyDefaults();
}

/** 参数显示：0~1 的参数按百分比显示（强度），其余直接显示数值 */
function displayValue(p: OpParam): string {
  const v = values.value[p.name];
  if (p.type === "number" && (p.step ?? 1) < 1) return `${Math.round(Number(v) * 100)}%`;
  return String(v);
}

/* ---------------- 载入图片 ---------------- */
async function chooseImage() {
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "选择要编辑的图片",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp"] }],
  });
  if (typeof picked !== "string") return;
  await loadImage(picked);
}

function loadImage(path: string): Promise<void> {
  return new Promise((resolve) => {
    const url = convertFileSrc(path);
    imgEl.onload = () => {
      srcPath.value = path;
      maskCanvas.width = imgEl.naturalWidth;
      maskCanvas.height = imgEl.naturalHeight;
      // 赋值 width/height 已把画布重置为全透明（= 未选中），无需再铺底色
      hasMask.value = false;
      hasResult.value = false;
      showResult.value = false;
      resultBlob = null;
      fitDisplay();
      redraw();
      statusMsg.value = `已载入 ${imgEl.naturalWidth}×${imgEl.naturalHeight}，框选或涂抹要处理的区域（不画=整图）`;
      resolve();
    };
    imgEl.onerror = () => {
      statusMsg.value = "图片加载失败";
      resolve();
    };
    imgEl.src = url;
  });
}

/** 显示画布适配：最长边贴合容器（保持比例） */
function fitDisplay() {
  const disp = displayCanvas.value;
  const overlay = overlayCanvas.value;
  if (!disp || !overlay) return;
  const cw = disp.parentElement?.clientWidth ?? 0;
  // 帧未稳定/容器被隐藏时宽度可能为 0，兜底避免算出 1×1 画布
  const maxW = cw > 0 ? cw : 800;
  const maxH = Math.max(320, window.innerHeight * 0.58);
  const s = Math.min(1, maxW / imgEl.naturalWidth, maxH / imgEl.naturalHeight);
  scale.value = s;
  const w = Math.max(1, Math.round(imgEl.naturalWidth * s));
  const h = Math.max(1, Math.round(imgEl.naturalHeight * s));
  disp.width = w;
  disp.height = h;
  overlay.width = w;
  overlay.height = h;
}

function redraw() {
  const disp = displayCanvas.value;
  const overlay = overlayCanvas.value;
  if (!disp || !overlay || !imgEl.naturalWidth) return;
  const ctx = disp.getContext("2d")!;
  const b = showResult.value && hasResult.value ? resultEl : imgEl;
  ctx.clearRect(0, 0, disp.width, disp.height);
  ctx.drawImage(b, 0, 0, disp.width, disp.height);

  // 蒙版红色半透明叠加
  const octx = overlay.getContext("2d")!;
  octx.clearRect(0, 0, overlay.width, overlay.height);
  octx.save();
  octx.drawImage(maskCanvas, 0, 0, overlay.width, overlay.height);
  octx.globalCompositeOperation = "source-in";
  octx.fillStyle = "rgba(255,64,64,.5)";
  octx.fillRect(0, 0, overlay.width, overlay.height);
  octx.restore();
  // 框选拖拽预览
  if (dragRect.value) {
    const { x, y, w, h } = dragRect.value;
    octx.strokeStyle = "#ffd54a";
    octx.lineWidth = 1.5;
    octx.setLineDash([5, 4]);
    octx.strokeRect(x, y, w, h);
  }
}

/* ---------------- 指针交互（显示坐标 → 原图坐标） ---------------- */
function posOf(e: PointerEvent) {
  const overlay = overlayCanvas.value!;
  const r = overlay.getBoundingClientRect();
  return {
    dx: e.clientX - r.left,
    dy: e.clientY - r.top,
    nx: (e.clientX - r.left) / scale.value,
    ny: (e.clientY - r.top) / scale.value,
  };
}

let painting = false;
let dragStart: { dx: number; dy: number; nx: number; ny: number } | null = null;

function onPointerDown(e: PointerEvent) {
  if (!srcPath.value) return;
  const overlay = overlayCanvas.value!;
  overlay.setPointerCapture(e.pointerId);
  const p = posOf(e);
  painting = true;
  if (tool.value === "rect") {
    dragStart = p;
    dragRect.value = null;
  } else {
    stroke(p.nx, p.ny);
    redraw();
  }
}

function onPointerMove(e: PointerEvent) {
  if (!painting || !srcPath.value) return;
  const p = posOf(e);
  if (tool.value === "rect" && dragStart) {
    dragRect.value = {
      x: Math.min(dragStart.dx, p.dx),
      y: Math.min(dragStart.dy, p.dy),
      w: Math.abs(p.dx - dragStart.dx),
      h: Math.abs(p.dy - dragStart.dy),
    };
    redraw();
  } else if (tool.value !== "rect") {
    stroke(p.nx, p.ny);
    redraw();
  }
}

function onPointerUp(e: PointerEvent) {
  if (!painting) return;
  painting = false;
  const overlay = overlayCanvas.value!;
  overlay.releasePointerCapture(e.pointerId);
  if (tool.value === "rect" && dragStart) {
    const p = posOf(e);
    const x = Math.min(dragStart.nx, p.nx);
    const y = Math.min(dragStart.ny, p.ny);
    const w = Math.abs(p.nx - dragStart.nx);
    const h = Math.abs(p.ny - dragStart.ny);
    dragRect.value = null;
    if (w > 2 && h > 2) {
      maskCtx.fillStyle = "#fff";
      maskCtx.fillRect(x, y, w, h);
      hasMask.value = true;
      statusMsg.value = "已加入框选区域";
    }
    dragStart = null;
  }
  redraw();
}

/** 画笔/橡皮落墨（原图坐标，半径随缩放换算）。橡皮 = 擦回透明（未选中） */
function stroke(nx: number, ny: number) {
  const r = brushSize.value / 2 / scale.value;
  maskCtx.globalCompositeOperation = tool.value === "eraser" ? "destination-out" : "source-over";
  maskCtx.fillStyle = "#fff";
  maskCtx.beginPath();
  maskCtx.arc(nx, ny, Math.max(1, r), 0, Math.PI * 2);
  maskCtx.fill();
  hasMask.value = true;
}

function clearMask() {
  maskCtx.clearRect(0, 0, maskCanvas.width, maskCanvas.height);
  hasMask.value = false;
  redraw();
  statusMsg.value = "已清空蒙版（将按整图处理）";
}

function selectAll() {
  maskCtx.fillStyle = "#fff";
  maskCtx.fillRect(0, 0, maskCanvas.width, maskCanvas.height);
  hasMask.value = true;
  redraw();
  statusMsg.value = "已全选（整图处理）";
}

/* ---------------- 调用微服务 ---------------- */
function canvasBlob(canvas: HTMLCanvasElement, type: string): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("canvas 导出失败"))), type),
  );
}

async function apply() {
  const op = currentOp.value;
  if (!srcPath.value || processing.value || !op) return;
  processing.value = true;
  statusMsg.value = "正在处理…";
  try {
    // 1) 确保微服务就绪（Rust 侧探活/收养/启动）
    const svc = await ensureBase();

    // 2) 原图字节（asset 协议 fetch，拿原始文件，不经 canvas 重编码）
    const imgBlob = await (await fetch(convertFileSrc(srcPath.value))).blob();

    const form = new FormData();
    form.append("op", op.id);
    form.append("params", JSON.stringify(values.value));
    form.append("image", imgBlob, "image.jpg");

    // 3) 蒙版可选：只在画过蒙版时导出（画布内是「透明=未选、白=选中」，
    //    铺黑合成后端约定的灰度蒙版 PNG；不传 mask 即整图处理）
    if (hasMask.value) {
      const exportCanvas = document.createElement("canvas");
      exportCanvas.width = maskCanvas.width;
      exportCanvas.height = maskCanvas.height;
      const ectx = exportCanvas.getContext("2d")!;
      ectx.fillStyle = "#000";
      ectx.fillRect(0, 0, exportCanvas.width, exportCanvas.height);
      ectx.drawImage(maskCanvas, 0, 0);
      form.append("mask", await canvasBlob(exportCanvas, "image/png"), "mask.png");
    }

    const resp = await fetch(`${svc}/api/apply`, { method: "POST", body: form });
    if (!resp.ok) {
      const detail = await resp.text().catch(() => "");
      throw new Error(`服务返回 ${resp.status}${detail ? `: ${detail.slice(0, 200)}` : ""}`);
    }
    resultBlob = await resp.blob();

    // 4) 展示结果
    const url = URL.createObjectURL(resultBlob);
    objectUrls.push(url);
    resultEl.onload = () => {
      hasResult.value = true;
      showResult.value = true;
      redraw();
      statusMsg.value = `处理完成（${op.label}）——可勾选对比原图/结果，满意后保存`;
    };
    resultEl.src = url;
  } catch (e) {
    statusMsg.value = `处理失败：${String(e)}`;
  } finally {
    processing.value = false;
  }
}

async function saveResult() {
  if (!resultBlob) return;
  const picked = await saveFileDialog({
    title: "保存处理结果",
    defaultPath: `${opId.value}.jpg`,
    filters: [{ name: "JPEG 图片", extensions: ["jpg"] }],
  });
  if (typeof picked !== "string") return;
  const buf = new Uint8Array(await resultBlob.arrayBuffer());
  await invoke("studio_save_result", { path: picked, data: Array.from(buf) });
  statusMsg.value = `已保存：${picked}`;
}

/* ---------------- 生命周期 ---------------- */
function onResize() {
  if (srcPath.value) {
    fitDisplay();
    redraw();
  }
}
window.addEventListener("resize", onResize);

onMounted(async () => {
  await loadOps();
  // FEAT-065：从灯箱带图进来时自动载入（打开即用）。
  // nextTick 等画布真正挂载 —— loadImage 的 onload 里会调 fitDisplay()，
  // 它依赖画布父容器的宽度。
  if (props.initialPath) {
    await nextTick();
    await loadImage(props.initialPath);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("resize", onResize);
  objectUrls.forEach((u) => URL.revokeObjectURL(u));
  objectUrls = [];
});
</script>

<template>
  <section class="ip" :style="panelStyle">
    <header class="ip-head">
      <button class="ip-btn" type="button" @click="chooseImage">📂 选择图片</button>
      <div class="ip-tools">
        <button
          v-for="t in [
            { id: 'rect', label: '▭ 框选' },
            { id: 'brush', label: '🖌 画蒙版' },
            { id: 'eraser', label: '🧽 橡皮' },
          ]"
          :key="t.id"
          type="button"
          class="ip-tool"
          :class="{ on: tool === t.id }"
          @click="tool = t.id as Tool"
        >
          {{ t.label }}
        </button>
        <label v-if="tool !== 'rect'" class="ip-brush">
          笔刷
          <input v-model.number="brushSize" type="range" min="4" max="80" step="2" />
          <b>{{ brushSize }}</b>
        </label>
        <button class="ip-btn" type="button" @click="selectAll">全选</button>
        <button class="ip-btn" type="button" @click="clearMask">清空蒙版</button>
      </div>
    </header>

    <!-- 算子选择：均衡化 / 模糊（模糊下再选具体方式） -->
    <div class="ip-ops">
      <div class="ip-seg">
        <button v-if="eqOp" type="button" :class="{ on: group === 'eq' }" @click="selectOp(eqOp.id)">
          {{ eqOp.label }}
        </button>
        <button
          type="button"
          :class="{ on: group === 'blur' }"
          :disabled="!blurOps.length"
          @click="blurOps.length && selectOp(blurOps[0].id)"
        >
          模糊
        </button>
      </div>
      <div v-if="group === 'blur'" class="ip-seg ip-sub">
        <button
          v-for="o in blurOps"
          :key="o.id"
          type="button"
          :class="{ on: opId === o.id }"
          @click="selectOp(o.id)"
        >
          {{ o.label }}
        </button>
      </div>
    </div>

    <div class="ip-stage">
      <canvas ref="displayCanvas" class="ip-canvas"></canvas>
      <canvas
        ref="overlayCanvas"
        class="ip-overlay"
        :class="{ 'ip-crosshair': tool === 'rect', 'ip-brush-cursor': tool !== 'rect' }"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
      ></canvas>
      <div v-if="!srcPath" class="ip-empty">📂 先选择一张图片开始编辑</div>
    </div>

    <!-- 参数面板：由 /api/ops 的 schema 自动渲染 -->
    <div v-if="currentOp" class="ip-params">
      <template v-for="p in currentOp.params" :key="p.name">
        <div v-if="p.type === 'enum'" class="ip-field">
          <span class="ip-field-label">{{ p.label }}</span>
          <div class="ip-seg">
            <button
              v-for="c in p.choices ?? []"
              :key="c.value"
              type="button"
              :class="{ on: values[p.name] === c.value }"
              @click="values[p.name] = c.value"
            >
              {{ c.label }}
            </button>
          </div>
        </div>
        <label v-else-if="p.type === 'number'" class="ip-range">
          <span>{{ p.label }}</span>
          <input v-model.number="values[p.name]" type="range" :min="p.min" :max="p.max" :step="p.step" />
          <b>{{ displayValue(p) }}</b>
        </label>
        <label v-else class="ip-check">
          <input v-model="values[p.name]" type="checkbox" />
          {{ p.label }}
        </label>
      </template>
      <span v-if="!hasMask" class="ip-hint">未画蒙版 → 按整图处理</span>
    </div>
    <div v-else class="ip-hint">算子列表加载中…</div>

    <footer class="ip-foot">
      <span class="ip-status" :class="{ err: statusMsg.startsWith('处理失败') || statusMsg.includes('失败') }">{{
        statusMsg
      }}</span>
      <div class="ip-actions">
        <label v-if="hasResult" class="ip-check">
          <input v-model="showResult" type="checkbox" @change="redraw" />
          对比结果
        </label>
        <button class="ip-btn" type="button" :disabled="!hasResult" @click="saveResult">💾 保存结果</button>
        <button class="ip-btn ip-primary" type="button" :disabled="!canApply" @click="apply">
          {{ processing ? "处理中…" : `✨ 应用${currentOp ? currentOp.label : ""}` }}
        </button>
      </div>
    </footer>
  </section>
</template>

<style scoped>
.ip {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border-radius: var(--radius-lg, 16px);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.ip-head,
.ip-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.ip-tools {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.ip-btn {
  height: 34px;
  padding: 0 14px;
  font-size: 13px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.12);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 9px;
  cursor: pointer;
  transition: background 0.2s;
}

.ip-btn:hover:not(:disabled) {
  background: rgba(120, 120, 130, 0.22);
}

.ip-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.ip-primary {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-primary:hover:not(:disabled) {
  background: #2f5de0;
}

.ip-tool {
  height: 34px;
  padding: 0 12px;
  font-size: 13px;
  color: var(--color-text-2);
  background: transparent;
  border: 1px solid rgba(120, 120, 130, 0.25);
  border-radius: 9px;
  cursor: pointer;
}

.ip-tool.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-brush {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-brush input[type="range"] {
  width: 110px;
}

.ip-ops {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.ip-stage {
  position: relative;
  display: flex;
  justify-content: center;
  min-height: 320px;
  border-radius: 12px;
  background:
    repeating-conic-gradient(rgba(128, 128, 128, 0.12) 0% 25%, transparent 0% 50%) 50% / 22px 22px;
}

.ip-canvas,
.ip-overlay {
  position: absolute;
  border-radius: 8px;
}

.ip-overlay.ip-crosshair {
  cursor: crosshair;
}

.ip-overlay.ip-brush-cursor {
  cursor: pointer;
}

.ip-empty {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  color: var(--color-text-2);
}

.ip-params {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
}

.ip-field {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-field-label {
  white-space: nowrap;
}

.ip-seg {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.ip-sub {
  padding-left: 4px;
}

.ip-seg button {
  height: 32px;
  padding: 0 12px;
  font-size: 12.5px;
  color: var(--color-text-2);
  background: rgba(120, 120, 130, 0.1);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 9px;
  cursor: pointer;
}

.ip-seg button.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-seg button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.ip-range {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-range input[type="range"] {
  width: 140px;
}

.ip-range b {
  min-width: 42px;
  text-align: right;
  color: var(--color-text);
}

.ip-hint {
  font-size: 12px;
  color: var(--color-text-2);
  opacity: 0.8;
}

.ip-status {
  font-size: 12.5px;
  color: var(--color-text-2);
  flex: 1;
  min-width: 200px;
}

.ip-status.err {
  color: #e5484d;
}

.ip-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.ip-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--color-text-2);
  cursor: pointer;
}
</style>
