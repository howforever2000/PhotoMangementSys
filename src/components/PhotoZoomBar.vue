<script setup lang="ts">
/**
 * FEAT-095（R1）：缩略图矩阵缩放条
 *
 * 需求：智慧相册「按人物分类」点头像出来的照片矩阵预览框不能放大缩小。
 * 本组件把「格子尺寸」这一唯一变量交给用户控制，调用方只需把它接到
 * grid 的 `grid-template-columns` 上 —— 避免了「缩放 = 换一套缩略图尺寸」
 * 这类重复实现（PhotoGrid / PersonGallery / CategoryGallery 三处缩略图
 * 都只有 256px 缓存，放大只是 CSS 拉伸，不会触发重新生成）。
 *
 * 尺寸档位用固定步长而非连续滑块：
 * - 缩略图缓存只有 256px，超过 320px 就是纯插值糊图，给到更大尺寸是欺骗用户；
 * - 步进档位让「点一下就大一档」比拖滑块更可控，也更容易命中缓存尺度。
 */
import { computed } from "vue";

const props = withDefaults(
  defineProps<{
    /** 当前格子最小边长（px） */
    modelValue: number;
    /** 最小格子边长（px） */
    min?: number;
    /** 最大格子边长（px）；上限即缩略图缓存尺度的两倍附近，再大只是糊 */
    max?: number;
    /** 步长（px） */
    step?: number;
    /** 每档显示的列数预览文案（可选） */
    disabled?: boolean;
  }>(),
  { min: 80, max: 320, step: 20, disabled: false },
);

const emit = defineEmits<{ (e: "update:modelValue", v: number): void }>();

/** 归一到步长网格并夹在 [min, max] 内（防止外部传入越界值导致布局崩） */
function normalize(v: number): number {
  const stepped = Math.round(v / props.step) * props.step;
  return Math.min(props.max, Math.max(props.min, stepped));
}

const current = computed(() => normalize(props.modelValue));
const canZoomOut = computed(() => !props.disabled && current.value > props.min);
const canZoomIn = computed(() => !props.disabled && current.value < props.max);

function step(delta: number) {
  const next = normalize(current.value + delta);
  if (next !== current.value) emit("update:modelValue", next);
}

/** 倍率文案：以 160px 为基准（≈ 常见笔记本窗口一屏 7-8 列的观感） */
const ratioText = computed(() => `${Math.round((current.value / 160) * 100)}%`);

defineExpose({ step, normalize });
</script>

<template>
  <div class="pzb" role="group" aria-label="缩略图缩放">
    <button
      class="pzb-btn"
      :disabled="!canZoomOut"
      title="缩小缩略图"
      @click="step(-step)"
    >－</button>
    <input
      class="pzb-range"
      type="range"
      :min="min"
      :max="max"
      :step="step"
      :value="current"
      :disabled="disabled"
      aria-label="缩略图尺寸"
      @input="emit('update:modelValue', normalize(Number(($event.target as HTMLInputElement).value)))"
    />
    <button
      class="pzb-btn"
      :disabled="!canZoomIn"
      title="放大缩略图"
      @click="step(step)"
    >＋</button>
    <span class="pzb-value" :title="`格子边长 ${current}px`">{{ ratioText }}</span>
    <slot name="extra" />
  </div>
</template>

<style scoped>
.pzb {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--color-surface, #fff) 72%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-primary) 24%, transparent);
  backdrop-filter: blur(4px);
}
.pzb-btn {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 1px solid color-mix(in srgb, var(--color-primary) 40%, transparent);
  background: transparent;
  color: inherit;
  font-size: 13px;
  line-height: 1;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background var(--dur-2);
}
.pzb-btn:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-primary) 16%, transparent);
}
.pzb-btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}
.pzb-range {
  width: 88px;
  accent-color: var(--color-primary);
  cursor: pointer;
}
.pzb-range:disabled { cursor: not-allowed; opacity: 0.4; }
.pzb-value {
  font-size: 11px;
  opacity: 0.7;
  min-width: 34px;
  text-align: right;
  font-variant-numeric: tabular-nums;
}
</style>
