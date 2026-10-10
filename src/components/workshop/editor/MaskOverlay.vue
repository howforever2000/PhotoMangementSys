<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";

/**
 * 局部蒙版绘制层（FEAT-099 扩展）
 * ====================================================================
 * 盖在编辑器画布之上的一层透明 canvas：用户框选 / 涂抹出「要调整的区域」，
 * 蒙版之外的像素完全不受当前调整影响。
 *
 * ## 画布分辨率 = 预览分辨率（不是显示分辨率）
 *
 * 蒙版是按**预览平面尺寸**（如长边 1600）保存的 PNG，导出时由后端
 * 最近邻放大到全分辨率。画布跟显示尺寸走会导致同一份蒙版在不同窗口大小下
 * 存出不同分辨率的文件，重新打开时选区就跑位了。
 *
 * ## 为什么用「透明=未选，白=选中」
 *
 * 与 python-studio 的 `_apply_masked` 约定一致，也让叠加显示天然可做：
 * 半透明红色表示选区。若反过来用「黑=未选」，覆盖层就得画全图底色。
 *
 * ## 坐标换算
 *
 * 事件坐标是**显示像素**，要除以 `scale` 才落到蒙版像素。`scale` 由父级
 * 通过 prop 传入（显示尺寸 / 蒙版尺寸），这里不做二次测量 ——
 * 父级已经有 ResizeObserver 在维护布局，多一处测量就多一处可能不同步。
 */
const props = defineProps<{
  /** 蒙版画布尺寸（宽高，与预览平面一致） */
  maskWidth: number;
  maskHeight: number;
  /** 显示尺寸 / 蒙版尺寸 */
  scale: number;
  /** 是否可编辑（没打开图片时禁用） */
  enabled: boolean;
  /**
   * 当前工具与笔刷尺寸。
   *
   * 由父级持有并从右侧面板下发给画布层 —— 工具条不在画布上，
   * 而在面板里（对齐 Lightroom：点「局部」→ 面板里选新建方式 → 回画布绘制）。
   * 画布层若自持一份状态，面板与画布的工具会各说各话。
   */
  tool: Tool;
  brushSize: number;
}>();

const emit = defineEmits<{
  /** 蒙版内容变化（绘制完成，可提交到后端） */
  (e: "change"): void;
}>();

/** 工具类型。与父级共用同一个定义（通过 `MaskTool` 导出）。 */
export type Tool = "rect" | "brush" | "eraser";

/** 离屏蒙版画布：0=透明（未选），255=白（选中） */
const maskCanvas = document.createElement("canvas");
const maskCtx = maskCanvas.getContext("2d", { willReadFrequently: true })!;

/** 预览用画布：把蒙版染成半透明红，叠在图上 */
const viewCanvas = document.createElement("canvas");
const viewCtx = viewCanvas.getContext("2d")!;

/** 本组件自身的 DOM canvas（模板里 ref 绑定） */
const overlayEl = ref<HTMLCanvasElement | null>(null);

const painting = ref(false);
const drawing = ref(false);
/** 拖动起点（蒙版像素坐标） */
let startX = 0;
let startY = 0;
/** 实时拖动矩形（显示坐标，用于预览） */
const dragRect = ref<{ x: number; y: number; w: number; h: number } | null>(null);

/** 蒙版是否为空（全透明 = 没画过 = 未选中） */
const empty = ref(true);


/** 把画布尺寸对齐到蒙版分辨率。 */
function resizeCanvases(): void {
  if (!props.maskWidth || !props.maskHeight) return;
  for (const c of [maskCanvas, viewCanvas]) {
    if (c.width !== props.maskWidth || c.height !== props.maskHeight) {
      // 改 width/height 会重置画布内容 —— 所以只在真的需要时改
      c.width = props.maskWidth;
      c.height = props.maskHeight;
    }
  }
  // 初始为全透明：透明 = 未选中
  maskCtx.clearRect(0, 0, props.maskWidth, props.maskHeight);
  empty.value = true;
  redraw();
}

/** 把蒙版画到预览层：半透明红 = 选中。 */
function redraw(): void {
  const w = props.maskWidth;
  const h = props.maskHeight;
  if (!w || !h) return;
  resizeCanvases();
  viewCtx.clearRect(0, 0, w, h);
  viewCtx.drawImage(maskCanvas, 0, 0);

  // 用「红色源-在」把蒙版的白色区域染红；再整体降低不透明度，
  // 这样用户能同时看清选区与底下的图像。
  viewCtx.globalCompositeOperation = "source-in";
  viewCtx.fillStyle = "#ff3b30";
  viewCtx.fillRect(0, 0, w, h);
  viewCtx.globalCompositeOperation = "source-over";
  viewCtx.globalAlpha = 0.35;
  viewCtx.drawImage(viewCanvas, 0, 0);
  viewCtx.globalAlpha = 1;

  // 同步到 DOM canvas（DOM canvas 用显示分辨率，靠 CSS 拉伸）
  const el = overlayEl.value;
  if (el) {
    if (el.width !== w || el.height !== h) {
      el.width = w;
      el.height = h;
    }
    const ectx = el.getContext("2d")!;
    ectx.clearRect(0, 0, w, h);
    ectx.drawImage(viewCanvas, 0, 0);
  }
}

/** 显示坐标 → 蒙版像素坐标。 */
function toMaskCoords(e: PointerEvent): { x: number; y: number } {
  const el = overlayEl.value;
  if (!el) return { x: 0, y: 0 };
  const r = el.getBoundingClientRect();
  const sx = props.maskWidth / r.width;
  const sy = props.maskHeight / r.height;
  return {
    x: Math.max(0, Math.min(props.maskWidth, (e.clientX - r.left) * sx)),
    y: Math.max(0, Math.min(props.maskHeight, (e.clientY - r.top) * sy)),
  };
}

function onDown(e: PointerEvent): void {
  if (!props.enabled || !props.maskWidth) return;
  painting.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  const p = toMaskCoords(e);
  startX = p.x;
  startY = p.y;

  if (props.tool === "rect") {
    dragRect.value = { x: p.x * props.scale, y: p.y * props.scale, w: 0, h: 0 };
  } else {
    drawing.value = true;
    paintAt(p.x, p.y);
  }
}

function onMove(e: PointerEvent): void {
  if (!painting.value) return;
  const p = toMaskCoords(e);

  if (props.tool === "rect") {
    const x = Math.min(startX, p.x);
    const y = Math.min(startY, p.y);
    const w = Math.abs(p.x - startX);
    const h = Math.abs(p.y - startY);
    dragRect.value = { x: x * props.scale, y: y * props.scale, w: w * props.scale, h: h * props.scale };
    return;
  }
  if (drawing.value) {
    paintAt(p.x, p.y);
    redraw();
  }
}

function onUp(e: PointerEvent): void {
  if (!painting.value) return;
  painting.value = false;
  try {
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  } catch {
    /* 指针已释放 */
  }

  if (props.tool === "rect") {
    const r = dragRect.value;
    dragRect.value = null;
    if (r && r.w > 2 && r.h > 2) {
      // 换算回蒙版像素再落笔
      maskCtx.fillStyle = "#ffffff";
      maskCtx.fillRect(r.x / props.scale, r.y / props.scale, r.w / props.scale, r.h / props.scale);
      empty.value = false;
      redraw();
      emit("change");
    }
    return;
  }

  drawing.value = false;
  emit("change");
}

/** 画一个圆点（画笔/橡皮）。 */
function paintAt(x: number, y: number): void {
  const r = Math.max(1, props.brushSize / 2 / props.scale);
  maskCtx.globalCompositeOperation = "source-over";
  maskCtx.fillStyle = props.tool === "eraser" ? "rgba(0,0,0,0)" : "#ffffff";
  if (props.tool === "eraser") {
    // 橡皮 = 擦成透明（未选中）。destination-out 才是「擦掉」，
    // 用透明黑覆盖只会得到一个 0 透明度的黑块。
    maskCtx.globalCompositeOperation = "destination-out";
  }
  maskCtx.beginPath();
  maskCtx.arc(x, y, r, 0, Math.PI * 2);
  maskCtx.fill();
  maskCtx.globalCompositeOperation = "source-over";
  if (props.tool !== "eraser") empty.value = false;
}

/** 清除整个蒙版。 */
function clearAll(): void {
  if (!props.enabled) return;
  maskCtx.clearRect(0, 0, props.maskWidth, props.maskHeight);
  empty.value = true;
  dragRect.value = null;
  redraw();
}

/** 导出为 PNG 字节（提交给后端）。 */
async function toPngBlob(): Promise<Blob | null> {
  if (empty.value || !props.maskWidth) return null;
  return await new Promise<Blob | null>((resolve) => {
    maskCanvas.toBlob((b) => resolve(b), "image/png");
  });
}

/** 从已有 PNG 恢复（重新打开图片时）。 */
async function loadFromPng(blob: Blob): Promise<void> {
  const bmp = await createImageBitmap(blob);
  resizeCanvases();
  maskCtx.clearRect(0, 0, props.maskWidth, props.maskHeight);
  // 蒙版可能与当前预览分辨率不同（换了图/换了档位），按比例拉伸对齐
  maskCtx.drawImage(bmp, 0, 0, props.maskWidth, props.maskHeight);
  bmp.close();
  empty.value = false;
  redraw();
}


defineExpose({ toPngBlob, loadFromPng, clearAll });

watch(
  () => [props.maskWidth, props.maskHeight],
  () => {
    if (props.maskWidth && props.maskHeight) resizeCanvases();
  },
);

onBeforeUnmount(() => {
  // 释放 objectURL 之外无资源；canvas 由 GC 回收
});
</script>

<template>
  <div class="mask-layer">
    <canvas
      ref="overlayEl"
      class="mask-canvas"
      :class="{ disabled: !enabled, drawing: painting }"
      @pointerdown="onDown"
      @pointermove="onMove"
      @pointerup="onUp"
      @pointercancel="onUp"
    />

    <!-- 框选实时预览 -->
    <div
      v-if="dragRect"
      class="mask-rect"
      :style="{
        left: `${dragRect.x}px`,
        top: `${dragRect.y}px`,
        width: `${dragRect.w}px`,
        height: `${dragRect.h}px`,
      }"
    />

    <!-- 画布上只留一条状态提示。工具选择与参数都在右侧面板
         （对齐 Lightroom：点「局部」→ 面板选新建方式 → 回画布绘制），
         画布上再摆一排图标会与面板重复，且两处状态容易不同步。 -->
    <div class="mask-hint-bar">
      <span v-if="empty">在画面上框选或涂抹一片区域，即可只调整选区内</span>
      <span v-else>选区已生效 · 在右侧面板调羽化与强度</span>
    </div>
  </div>
</template>

<style scoped>
.mask-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

.mask-canvas {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  pointer-events: auto;
  touch-action: none;
  cursor: crosshair;
}

.mask-canvas.disabled {
  pointer-events: none;
}

.mask-canvas.drawing {
  cursor: grabbing;
}

.mask-rect {
  position: absolute;
  border: 1.5px dashed #ff6b60;
  background: rgba(255, 107, 96, 0.16);
  pointer-events: none;
}

/* 画布上的状态提示：只说明「现在该做什么」，不承载交互 */
.mask-hint-bar {
  position: absolute;
  left: 50%;
  bottom: 14px;
  transform: translateX(-50%);
  padding: 6px 14px;
  border-radius: 999px;
  background: rgba(24, 24, 30, 0.72);
  backdrop-filter: blur(8px);
  border: 1px solid rgba(255, 255, 255, 0.14);
  color: #f2f2f5;
  font-size: 12px;
  white-space: nowrap;
  pointer-events: none;
}
</style>
