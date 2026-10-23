<script setup lang="ts">
/**
 * Sparkline —— 纯 CSS 迷你柱阵（FEAT-103 · P1）
 *
 * 用途：数据带每个读数下方一行趋势（46×16），或年组头的月度分布（组级更小）。
 *
 * 为什么自己画而不引图表库：
 *   ① 全项目零图表依赖（package.json 无 echarts/d3/chart.js），引入一个
 *      200KB+ 的库只为画 7~12 根宽度 4px 的柱子，与"极简"主张直接矛盾；
 *   ② 这里要的不是"图表"，而是**一个读数旁边的形状提示**——没有坐标轴、
 *      没有 tooltip、没有交互，图表库的能力全部用不上；
 *   ③ 纯 CSS 柱天然享受 prefers-reduced-motion 降级（无动画）。
 *
 * 归一化：高度按 [min, max] 线性映射到 [floorPct, 100%]。为什么不从 0 起：
 *   数值全在 90~100 之间时（如"相册数 12 / 持平稳态"），从 0 起会给出一排
 *   等高的平柱，看起来像"没有数据"；按 min 起能表达"有变化但幅度小"。
 *   平滑项 SPAN_FLOOR 用于避免 max==min 时除零。
 */
import { computed } from "vue";
import { toBarPercents } from "../../utils/sparkline.ts";

const props = withDefaults(
  defineProps<{
    /** 数值序列（从左到右） */
    data: number[];
    /** 柱宽（px） */
    barWidth?: number;
    /** 柱间距（px） */
    gap?: number;
    /** 容器高度（px） */
    height?: number;
    /** 高亮索引；-1 或越界则不强调 */
    highlight?: number;
    /** 极值归一化时是否允许从 min 起（false = 从 0 起） */
    fromMin?: boolean;
    /** 最低柱高（%），保证"接近 0"也能看见 */
    floorPct?: number;
  }>(),
  {
    barWidth: 4,
    gap: 2,
    height: 16,
    highlight: -1,
    fromMin: true,
    floorPct: 8,
  },
);

/** 高度百分比序列；空数据返回空数组（模板里 v-if 处理）
 *  归一化规则已抽到 utils/sparkline.ts（纯函数、有 11 项断言），
 *  这里只做入参转接 —— 组件不拥有这段"形状语言"的解释权。 */
const bars = computed<number[]>(() =>
  toBarPercents(props.data, { floorPct: props.floorPct, fromMin: props.fromMin }),
);

const boxStyle = computed(() => ({
  height: `${props.height}px`,
  gap: `${props.gap}px`,
}));

function barStyle(pct: number) {
  /* 只内联高度与宽度，颜色交给 CSS（内联颜色会盖住浅色模式的覆盖规则） */
  return {
    flex: "1 1 0",
    minWidth: `${props.barWidth}px`,
    height: `${pct}%`,
  };
}
</script>

<template>
  <!-- 装饰性元素：读屏用户已经能从数据带的数字得到信息，这里不应重复朗读 -->
  <div v-if="bars.length" class="u-spark" :style="boxStyle" aria-hidden="true">
    <i v-for="(pct, i) in bars" :key="i" :class="{ on: i === highlight }" :style="barStyle(pct)"></i>
  </div>
</template>

<style scoped>
.u-spark {
  display: flex;
  align-items: flex-end;
}
.u-spark i {
  display: block;
  border-radius: 1px;
  background: rgba(255, 255, 255, 0.16);
  transition: background var(--dur-2);
}
.u-spark i.on {
  background: var(--c-accent);
}

/* 浅色模式：16% 白在浅底上不可见，换成 16% 黑。
   用 body:not(.theme-dark) 而不是 @media prefers-color-scheme ——
   项目的深浅是用户偏好（body.theme-dark），不是系统设置。 */
body:not(.theme-dark) .u-spark i {
  background: rgba(23, 22, 26, 0.22);
}
body:not(.theme-dark) .u-spark i.on {
  background: var(--c-accent);
}
</style>
