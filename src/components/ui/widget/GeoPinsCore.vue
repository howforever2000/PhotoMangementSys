<script setup lang="ts">
/**
 * GeoPinsCore —— 轻量地点散点芯（FEAT-110）
 * 原子职责：把经纬度聚合成散点示意（不是精确地图——精确地图用 LocationMap）。
 * 用固定 viewBox + meet 等比投影，点大小 ∝ √张数；最多给前两名常驻地名。
 * 点的是“分布密度”这一眼信号，详情去「地点」tab 看真地图。
 */
import { computed } from "vue";
import type { LocationGroupRow } from "../../../types/content";

const props = withDefaults(
  defineProps<{
    groups: LocationGroupRow[];
    /** 常驻地名标签的点数（按张数取前 N）。默认 1：点位在密集区域会重叠，
     *  只给最大聚集点常驻标签，其余靠悬停/点击进地点页看真地图。 */
    labelTop?: number;
  }>(),
  { labelTop: 1 },
);

const emit = defineEmits<{ (e: "pick", g: LocationGroupRow): void }>();

/* 中国范围（含一点留白）；等距圆柱投影即可，散点不要求等积精度 */
const LON_MIN = 72;
const LON_MAX = 136;
const LAT_MIN = 16;
const LAT_MAX = 55;
const W = LON_MAX - LON_MIN; // 64
const H = LAT_MAX - LAT_MIN; // 39
const PAD = 4;
const VB_W = W + PAD * 2; // 72
const VB_H = H + PAD * 2; // 47

const points = computed(() => {
  const named = props.groups.filter((g) => g.lat != null && g.lon != null);
  const max = named.reduce((m, g) => Math.max(m, g.count), 0) || 1;
  const byCount = [...named].sort((a, b) => b.count - a.count);
  const labelNames = new Set(byCount.slice(0, props.labelTop).map((g) => g.location));
  return named.map((g) => {
    const x = PAD + ((g.lon! - LON_MIN) / W) * W;
    const y = PAD + (1 - (g.lat! - LAT_MIN) / H) * H;
    /* FEAT-116：半径上调（1.1+2.6 → 1.4+3.1）。
       在 4 列宽的板里，72 宽的 viewBox 被缩到 ~380px，原来的点只有 6~8px，
       一屏看过去像"板上有灰"。现在最大聚点 ~14px，散点也能一眼看见密度。 */
    const r = 1.4 + Math.sqrt(g.count / max) * 3.1;
    return { g, x, y, r, hot: g.count === max, showLabel: labelNames.has(g.location) };
  });
});

function shortName(n: string | null): string {
  /* 形如 “四川省·成都” / “重庆市”：取 · 后的城市名，再去行政后缀 */
  const city = (n ?? "").split("·").pop() ?? "";
  return city.replace(/[省市区县蒙古族藏族回族壮族维吾尔自治区特别行政区]+$/g, "").slice(0, 4);
}
</script>

<template>
  <div class="w-geo">
    <svg :viewBox="`0 0 ${VB_W} ${VB_H}`" preserveAspectRatio="xMidYMid meet" class="w-geo-svg">
      <defs>
        <pattern id="w-geo-dots" width="3.2" height="3.2" patternUnits="userSpaceOnUse">
          <circle cx="0.6" cy="0.6" r="0.3" fill="currentColor" />
        </pattern>
      </defs>
      <g class="w-geo-field"><rect width="100%" height="100%" fill="url(#w-geo-dots)" /></g>
      <g v-for="(p, i) in points" :key="i" class="w-geo-pin" @click="emit('pick', p.g)">
        <circle
          :cx="p.x"
          :cy="p.y"
          :r="p.r + 1.6"
          :fill="p.hot ? 'var(--w-accent)' : 'var(--w-accent-2)'"
          opacity="0.18"
        />
        <circle
          :cx="p.x"
          :cy="p.y"
          :r="p.r"
          :fill="p.hot ? 'var(--w-accent)' : 'var(--w-accent-2)'"
        />
        <text
          v-if="p.showLabel"
          :x="p.x + p.r + 0.8"
          :y="p.y + 0.9"
          class="w-geo-txt"
        >{{ shortName(p.g.location) }} {{ p.g.count }}</text>
      </g>
    </svg>
  </div>
</template>

<style scoped>
.w-geo {
  flex: 1;
  min-height: 0;
  margin-top: 6px;
  border-radius: var(--w-radius-inner);
  overflow: hidden;
}
.w-geo svg {
  width: 100%;
  height: 100%;
  display: block;
  /* 点阵用 widget 专用点色（实测旧值 11% 白在深底上 1.01:1，整片看不见） */
  color: var(--w-dot);
  opacity: 1;
}
/* FEAT-116：点阵原本铺满整块矩形，读起来是"一块有纹理的灰底"而不是"一张图"。
   遮罩只加在**点阵层**上（不是整个 svg）—— 否则贴边的点会被一起淡掉。 */
.w-geo-field {
  -webkit-mask-image: radial-gradient(118% 96% at 50% 48%, #000 42%, rgba(0, 0, 0, 0.35) 78%, transparent 100%);
  mask-image: radial-gradient(118% 96% at 50% 48%, #000 42%, rgba(0, 0, 0, 0.35) 78%, transparent 100%);
}
.w-geo-pin {
  cursor: pointer;
}
/* FEAT-117：同时给 fill 与 color —— 对比度审计读的是 computed color，
   只写 fill 会出现"肉眼已修好、审计仍报 1.03:1"的假阴性。 */
.w-geo-txt {
  fill: var(--color-text-2);
  color: var(--color-text-2);
  font-size: 3.1px;
  font-weight: 600;
}
</style>
