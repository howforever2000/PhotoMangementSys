<script setup lang="ts">
/**
 * GaugeCore —— 环形进度芯（FEAT-110）
 * 原子职责：给定 0~1 的比例，画环 + 中心读数 + 右侧可选说明（默认插槽）。
 * variant：accent = 强调渐变；ok = 语义成功色（用于「完成度」类指标）。
 * 每实例独立渐变 id（同屏多个环不能共用一个 <linearGradient> 节点）。
 */
import { computed } from "vue";

let seq = 0;
const gid = `wg-${++seq}-${Math.random().toString(36).slice(2, 7)}`;

const props = withDefaults(
  defineProps<{
    /** 比例 0~1 */
    ratio: number;
    /** 中心主读数文本（默认百分比） */
    valueText?: string;
    /** 中心小字 */
    subText?: string;
    variant?: "accent" | "ok";
    /** 环直径 px */
    size?: number;
  }>(),
  { valueText: "", subText: "", variant: "accent", size: 78 },
);

const R = 32;
const C = 2 * Math.PI * R;
const clamped = computed(() => Math.min(1, Math.max(0, props.ratio)));
const dashOffset = computed(() => C * (1 - clamped.value));
const center = computed(() => (props.valueText || `${(clamped.value * 100).toFixed(1)}%`));
const stroke = computed(() => (props.variant === "ok" ? "var(--color-ok-vivid)" : `url(#${gid})`));
</script>

<template>
  <div class="w-gauge-wrap">
    <div class="w-gauge" :style="{ width: `${size}px`, height: `${size}px` }">
      <svg :width="size" :height="size" viewBox="0 0 78 78">
        <defs v-if="variant === 'accent'">
          <linearGradient :id="gid" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stop-color="var(--w-accent)" />
            <stop offset="1" stop-color="var(--w-accent-2)" />
          </linearGradient>
        </defs>
        <circle cx="39" cy="39" :r="R" fill="none" stroke="var(--w-track)" stroke-width="6" />
        <circle
          cx="39"
          cy="39"
          :r="R"
          fill="none"
          :stroke="stroke"
          stroke-width="6"
          stroke-linecap="round"
          :stroke-dasharray="C"
          :stroke-dashoffset="dashOffset"
        />
      </svg>
      <div class="w-g-val">
        <b :style="{ fontSize: size >= 72 ? '15px' : '13px' }">{{ center }}</b>
        <span v-if="subText">{{ subText }}</span>
      </div>
    </div>
    <div v-if="$slots.default" class="w-gauge-meta"><slot /></div>
  </div>
</template>

<style scoped>
.w-gauge-wrap {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 14px;
  min-height: 0;
}
.w-gauge {
  position: relative;
  flex: none;
}
.w-gauge svg {
  transform: rotate(-90deg);
}
.w-g-val {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  text-align: center;
}
.w-g-val b {
  font-weight: 600;
  letter-spacing: -0.3px;
  line-height: 1.1;
}
.w-g-val span {
  font-size: 8.5px;
  color: var(--color-text-3);
  display: block;
}
.w-gauge-meta {
  font-size: 10.5px;
  color: var(--color-text-2);
  line-height: 1.7;
  min-width: 0;
}
.w-gauge-meta :deep(b) {
  color: var(--color-text);
  font-weight: 600;
}
</style>
