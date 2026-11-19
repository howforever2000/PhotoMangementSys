<script setup lang="ts">
/**
 * 工作台 · 底部胶片条（FEAT-180）
 * ===================================================================
 * LR 式的一条横向缩略图带：滚轮/按钮横滑，当前图自动滚进视野中心。
 *
 * 为什么不再用网格铺满中栏：网格会把对比大图往下推，一张 3:2 照片只能拿到
 * 420px 高（FEAT-177 的 `max-height`），用户看不清修图细节。现在对比图吃满
 * 剩余高度，照片来源收进这一条；要挑图时切到网格视图。
 *
 * 纯展示 + 事件：状态、顺序、样片归属都由页面/store 决定。
 */
import { nextTick, onBeforeUnmount, ref, watch } from "vue";

import type { WbThumbItem } from "../../../types/workbench";
import { thumbStatusText } from "../../../utils/workbenchThumb";

const props = withDefaults(
  defineProps<{
    items: WbThumbItem[];
    currentPath: string;
    /** 自选样片模式：点格子是勾样片，不是换对比对象 */
    picking?: boolean;
    disabled?: boolean;
  }>(),
  { picking: false, disabled: false },
);

const emit = defineEmits<{
  pick: [path: string];
  "toggle-select": [path: string];
}>();

const strip = ref<HTMLElement | null>(null);

function statusText(it: WbThumbItem): string {
  return thumbStatusText(it, "compact");
}

/** 当前图滚到可视区中间：换图后不需要用户自己去找它 */
function revealCurrent(): void {
  const box = strip.value;
  if (!box || !props.currentPath) return;
  const cell = box.querySelector<HTMLElement>(`[data-path="${CSS.escape(props.currentPath)}"]`);
  if (!cell) return;
  // 用 rect 差值而不是 offsetLeft：胶片条本身有左右箭头与内边距，
  // offsetLeft 相对的是外层定位容器，会把格子整体偏掉一截。
  const cellRect = cell.getBoundingClientRect();
  const boxRect = box.getBoundingClientRect();
  const left =
    box.scrollLeft + (cellRect.left - boxRect.left) - (box.clientWidth - cellRect.width) / 2;
  box.scrollTo({ left: Math.max(0, left), behavior: "smooth" });
}

watch(
  () => props.currentPath,
  () => void nextTick(revealCurrent),
  { immediate: true },
);

/** 竖向滚轮 → 横向滑动（触控板横向手势本来就是 deltaX，直接相加）。
    触控板的 wheel 事件密度远高于刷新率，按 rAF 合并成每帧一次写入。 */
let wheelPending = 0;
let wheelRaf = 0;
function onWheel(e: WheelEvent): void {
  const box = strip.value;
  if (!box) return;
  wheelPending += Math.abs(e.deltaY) > Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
  if (wheelRaf) return;
  wheelRaf = requestAnimationFrame(() => {
    wheelRaf = 0;
    const delta = wheelPending;
    wheelPending = 0;
    if (strip.value) strip.value.scrollLeft += delta;
  });
}

onBeforeUnmount(() => {
  if (wheelRaf) cancelAnimationFrame(wheelRaf);
  wheelRaf = 0;
});

function scrollByPage(dir: -1 | 1): void {
  const box = strip.value;
  if (!box) return;
  box.scrollBy({ left: dir * Math.max(160, box.clientWidth * 0.7), behavior: "smooth" });
}

function onCellKey(e: KeyboardEvent, path: string): void {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    emit("pick", path);
  }
}
</script>

<template>
  <div class="fs" :class="{ picking }">
    <button class="fs-arrow l" type="button" title="向左翻" @click="scrollByPage(-1)">‹</button>

    <div ref="strip" class="fs-strip" @wheel.prevent="onWheel">
      <div
        v-for="it in items"
        :key="it.path"
        class="fs-cell"
        :class="{ on: it.path === currentPath, sample: it.sample, done: it.status === 'done' }"
        :data-path="it.path"
        role="button"
        :tabindex="disabled ? -1 : 0"
        :title="it.name"
        @click="emit('pick', it.path)"
        @keydown="onCellKey($event, it.path)"
      >
        <img v-if="it.src" :src="it.src" :alt="it.name" loading="lazy" draggable="false" />
        <span v-else class="fs-ph" />

        <span v-if="statusText(it)" class="fs-badge" :class="it.status">{{ statusText(it) }}</span>
        <span v-if="it.sample" class="fs-sample">样</span>
        <button
          class="fs-check"
          :class="{ on: it.selected }"
          type="button"
          :title="it.selected ? '取消勾选' : '勾选（批量套用目标）'"
          @click.stop="emit('toggle-select', it.path)"
        >
          ✓
        </button>
        <div v-if="it.status === 'running' || it.status === 'queued'" class="fs-prog">
          <i :style="{ width: `${it.progress}%` }" />
        </div>
      </div>
    </div>

    <button class="fs-arrow r" type="button" title="向右翻" @click="scrollByPage(1)">›</button>
  </div>
</template>

<style scoped>
.fs {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  padding: 8px 0 2px;
  border-top: 1px solid var(--c-rule);
}
.fs-strip {
  display: flex;
  align-items: center;
  gap: 7px;
  flex: 1;
  min-width: 0;
  overflow-x: auto;
  overflow-y: hidden;
  padding: 2px 2px 6px;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.16) transparent;
  scroll-behavior: smooth;
}
.fs-strip::-webkit-scrollbar {
  height: 6px;
}
.fs-strip::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.14);
  border-radius: 3px;
}

.fs-cell {
  position: relative;
  flex: none;
  width: 96px;
  height: 64px;
  border-radius: 7px;
  overflow: hidden;
  background: var(--hover, rgba(120, 120, 130, 0.1));
  box-shadow: inset 0 0 0 1px var(--c-rule);
  cursor: pointer;
  outline: none;
}
.fs-cell:hover {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.fs-cell.on {
  box-shadow:
    inset 0 0 0 2px var(--c-accent),
    0 0 0 1px rgba(0, 0, 0, 0.4);
}
.fs-cell:focus-visible {
  box-shadow:
    inset 0 0 0 2px var(--c-accent),
    0 0 0 2px color-mix(in srgb, var(--c-accent) 35%, transparent);
}
.fs-cell img,
.fs-ph {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.fs-ph {
  position: absolute;
  inset: 0;
  background: var(--hover, rgba(120, 120, 130, 0.1));
}

.fs-badge {
  position: absolute;
  left: 4px;
  bottom: 4px;
  padding: 0 5px;
  border-radius: 999px;
  font-size: 8.5px;
  line-height: 1.7;
  /* 压暗色角标：固定浅色字，浅色主题下也可读 */
  color: rgba(255, 255, 255, 0.78);
  background: rgba(10, 9, 13, 0.72);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
  font-variant-numeric: tabular-nums;
}
.fs-badge.done {
  color: #9fe0a8;
}
.fs-badge.failed {
  color: #ff9a95;
}
.fs-badge.running,
.fs-badge.queued {
  color: #f3d08c;
}
.fs-sample {
  position: absolute;
  right: 22px;
  top: 4px;
  padding: 0 4px;
  border-radius: 4px;
  font-size: 8.5px;
  font-weight: 700;
  color: #1a1608;
  background: var(--c-accent);
}
.fs-check {
  position: absolute;
  top: 4px;
  left: 4px;
  width: 15px;
  height: 15px;
  padding: 0;
  border-radius: 4px;
  border: 1px solid rgba(255, 255, 255, 0.55);
  background: rgba(10, 9, 13, 0.6);
  color: transparent;
  font-size: 9px;
  line-height: 1;
  display: grid;
  place-items: center;
  cursor: pointer;
}
.fs-check.on {
  background: var(--c-accent);
  border-color: var(--c-accent);
  color: #1a1608;
}
.fs-prog {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  background: rgba(0, 0, 0, 0.4);
}
.fs-prog i {
  display: block;
  height: 100%;
  background: var(--c-accent);
}

.fs-arrow {
  flex: none;
  width: 20px;
  height: 56px;
  padding: 0;
  border: none;
  border-radius: 6px;
  background: none;
  color: var(--fg-2);
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
}
.fs-arrow:hover {
  color: var(--fg);
  background: var(--hover);
}
.fs.picking .fs-cell {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 30%, transparent);
}
</style>
