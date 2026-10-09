<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { CurvePoint, EditCurveSpec } from "../../../types/edit";

/**
 * 色调曲线编辑器（FEAT-099）
 * ===================================================================
 * 交互对齐 Lightroom 的「曲线」面板：
 *
 * - 对角灰线是恒等基准；控制点可拖动；
 * - **点击轨道空白处添加控制点**（这是最关键的发现方式，
 *   比「右键 → 添加点」快得多）；
 * - **拖动端点（首/末点）只改 y**，x 固定在 0 / 1 —— 否则「只调高光」
 *   会被曲线悄悄改了对比度；
 * - **点击控制点后按 Delete / 右键**删除；
 * - 灰色小圆点（9 个）是「按亮度等距的采样提示」，与 Lightroom 一致，
 *   方便用户对齐自己熟悉的色调曲线预设。
 *
 * ## 为什么 RGB 曲线允许「拖动中间点」，端点却不允许改 x
 *
 * 中间点的 x 位置就是「这个亮度点的位置」，用户想在那里改斜率；
/// 而首/末点的 x 就是黑场/白场本身，改它等于改对比度 ——
 * 那已经有独立的「对比度」滑块了，两者会打架。
 */
const props = defineProps<{
  spec: EditCurveSpec;
  points: CurvePoint[];
  /** 拖动中置 true（用于在轨道上绘制活动曲线） */
  active?: boolean;
}>();

const emit = defineEmits<{
  (e: "begin"): void;
  /** 拖动中的实时值 */
  (e: "update:points", pts: CurvePoint[]): void;
  /** 一次操作结束 */
  (e: "commit:points", pts: CurvePoint[]): void;
}>();

const canvas = ref<HTMLCanvasElement | null>(null);
const wrap = ref<HTMLElement | null>(null);

const SIZE = 220;
const PAD = 10;
/** 控制点命中半径（CSS 像素）。 */
const HIT = 9;

/** 拖动中的控制点下标；-1 = 未拖动 */
const dragIndex = ref(-1);
/** 悬停的控制点下标 */
const hoverIndex = ref(-1);

/** 始终存在的隐式端点（用于绘制对角参考线与端点拖动）。 */
const hasAny = computed(() => props.points.length > 0);

/** 当前可见的控制点（含隐式端点）。 */
const visiblePoints = computed<CurvePoint[]>(() => {
  const pts = [...props.points];
  if (pts.length === 0) return pts;
  if (pts[0].x > 0.001) pts.unshift({ x: 0, y: 0 });
  if (pts[pts.length - 1].x < 0.999) pts.push({ x: 1, y: 1 });
  return pts;
});

/** 是否有可编辑的端点（用户至少放了一个内部点）。 */
const endpointsEditable = computed(() => props.points.length > 0);

/* ---------------- 坐标换算 ---------------- */

const plotW = SIZE - PAD * 2;

function toCanvasX(x: number): number {
  return PAD + x * plotW;
}
function toCanvasY(y: number): number {
  return SIZE - PAD - y * plotW;
}
function fromCanvasX(px: number): number {
  return clamp01((px - PAD) / plotW);
}
function fromCanvasY(py: number): number {
  return clamp01((SIZE - PAD - py) / plotW);
}

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

/* ---------------- 绘制 ---------------- */

/**
 * 曲线形状用与后端一致的 Catmull-Rom 采样。
 *
 * 前端必须用**同一套插值**，否则用户拖动时看到的曲线与实际渲染不一致 ——
 * 这是曲线编辑器最容易被察觉的缺陷（点对了位置，出来的影调不对）。
 */
function catmullRom(points: CurvePoint[], x: number): number {
  const n = points.length;
  if (n < 2) return x;
  const p = (i: number): CurvePoint => {
    if (i < 0) {
      const a = points[0];
      const b = points[1];
      return { x: 2 * a.x - b.x, y: 2 * a.y - b.y };
    }
    if (i >= n) {
      const a = points[n - 1];
      const b = points[n - 2];
      return { x: 2 * a.x - b.x, y: 2 * a.y - b.y };
    }
    return points[i];
  };
  let seg = 0;
  for (let j = 0; j < n - 1; j++) if (x >= points[j].x) seg = j;
  const p0 = p(seg - 1);
  const p1 = p(seg);
  const p2 = p(seg + 1);
  const p3 = p(seg + 2);
  const span = p2.x - p1.x;
  const t = Math.abs(span) < 1e-9 ? 0 : (x - p1.x) / span;
  const y =
    0.5 *
    (2 * p1.y +
      (-p0.y + p2.y) * t +
      (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t * t +
      (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t * t * t);
  return clamp01(y);
}

function draw(): void {
  const c = canvas.value;
  if (!c) return;
  const dpr = window.devicePixelRatio || 1;
  if (c.width !== SIZE * dpr || c.height !== SIZE * dpr) {
    c.width = SIZE * dpr;
    c.height = SIZE * dpr;
  }
  const g = c.getContext("2d");
  if (!g) return;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, SIZE, SIZE);

  // 背景
  g.fillStyle = "rgba(128,128,138,0.08)";
  g.fillRect(PAD, PAD, plotW, plotW);

  // 四分网格
  g.strokeStyle = "rgba(128,128,138,0.18)";
  g.lineWidth = 1;
  for (let i = 1; i < 4; i++) {
    const p = PAD + (plotW * i) / 4;
    g.beginPath();
    g.moveTo(p, PAD);
    g.lineTo(p, SIZE - PAD);
    g.moveTo(PAD, p);
    g.lineTo(SIZE - PAD, p);
    g.stroke();
  }

  // 对角参考线（恒等）
  g.strokeStyle = "rgba(128,128,138,0.4)";
  g.setLineDash([3, 3]);
  g.beginPath();
  g.moveTo(PAD, SIZE - PAD);
  g.lineTo(SIZE - PAD, PAD);
  g.stroke();
  g.setLineDash([]);

  // 边框
  g.strokeStyle = "rgba(128,128,138,0.35)";
  g.strokeRect(PAD + 0.5, PAD + 0.5, plotW - 1, plotW - 1);

  if (!hasAny.value) {
    g.fillStyle = "rgba(128,128,138,0.5)";
    g.font = '11px system-ui, -apple-system, "PingFang SC", sans-serif';
    g.textAlign = "center";
    g.fillText("点击曲线添加控制点", SIZE / 2, SIZE / 2 - 6);
    g.fillText("拖动调整 · 双击删除", SIZE / 2, SIZE / 2 + 10);
    drawHandles(g);
    return;
  }

  // 曲线本体
  const pts = visiblePoints.value;
  g.strokeStyle = "#e8ecf4";
  g.lineWidth = 1.6;
  g.beginPath();
  for (let i = 0; i <= 120; i++) {
    const x = i / 120;
    const y = catmullRom(pts, x);
    const px = toCanvasX(x);
    const py = toCanvasY(y);
    if (i === 0) g.moveTo(px, py);
    else g.lineTo(px, py);
  }
  g.stroke();

  // 9 个等亮度采样点（Lightroom 的灰点提示）
  g.fillStyle = "rgba(200,206,218,0.55)";
  for (let i = 0; i <= 8; i++) {
    const x = i / 8;
    const y = catmullRom(pts, x);
    g.beginPath();
    g.arc(toCanvasX(x), toCanvasY(y), 1.4, 0, Math.PI * 2);
    g.fill();
  }

  drawHandles(g);
}

function drawHandles(g: CanvasRenderingContext2D): void {
  const pts = visiblePoints.value;
  pts.forEach((p, i) => {
    const isHover = i === hoverIndex.value;
    const isDrag = i === dragIndex.value;
    const editable = i > 0 && i < pts.length - 1 ? true : endpointsEditable.value;
    const r = isDrag ? 5.5 : isHover ? 5 : 4;
    g.beginPath();
    g.arc(toCanvasX(p.x), toCanvasY(p.y), r, 0, Math.PI * 2);
    g.fillStyle = editable
      ? isDrag || isHover
        ? "#ffffff"
        : "#c9d1e0"
      : "rgba(160,166,180,0.6)";
    g.fill();
    g.strokeStyle = "rgba(30,36,48,0.85)";
    g.lineWidth = 1.2;
    g.stroke();
  });
}

/* ---------------- 交互 ---------------- */

function localPos(e: PointerEvent | MouseEvent): { x: number; y: number } {
  const el = canvas.value;
  if (!el) return { x: 0, y: 0 };
  const r = el.getBoundingClientRect();
  // CSS 尺寸固定为 SIZE；按实际缩放比例还原
  const k = SIZE / r.width;
  return { x: (e.clientX - r.left) * k, y: (e.clientY - r.top) * k };
}

function hitTest(px: number, py: number): number {
  const pts = visiblePoints.value;
  for (let i = pts.length - 1; i >= 0; i--) {
    const dx = px - toCanvasX(pts[i].x);
    const dy = py - toCanvasY(pts[i].y);
    if (Math.hypot(dx, dy) <= HIT) return i;
  }
  return -1;
}

function onPointerDown(e: PointerEvent): void {
  if (e.button !== 0) return;
  const { x, y } = localPos(e);
  const idx = hitTest(x, y);

  if (idx < 0) {
    // 空白处：添加控制点
    const cx = fromCanvasX(x);
    const cy = fromCanvasY(y);
    emit("begin");
    const next = upsert(props.points, cx, cy);
    if (next.length !== props.points.length) {
      dragIndex.value = next.findIndex((p) => Math.abs(p.x - cx) < 1e-3);
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      emit("update:points", next);
    } else {
      emit("commit:points", next);
    }
    e.preventDefault();
    return;
  }

  const pts = visiblePoints.value;
  const isEndpoint = idx === 0 || idx === pts.length - 1;
  // 端点存在但用户还没放内部点时，端点不可拖（那是隐式的恒等端点）
  if (isEndpoint && !endpointsEditable.value) return;

  dragIndex.value = idx;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  emit("begin");
  e.preventDefault();
}

function onPointerMove(e: PointerEvent): void {
  const { x, y } = localPos(e);
  if (dragIndex.value < 0) {
    const idx = hitTest(x, y);
    if (idx !== hoverIndex.value) {
      hoverIndex.value = idx;
      draw();
    }
    return;
  }
  const pts = visiblePoints.value;
  const isEndpoint = dragIndex.value === 0 || dragIndex.value === pts.length - 1;
  const cx = isEndpoint ? pts[dragIndex.value].x : fromCanvasX(x);
  const cy = clamp01(fromCanvasY(y));
  emit("update:points", upsert(props.points, cx, cy));
}

function onPointerUp(e: PointerEvent): void {
  if (dragIndex.value < 0) return;
  dragIndex.value = -1;
  try {
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
  } catch {
    /* 已释放 */
  }
  emit("commit:points", props.points);
}

/** 双击控制点 = 删除。 */
function onDoubleClick(e: MouseEvent): void {
  const { x, y } = localPos(e);
  const idx = hitTest(x, y);
  const pts = visiblePoints.value;
  if (idx < 0) return;
  if (idx === 0 || idx === pts.length - 1) return; // 端点不删
  emit("begin");
  emit("commit:points", removeNear(props.points, pts[idx].x));
  e.preventDefault();
}

/**
 * 插入/替换控制点（与后端 `Curve::upsert` 同一套规则）。
 *
 * 前端必须与后端一致，否则「拖动的点」和「后端算的点」会分叉 ——
 * 表现为松手后曲线跳一下。
 */
function upsert(points: CurvePoint[], x: number, y: number): CurvePoint[] {
  const out = points.map((p) => ({ ...p }));
  for (const p of out) {
    if (Math.abs(p.x - x) < 1e-3) {
      p.y = y;
      return out.sort((a, b) => a.x - b.x);
    }
  }
  out.push({ x, y });
  out.sort((a, b) => a.x - b.x);
  return out;
}

function removeNear(points: CurvePoint[], x: number): CurvePoint[] {
  const out = points.filter((p) => Math.abs(p.x - x) >= 1e-3);
  return out.length < 2 ? [] : out;
}

function reset(): void {
  emit("begin");
  emit("commit:points", []);
}

/* ---------------- 响应式 ---------------- */

watch(
  () => [props.points, props.active],
  () => draw(),
  { deep: true },
);

const ro = ref<ResizeObserver | null>(null);
onMountedResize();

function onMountedResize(): void {
  const el = wrap.value;
  if (!el || typeof ResizeObserver === "undefined") {
    draw();
    return;
  }
  const obs = new ResizeObserver(() => draw());
  obs.observe(el);
  ro.value = obs;
}

onBeforeUnmount(() => {
  ro.value?.disconnect();
  ro.value = null;
});
</script>

<template>
  <div class="curve-editor">
    <div class="curve-head">
      <span class="curve-title">{{ spec.label }}</span>
      <button
        v-if="points.length > 0"
        class="curve-clear"
        type="button"
        title="清除该曲线"
        @click="reset"
      >
        清除
      </button>
    </div>
    <div ref="wrap" class="curve-canvas-wrap" :title="spec.help">
      <canvas
        ref="canvas"
        :width="SIZE"
        :height="SIZE"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
        @dblclick="onDoubleClick"
      ></canvas>
    </div>
    <p class="curve-help">{{ spec.help }}</p>
  </div>
</template>

<style scoped>
.curve-editor {
  padding: 4px 0 2px;
}
.curve-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}
.curve-title {
  font-size: 12px;
  color: var(--pm-label, var(--color-text-2));
}
.curve-clear {
  padding: 1px 8px;
  font-size: 11px;
  color: var(--pm-hint, var(--color-text-2));
  background: rgba(128, 128, 138, 0.14);
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 6px;
  cursor: pointer;
}
.curve-clear:hover {
  background: rgba(128, 128, 138, 0.26);
}
.curve-canvas-wrap {
  width: 220px;
  height: 220px;
  border-radius: 8px;
  overflow: hidden;
}
.curve-canvas-wrap canvas {
  width: 100%;
  height: 100%;
  display: block;
  cursor: crosshair;
  touch-action: none;
}
.curve-help {
  margin: 6px 0 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--pm-hint, var(--color-text-2));
}
</style>