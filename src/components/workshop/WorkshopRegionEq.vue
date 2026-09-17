<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useThemeStore } from "../../stores/theme";

/**
 * 创意工坊 · 区域直方图均衡化（FEAT-063）
 * ========================================
 * 交互：载入图片 → 框选（拖拽矩形）或画笔涂抹/橡皮 生成蒙版 → 调参数 → 应用。
 * 处理：独立 Python 微服务（python-studio，Rust studio_ensure 负责拉起），
 *       前端 fetch 直连 127.0.0.1，multipart 上传原图 + 灰度蒙版。
 * 蒙版约定：独立离屏 canvas，与原图同分辨率，白=选中，黑=未选。
 */
const theme = useThemeStore();

/* ---------------- 状态 ---------------- */
type Tool = "rect" | "brush" | "eraser";
const tool = ref<Tool>("rect");
const brushSize = ref(24);
const mode = ref<"clahe" | "global">("clahe");
const strength = ref(0.8);
const feather = ref(8);

const srcPath = ref("");
const statusMsg = ref("先选择一张图片");
const processing = ref(false);
const hasResult = ref(false);
const showResult = ref(false);

const imgEl = new Image();
const resultEl = new Image();
let resultBlob: Blob | null = null;
let objectUrls: string[] = [];

/** 蒙版离屏画布（原图分辨率，白=选中） */
const maskCanvas = document.createElement("canvas");
const maskCtx = maskCanvas.getContext("2d")!;

const displayCanvas = ref<HTMLCanvasElement | null>(null);
const overlayCanvas = ref<HTMLCanvasElement | null>(null);

/** 显示缩放：画布尺寸 / 原图尺寸 */
const scale = ref(1);
/** 拖拽框选的实时预览（显示坐标） */
const dragRect = ref<{ x: number; y: number; w: number; h: number } | null>(null);

const canApply = computed(() => !!srcPath.value && !processing.value);
const panelStyle = computed(() => theme.cardStyle);

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
      maskCtx.fillStyle = "#000";
      maskCtx.fillRect(0, 0, maskCanvas.width, maskCanvas.height);
      hasResult.value = false;
      showResult.value = false;
      resultBlob = null;
      fitDisplay();
      redraw();
      statusMsg.value = `已载入 ${imgEl.naturalWidth}×${imgEl.naturalHeight}，框选或涂抹要均衡的区域`;
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
  const maxW = disp.parentElement?.clientWidth ?? 800;
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
  const base = showResult.value && hasResult.value ? resultEl : imgEl;
  ctx.clearRect(0, 0, disp.width, disp.height);
  ctx.drawImage(base, 0, 0, disp.width, disp.height);

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
      statusMsg.value = "已加入框选区域";
    }
    dragStart = null;
  }
  redraw();
}

/** 画笔/橡皮落墨（原图坐标，半径随缩放换算） */
function stroke(nx: number, ny: number) {
  const r = (brushSize.value / 2) / scale.value;
  maskCtx.fillStyle = tool.value === "eraser" ? "#000" : "#fff";
  maskCtx.beginPath();
  maskCtx.arc(nx, ny, Math.max(1, r), 0, Math.PI * 2);
  maskCtx.fill();
}

function clearMask() {
  maskCtx.fillStyle = "#000";
  maskCtx.fillRect(0, 0, maskCanvas.width, maskCanvas.height);
  redraw();
  statusMsg.value = "已清空蒙版";
}

function selectAll() {
  maskCtx.fillStyle = "#fff";
  maskCtx.fillRect(0, 0, maskCanvas.width, maskCanvas.height);
  redraw();
  statusMsg.value = "已全选（整图均衡）";
}

/* ---------------- 调用微服务 ---------------- */
function canvasBlob(canvas: HTMLCanvasElement, type: string): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("canvas 导出失败"))), type),
  );
}

async function apply() {
  if (!srcPath.value || processing.value) return;
  processing.value = true;
  statusMsg.value = "正在处理…";
  try {
    // 1) 确保微服务就绪（Rust 侧探活/收养/启动）
    const base: string = await invoke("studio_ensure");

    // 2) 原图字节（asset 协议 fetch，拿原始文件，不经 canvas 重编码）
    const imgBlob = await (await fetch(convertFileSrc(srcPath.value))).blob();
    const maskBlob = await canvasBlob(maskCanvas, "image/png");

    // 3) multipart 提交
    const form = new FormData();
    form.append("image", imgBlob, "image.jpg");
    form.append("mask", maskBlob, "mask.png");
    form.append("mode", mode.value);
    form.append("strength", String(strength.value));
    form.append("feather", String(feather.value));
    const resp = await fetch(`${base}/api/region-equalize`, { method: "POST", body: form });
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
      statusMsg.value = "处理完成（红色区域已均衡化）——可勾选对比原图/结果，满意后保存";
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
    defaultPath: "region-eq.jpg",
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
onBeforeUnmount(() => {
  window.removeEventListener("resize", onResize);
  objectUrls.forEach((u) => URL.revokeObjectURL(u));
  objectUrls = [];
});
</script>

<template>
  <section class="region-eq" :style="panelStyle">
    <header class="re-head">
      <button class="re-btn" type="button" @click="chooseImage">📂 选择图片</button>
      <div class="re-tools">
        <button
          v-for="t in [
            { id: 'rect', label: '▭ 框选' },
            { id: 'brush', label: '🖌 画蒙版' },
            { id: 'eraser', label: '🧽 橡皮' },
          ]"
          :key="t.id"
          type="button"
          class="re-tool"
          :class="{ on: tool === t.id }"
          @click="tool = t.id as Tool"
        >
          {{ t.label }}
        </button>
        <label v-if="tool !== 'rect'" class="re-brush">
          笔刷
          <input v-model.number="brushSize" type="range" min="4" max="80" step="2" />
          <b>{{ brushSize }}</b>
        </label>
        <button class="re-btn" type="button" @click="selectAll">全选</button>
        <button class="re-btn" type="button" @click="clearMask">清空蒙版</button>
      </div>
    </header>

    <div class="re-stage">
      <canvas ref="displayCanvas" class="re-canvas"></canvas>
      <canvas
        ref="overlayCanvas"
        class="re-overlay"
        :class="{ 're-crosshair': tool === 'rect', 're-brush-cursor': tool !== 'rect' }"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
      ></canvas>
      <div v-if="!srcPath" class="re-empty">📂 先选择一张图片开始编辑</div>
    </div>

    <div class="re-params">
      <div class="re-seg">
        <button type="button" :class="{ on: mode === 'clahe' }" @click="mode = 'clahe'">CLAHE（局部自适应）</button>
        <button type="button" :class="{ on: mode === 'global' }" @click="mode = 'global'">全局均衡</button>
      </div>
      <label class="re-range">
        <span>强度</span>
        <input v-model.number="strength" type="range" min="0.1" max="1" step="0.05" />
        <b>{{ Math.round(strength * 100) }}%</b>
      </label>
      <label class="re-range">
        <span>羽化</span>
        <input v-model.number="feather" type="range" min="0" max="40" step="1" />
        <b>{{ feather }}px</b>
      </label>
    </div>

    <footer class="re-foot">
      <span class="re-status" :class="{ err: statusMsg.startsWith('处理失败') || statusMsg.includes('失败') }">{{ statusMsg }}</span>
      <div class="re-actions">
        <label v-if="hasResult" class="re-check">
          <input v-model="showResult" type="checkbox" @change="redraw" />
          对比结果
        </label>
        <button class="re-btn" type="button" :disabled="!hasResult" @click="saveResult">💾 保存结果</button>
        <button class="re-btn re-primary" type="button" :disabled="!canApply" @click="apply">
          {{ processing ? "处理中…" : "✨ 应用均衡化" }}
        </button>
      </div>
    </footer>
  </section>
</template>

<style scoped>
.region-eq {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border-radius: var(--radius-lg, 16px);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.re-head,
.re-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.re-tools {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.re-btn {
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

.re-btn:hover:not(:disabled) {
  background: rgba(120, 120, 130, 0.22);
}

.re-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.re-primary {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.re-primary:hover:not(:disabled) {
  background: #2f5de0;
}

.re-tool {
  height: 34px;
  padding: 0 12px;
  font-size: 13px;
  color: var(--color-text-2);
  background: transparent;
  border: 1px solid rgba(120, 120, 130, 0.25);
  border-radius: 9px;
  cursor: pointer;
}

.re-tool.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.re-brush {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.re-brush input[type="range"] {
  width: 110px;
}

.re-stage {
  position: relative;
  display: flex;
  justify-content: center;
  min-height: 320px;
  border-radius: 12px;
  background:
    repeating-conic-gradient(rgba(128, 128, 128, 0.12) 0% 25%, transparent 0% 50%) 50% / 22px 22px;
}

.re-canvas,
.re-overlay {
  position: absolute;
  border-radius: 8px;
}

.re-overlay.re-crosshair {
  cursor: crosshair;
}

.re-overlay.re-brush-cursor {
  cursor: pointer;
}

.re-empty {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  color: var(--color-text-2);
}

.re-params {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
}

.re-seg {
  display: flex;
  gap: 8px;
}

.re-seg button {
  height: 32px;
  padding: 0 12px;
  font-size: 12.5px;
  color: var(--color-text-2);
  background: rgba(120, 120, 130, 0.1);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 9px;
  cursor: pointer;
}

.re-seg button.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.re-range {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.re-range input[type="range"] {
  width: 140px;
}

.re-range b {
  min-width: 42px;
  text-align: right;
  color: var(--color-text);
}

.re-status {
  font-size: 12.5px;
  color: var(--color-text-2);
  flex: 1;
  min-width: 200px;
}

.re-status.err {
  color: #e5484d;
}

.re-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.re-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--color-text-2);
  cursor: pointer;
}
</style>
