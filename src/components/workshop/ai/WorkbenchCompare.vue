<script setup lang="ts">
/**
 * 工作台前后对比框（FEAT-177）
 * ===================================================================
 * 两件事，各自独立：
 *   ① **全图可见**：外层吃满父容器（FEAT-180 起是「中栏剩余高度」，不再锁
 *      16:9/420px），内层 `.cmp-box` 按当前照片的真实宽高比居中 ——
 *      3:2 / 4:5 都不裁切。旧实现是 `object-fit:cover` 写死在 16:9 框里，
 *      竖构图照片上下被切掉一截，用户判断不了构图；固定高度又让竖图只剩
 *      三百多像素宽，看不清修图细节。
 *   ② **拖得动**：整块画面接管指针事件，竖线两侧各留 13px 透明热区（2px 的线
 *      抓不住）。旧实现只有底部一根 `<input type=range>`，画面里那个圆角把手
 *      看着能拖、其实是死的。
 *
 * 纯展示组件：不改数据、不发请求，只把拖动结果 emit 出去。
 *
 * FEAT-181：接入与 PhotoLightbox 同一套缩放（`useImageZoom`）—— Ctrl+滚轮以光标为中心
 * 缩放、放大后拖动平移；缩小到 100%（适应窗口）时拖画面仍是拉对比线，放大后
 * 改由白线自己的热区接管（`@pointerdown.stop`），两者不打架。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { useImageZoom } from "../../../composables/useImageZoom";

const props = withDefaults(
  defineProps<{
    beforeSrc: string;
    /** 效果图（真实产物或即时预览的 data URL）；空 = 还没出片 */
    afterSrc?: string;
    /** 当前照片真实宽高比 w/h */
    aspect?: number;
    /** 分割位置 0~100 */
    pos: number;
    /** 当前缩放倍率（1 = 适应窗口）；供页面在说明文案里区分「拖线」/「平移」 */
    zoom?: number;
    /** 正在后台算预览 */
    previewing?: boolean;
    /** 右下角小标注（如「预览 1280px」） */
    previewNote?: string;
  }>(),
  { afterSrc: "", aspect: 16 / 9, zoom: 1, previewing: false, previewNote: "" },
);

const emit = defineEmits<{
  "update:pos": [value: number];
  "update:zoom": [value: number];
}>();

const frame = ref<HTMLElement | null>(null);
const box = ref<HTMLElement | null>(null);
/** 内框的「适应窗口」尺寸（px），由 layout 计算 */
const boxSize = ref({ w: 0, h: 0 });

const hasSplit = computed(() => Boolean(props.afterSrc));

/* ---------------- 缩放 / 平移（与图像预览共用） ---------------- */
const {
  scale,
  dragging: zoomDragging,
  transform,
  resetView,
  zoomBy,
  onWheel,
  onDragStart,
  onDragMove,
  onDragEnd,
} = useImageZoom({ min: 1, max: 8, requireCtrl: true, anchorToCursor: true });

watch(scale, (v) => emit("update:zoom", v));
watch(
  () => props.beforeSrc,
  () => resetView(),
);

/** 内框按图片比例排：min(视口宽, 视口高 × 比例)，避免出现形变或裁切 */
function layout(): void {
  const el = frame.value;
  if (!el) return;
  const w = el.clientWidth;
  const h = el.clientHeight;
  if (!w || !h) return;
  const ar = props.aspect > 0 ? props.aspect : 16 / 9;
  const bw = Math.min(w, h * ar);
  boxSize.value = { w: Math.round(bw), h: Math.round(bw / ar) };
  // 窗口/侧栏尺寸变了，按旧倍率算出来的平移会错位；回到适应窗口
  if (scale.value > 1) resetView();
}

let ro: ResizeObserver | null = null;
onMounted(() => {
  layout();
  if (frame.value) {
    ro = new ResizeObserver(layout);
    ro.observe(frame.value);
  }
});
onBeforeUnmount(() => ro?.disconnect());
watch(() => props.aspect, layout);

const boxStyle = computed<Record<string, string>>(() => ({
  width: `${boxSize.value.w}px`,
  height: `${boxSize.value.h}px`,
  marginLeft: `${-boxSize.value.w / 2}px`,
  marginTop: `${-boxSize.value.h / 2}px`,
  transform: transform.value,
  // 白线/把手在放大时会被 box 一起放大；CSS 用 1/--z 抵消，保持 2px 线与 30px 把手
  "--z": String(scale.value),
}));

/* ---------------- 对比线拖拽 ---------------- */
let splitDragging = false;

function applyX(clientX: number): void {
  const r = box.value?.getBoundingClientRect();
  if (!r || !r.width) return;
  const v = Math.round(((clientX - r.left) / r.width) * 100);
  emit("update:pos", Math.max(0, Math.min(100, v)));
}

function startSplit(e: PointerEvent): void {
  if (!hasSplit.value) return;
  splitDragging = true;
  frame.value?.setPointerCapture?.(e.pointerId);
  applyX(e.clientX);
}

/** 白线自己的 pointerdown：放大时不再让整块画面抢走这次拖拽 */
function onDividerDown(e: PointerEvent): void {
  e.stopPropagation();
  startSplit(e);
}

function onPointerDown(e: PointerEvent): void {
  if (e.button !== 0) return;
  // 放大后：拖任意处 = 平移；拖白线 = 对比（白线自己 stop 了）
  if (scale.value > 1) {
    onDragStart(e);
    return;
  }
  startSplit(e);
}

function onPointerMove(e: PointerEvent): void {
  if (zoomDragging.value) {
    onDragMove(e);
    return;
  }
  if (splitDragging) applyX(e.clientX);
}

function onPointerUp(e: PointerEvent): void {
  if (zoomDragging.value) onDragEnd();
  if (splitDragging) {
    splitDragging = false;
    frame.value?.releasePointerCapture?.(e.pointerId);
  }
}

/** Ctrl+滚轮：以光标为不动点缩放（页面不滚动，普通滚轮不拦截）。
    必须**同步** preventDefault（否则默认滚动已经发生），缩放本体按 rAF 合并 ——
    触控板的 wheel 密度远高于刷新率，逐事件 applyScale 会白算好几次。 */
let wheelEvent: WheelEvent | null = null;
let wheelRaf = 0;
function onViewerWheel(e: WheelEvent): void {
  if (!e.ctrlKey) return;
  e.preventDefault();
  wheelEvent = e;
  if (wheelRaf) return;
  wheelRaf = requestAnimationFrame(() => {
    wheelRaf = 0;
    const ev = wheelEvent;
    wheelEvent = null;
    const el = frame.value;
    if (!ev || !el) return;
    const r = el.getBoundingClientRect();
    onWheel(ev, { x: ev.clientX, y: ev.clientY }, { x: r.left + r.width / 2, y: r.top + r.height / 2 });
  });
}

onBeforeUnmount(() => {
  if (wheelRaf) cancelAnimationFrame(wheelRaf);
  wheelRaf = 0;
});

/** 键盘替代路径：拉不开时用方向键也能比 */
function onKey(e: KeyboardEvent): void {
  if (!hasSplit.value) return;
  const step = e.shiftKey ? 10 : 1;
  if (e.key === "ArrowLeft") {
    emit("update:pos", Math.max(0, props.pos - step));
  } else if (e.key === "ArrowRight") {
    emit("update:pos", Math.min(100, props.pos + step));
  } else if (e.key === "Home") {
    emit("update:pos", 0);
  } else if (e.key === "End") {
    emit("update:pos", 100);
  } else {
    return;
  }
  e.preventDefault();
}
</script>

<template>
  <div
    ref="frame"
    class="wbc"
    :class="{
      split: hasSplit && scale === 1,
      pannable: scale > 1,
      panning: zoomDragging,
    }"
    :tabindex="hasSplit ? 0 : -1"
    role="slider"
    aria-label="前后对比"
    :aria-valuenow="pos"
    aria-valuemin="0"
    aria-valuemax="100"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
    @wheel="onViewerWheel"
    @keydown="onKey"
  >
    <div ref="box" class="wbc-box" :style="boxStyle">
      <img class="wbc-img before" :src="beforeSrc" alt="原图" draggable="false" />
      <img
        v-if="hasSplit"
        class="wbc-img after"
        :src="afterSrc"
        alt="效果"
        draggable="false"
        :style="{ clipPath: `inset(0 0 0 ${pos}%)` }"
      />
      <span
        v-if="hasSplit"
        class="wbc-div"
        :style="{ left: `${pos}%` }"
        @pointerdown.stop="onDividerDown"
      >
        <span class="wbc-knob">↔</span>
      </span>
    </div>

    <span class="wbc-tag l">原图</span>
    <span v-if="hasSplit" class="wbc-tag r">效果</span>
    <span v-else class="wbc-empty"><slot name="empty">尚未出片</slot></span>
    <span v-if="previewing" class="wbc-busy">预览中…</span>

    <div v-if="beforeSrc" class="wbc-zoom" @pointerdown.stop @wheel.stop>
      <button
        type="button"
        class="wbc-zoom-btn"
        :disabled="scale <= 1"
        title="缩小（Ctrl+滚轮）"
        @click="zoomBy(1 / 1.2)"
      >
        －
      </button>
      <button
        type="button"
        class="wbc-zoom-pct"
        title="复原为适应窗口（100%）"
        @click="resetView"
      >
        {{ Math.round(scale * 100) }}%
      </button>
      <button
        type="button"
        class="wbc-zoom-btn"
        :disabled="scale >= 8"
        title="放大（Ctrl+滚轮）"
        @click="zoomBy(1.2)"
      >
        ＋
      </button>
    </div>

    <span v-if="previewNote" class="wbc-note">{{ previewNote }}</span>
  </div>
</template>

<style scoped>
/* 视口吃满父容器：高度由页面（中栏剩余高度）决定，换图不跳尺寸。
   用 absolute + inset 而不是 height:100%：父级高度来自 flex/grid 的分配时，
   百分比高度在部分布局下会被当成「不确定」而退化成 min-height（窄窗实测只有 180px）。
   宿主要提供 position:relative 的定位盒（`.wb-viewer`）。 */
.wbc {
  position: absolute;
  inset: 0;
  min-height: 180px;
  margin: 0;
  border-radius: var(--r-ui);
  overflow: hidden;
  background: #08070d;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  outline: none;
}
.wbc.split {
  cursor: ew-resize;
  touch-action: none;
  user-select: none;
}
/* 放大后：拖画面平移；白线自己保持 ew-resize（见 .wbc-div） */
.wbc.pannable {
  cursor: grab;
  touch-action: none;
  user-select: none;
}
.wbc.panning {
  cursor: grabbing;
}
.wbc.split:focus-visible {
  box-shadow:
    inset 0 0 0 1px var(--c-rule),
    0 0 0 2px color-mix(in srgb, var(--c-accent) 45%, transparent);
}

/* 居中靠 JS 下发的负 margin，transform 只留给缩放/平移：
   这样 transform-origin:center 就是「内框中心」，以光标为锚点缩放时算式才对。 */
.wbc-box {
  position: absolute;
  top: 50%;
  left: 50%;
  overflow: hidden;
  transform-origin: center center;
}
.wbc-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.wbc-img.before {
  filter: saturate(0.94) brightness(0.98);
}

/* 白线在放大后会跟着 box 一起缩放：用 --z（= 当前倍率）把线宽/热区/把手反向抵消，
   视觉上始终是 2px 线与 30px 把手。pointer-events:auto = 放大后由白线自己接管对比拖拽。 */
.wbc-div {
  position: absolute;
  top: 0;
  bottom: 0;
  width: calc(2px / var(--z, 1));
  transform: translateX(calc(-1px / var(--z, 1)));
  background: rgba(255, 255, 255, 0.85);
  box-shadow: 0 0 10px rgba(0, 0, 0, 0.4);
  pointer-events: auto;
  cursor: ew-resize;
  touch-action: none;
}
/* 2px 的线抓不住：左右各铺 13px 透明热区（随倍率反向缩放） */
.wbc-div::before {
  content: "";
  position: absolute;
  top: 0;
  bottom: 0;
  left: calc(-13px / var(--z, 1));
  width: calc(28px / var(--z, 1));
}
.wbc-knob {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%) scale(calc(1 / var(--z, 1)));
  width: 30px;
  height: 30px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 12px;
  color: #fff;
  background: rgba(20, 18, 26, 0.8);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.4);
}

.wbc-tag,
.wbc-busy,
.wbc-note,
.wbc-zoom {
  z-index: 2;
}
.wbc-tag {
  position: absolute;
  top: 10px;
  padding: 2px 8px;
  border-radius: var(--r-pill);
  font-size: 10px;
  /* 中间查看器始终是黑底：这些浮层不跟主题文字色走（浅色主题下也要白字） */
  color: #fff;
  background: rgba(10, 9, 13, 0.62);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
}
.wbc-tag.l {
  left: 10px;
}
.wbc-tag.r {
  right: 10px;
}
.wbc-empty {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: var(--fs-meta);
  color: rgba(255, 255, 255, 0.58);
  pointer-events: none;
}
.wbc-busy {
  position: absolute;
  top: 10px;
  left: 50%;
  transform: translateX(-50%);
  padding: 2px 10px;
  border-radius: var(--r-pill);
  font-size: 10px;
  color: #f2d79b;
  background: rgba(10, 9, 13, 0.66);
  box-shadow: inset 0 0 0 1px rgba(242, 215, 155, 0.45);
  animation: wbc-pulse 1s ease-in-out infinite;
}
.wbc-note {
  position: absolute;
  right: 10px;
  bottom: 10px;
  padding: 2px 8px;
  border-radius: var(--r-pill);
  font-size: 9.5px;
  color: rgba(255, 255, 255, 0.72);
  background: rgba(10, 9, 13, 0.55);
}

/* 缩放工具条：100% = 适应窗口；点百分比复原 */
.wbc-zoom {
  position: absolute;
  left: 10px;
  bottom: 10px;
  display: flex;
  align-items: center;
  gap: 1px;
  padding: 2px 3px;
  border-radius: var(--r-pill);
  background: rgba(10, 9, 13, 0.66);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
}
.wbc-zoom-btn {
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  border-radius: 50%;
  display: grid;
  place-items: center;
  background: none;
  color: rgba(255, 255, 255, 0.88);
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
}
.wbc-zoom-btn:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.12);
}
.wbc-zoom-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.wbc-zoom-pct {
  min-width: 42px;
  height: 22px;
  padding: 0 4px;
  border: none;
  border-radius: var(--r-pill);
  background: none;
  color: rgba(255, 255, 255, 0.7);
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  cursor: pointer;
}
.wbc-zoom-pct:hover {
  color: #f2d79b;
}
@keyframes wbc-pulse {
  0%,
  100% {
    opacity: 0.55;
  }
  50% {
    opacity: 1;
  }
}
</style>

