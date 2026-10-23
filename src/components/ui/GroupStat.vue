<script setup lang="ts">
/**
 * GroupStat —— 组级统计（FEAT-103 · P1）
 *
 * 用在「年分组头」「分类分组头」这类位置：与组标题同基线，字号降一档。
 *
 * 与 StatBand 的分工（这是本组件存在的理由）：
 *   StatBand   = 页面级，4~6 个读数，横向铺开，带 sparkline
 *   GroupStat  = 组级，2~3 个短读数，内联在标题行，右侧可带迷你分布
 * 两者的差别不是"大小号"，而是**信息密度与位置的差别**：
 * 页面级要能扫，组级要能贴着标题读，不打断阅读流。
 */
import Sparkline from "./Sparkline.vue";

export interface GroupStatItem {
  /** 数值（如 "388"） */
  value: string | number;
  /** 单位/说明（如 "张" / "个月"） */
  label?: string;
}

withDefaults(
  defineProps<{
    items: GroupStatItem[];
    /** 右侧迷你分布（月度/季度分布等） */
    dist?: number[];
    /** 迷你分布的高亮索引 */
    distHighlight?: number;
  }>(),
  { distHighlight: -1 },
);
</script>

<template>
  <div class="u-groupstat">
    <template v-for="(it, i) in items" :key="i">
      <span v-if="i > 0" class="sep">·</span>
      <span class="pair">
        <b>{{ it.value }}</b>
        <span v-if="it.label" class="lb">{{ it.label }}</span>
      </span>
    </template>
    <Sparkline
      v-if="dist?.length"
      class="dist"
      :data="dist"
      :bar-width="5"
      :height="13"
      :highlight="distHighlight"
    />
  </div>
</template>

<style scoped>
.u-groupstat {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  font-size: var(--fs-meta);
  letter-spacing: var(--ls-meta);
  /* 组级统计整行用 fg-2：它跟标题同行，读者会顺带看，
     不能因为"比标题次要"就降到 2:1 对比度 —— 那等于不显示。 */
  color: var(--c-fg-2);
  opacity: 0.82; /* 比标题淡一档的层次，但不拿对比度去换 */
  font-variant-numeric: tabular-nums;
}

.pair {
  display: inline-flex;
  align-items: baseline;
  gap: 4px;
}
.pair b {
  font-weight: var(--fw-em);
  color: var(--c-fg);
}
.lb {
  font-weight: 400;
}

.sep {
  color: var(--c-fg-2);
  opacity: 0.55;
}

.dist {
  margin-left: 8px;
  /* 迷你分布要与文字基线对齐，而不是与行盒中心对齐 */
  align-self: flex-end;
  margin-bottom: 1px;
}
</style>
