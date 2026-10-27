<script setup lang="ts">
/**
 * RankBarsCore —— 横向排行条芯（FEAT-110 / FEAT-120 加 dense）
 * 原子职责：label + 一根按 ratio（0~1）铺宽的强调渐变条 + 数值文本，若干行。
 * 用于「最大聚集（Top 城市）」这类榜，不自己排序（顺序与归一化由父级算好）。
 *
 * FEAT-120 为什么加 dense：概览「地点」块拆成两栏后，这一栏只有 ~192px。
 * 常规档的 44px 标签列 + 9px 间距会把条挤到 90px 出头，五行排在一起读不出差距；
 * dense 把标签列收到 40px、行距收到 7px、条高 5→4px，条才恢复成"可比较的长度"。
 * 仍然只有一个 atom —— 不为"窄一点"再复制一个组件。
 */
withDefaults(
  defineProps<{
    items: { label: string; value: string | number; ratio: number }[];
    /** 窄栏紧凑档：标签列 40px、行距 9px、条高 4px（概览两栏布局用） */
    dense?: boolean;
    /**
     * 条的色：accent = 强调渐变（默认，单块成榜时用）；
     * muted = 弱色实心。排行榜里放了 5 根强调条，同屏就会出现 5 个"金色"，
     * 而 tokens 的色彩预算是"强调色只标一个答案" —— 长度已经把名次说完了，
     * 条本身不需要再喊一遍。
     */
    tone?: "accent" | "muted";
  }>(),
  { dense: false, tone: "accent" },
);
</script>

<template>
  <div class="w-rank" :class="{ dense, muted: tone === 'muted' }">
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
.w-rank-row b {
  display: block;
  height: 100%;
  border-radius: 3px;
  background: linear-gradient(90deg, var(--w-accent), var(--w-accent-2));
}
.w-rank.muted .w-rank-row b {
  background: var(--w-fg2);
  /* 0.55 而不是更实：浅色主题下 --w-fg2 是接近墨色的深灰，
     实心时排行榜会成为整块板里最重的元素，把强调色那一栏（年份峰值）压掉。
     弱化到 0.55 后"长度"依然是主信息，"色"退回配角。 */
  opacity: 0.55;
}
.w-rank-row em {
  font-style: normal;
  color: var(--color-text-3);
  font-variant-numeric: tabular-nums;
}

/* 窄栏紧凑档 */
.w-rank.dense {
  gap: 9px;
  margin-top: 0;
}
.w-rank.dense .w-rank-row {
  grid-template-columns: 40px 1fr auto;
  gap: 7px;
  font-size: 9.5px;
}
.w-rank.dense .w-rank-row i {
  height: 4px;
}
</style>
