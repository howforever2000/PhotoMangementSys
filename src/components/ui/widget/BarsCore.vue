<script setup lang="ts">
/**
 * BarsCore —— 迷你柱阵芯（FEAT-110）
 * 原子职责：给一组归一化高度（0~1）画等宽柱；可标记某根为高亮（当月等）。
 * 不内置坐标轴/弹窗——仪表卡里的柱阵只表达“形状与当下”，细节在点开的详情页。
 */
withDefaults(
  defineProps<{
    /** 每根柱的高度比例 0~1 */
    values: number[];
    /** 高亮柱的下标集合（强调渐变 + 微光） */
    activeIndexes?: number[];
    /** 每根柱下方小字（如月份缩写），可省略 */
    labels?: string[];
  }>(),
  { activeIndexes: () => [], labels: () => [] },
);
</script>

<template>
  <div class="w-bars">
    <i
      v-for="(v, idx) in values"
      :key="idx"
      :class="{ on: activeIndexes.includes(idx), 'is-zero': v <= 0 }"
      :style="{ height: `${v > 0 ? Math.min(100, Math.max(3, v * 100)) : 0}%` }"
    >
      <small v-if="labels[idx]">{{ labels[idx] }}</small>
    </i>
  </div>
</template>

<style scoped>
.w-bars {
  flex: 1;
  display: flex;
  align-items: flex-end;
  gap: 5px;
  min-height: 0;
}
.w-bars i {
  flex: 1;
  min-height: 0;
  border-radius: 2.5px 2.5px 1px 1px;
  background: var(--w-track);
  position: relative;
}
/* 值为 0（如本年度还没到的未来月份）不画柱，只保留占位，不出现“最小值假柱” */
.w-bars i.is-zero {
  background: transparent;
}
.w-bars i.on {
  background: linear-gradient(180deg, var(--w-accent), var(--w-accent-2));
  box-shadow: 0 0 10px -2px var(--w-accent);
}
.w-bars small {
  position: absolute;
  left: 50%;
  bottom: -14px;
  transform: translateX(-50%);
  font-size: 8px;
  color: var(--color-text-3);
  /* <small> 在 <i> 里，而 <i> 默认斜体 —— 年份是数字，斜体看着像手写体、
     tabular-nums 也失效（数字宽度不再等高）。必须显式复位。 */
  font-style: normal;
  font-variant-numeric: tabular-nums;
}
</style>
