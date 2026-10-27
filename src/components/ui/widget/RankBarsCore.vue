<script setup lang="ts">
/**
 * RankBarsCore —— 横向排行条芯（FEAT-110）
 * 原子职责：label + 一根按 ratio（0~1）铺宽的强调渐变条 + 数值文本，若干行。
 * 用于「最大聚集（Top 城市）」这类榜，不自己排序（顺序与归一化由父级算好）。
 */
defineProps<{
  items: { label: string; value: string | number; ratio: number }[];
}>();
</script>

<template>
  <div class="w-rank">
    <div v-for="(r, i) in items" :key="i" class="w-rank-row">
      <span>{{ r.label }}</span>
      <i><b :style="{ width: `${Math.min(100, Math.max(0, r.ratio * 100))}%` }"></b></i>
      <em>{{ r.value }}</em>
    </div>
  </div>
</template>

<style scoped>
.w-rank {
  display: flex;
  flex-direction: column;
  gap: 9px;
  margin-top: 12px;
}
.w-rank-row {
  display: grid;
  grid-template-columns: 44px 1fr auto;
  align-items: center;
  gap: 9px;
  font-size: 10px;
}
.w-rank-row span {
  color: var(--color-text-2);
  white-space: nowrap;
}
.w-rank-row i {
  height: 5px;
  border-radius: 3px;
  background: var(--w-track);
  overflow: hidden;
}
/* FEAT-115：默认用弱色条，只有**第一名**才用强调色。
   改前 6 行全部金色 —— 一屏 6 处强调等于没有强调，
   而且读者看不出"谁第一"（长度已经表达了比例，颜色该只标记冠军）。 */
.w-rank-row b {
  display: block;
  height: 100%;
  border-radius: 3px;
  background: var(--color-text-3);
}
.w-rank-row:first-child b {
  background: linear-gradient(90deg, var(--w-accent), var(--w-accent-2));
}
.w-rank-row em {
  font-style: normal;
  color: var(--color-text-3);
  font-variant-numeric: tabular-nums;
}
</style>
