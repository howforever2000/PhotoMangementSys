<script setup lang="ts">
/**
 * StatBand —— 统计数据带（FEAT-103 · P1）
 *
 * 替代原来的「统计卡片阵列」。核心区别不是样式，而是**信息结构**：
 *   卡片：标签 + 数字（占 96px 高、一张卡的横向面积）
 *   数据带：标签 + 数字 + sparkline（占同样的高度，横向压在一起，多出趋势）
 * 也就是说：同样的屏幕面积，数据带能多读一条"变化方向"。
 *
 * 为什么不用卡片容器：
 *   极简版面上，卡片边框的作用是"分组"。但当 5 张卡横向排列时，5 条边框
 *   反而切碎了本来连成一片的读数区——眼睛要在 5 个框之间跳。改用发丝竖线后，
 *   读数区是一条连续的带，只在需要停顿的地方有一道线。
 *
 * 分隔线用 ::before 而不是 border：border 会参与盒模型，让相邻格子的内容
 * 出现 1px 的不一致；绝对定位的伪元素不影响布局。
 */
import Sparkline from "./Sparkline.vue";

export interface StatItem {
  /** 标签（9.5px 大写灰） */
  label: string;
  /** 主读数（25px） */
  value: string | number;
  /** 读数后的单位（10.5px，弱化） */
  unit?: string;
  /** 变化值（如 "+28" / "↓31%"） */
  delta?: string;
  /** 变化值语气：up = 强调色 / mute = 弱化 / down = 危险色 */
  tone?: "up" | "mute" | "down";
  /** 趋势序列（缺省则不画 sparkline） */
  spark?: number[];
  /** 可选的第二行说明（出现在读数下方、sparkline 之上，10px 弱化） */
  note?: string;
  /**
   * FEAT-105：环形进度（0~1）。Rainmeter 皮肤里数据面板下挂的就是这种环。
   * 与 spark 的区别：spark 表示**趋势**（一段时间的序列），
   * 环表示**占比**（完成度）。两者不可互换。
   */
  ring?: number;
}

withDefaults(
  defineProps<{
    items: StatItem[];
    /** 带区高度（默认 86，与设计稿一致） */
    height?: number;
  }>(),
  { height: 86 },
);
</script>

<template>
  <div class="u-statband" :style="{ minHeight: `${height}px` }">
    <div v-for="(it, i) in items" :key="it.label + i" class="u-stat">
      <span class="u-stat-label">{{ it.label }}</span>
      <div class="u-stat-value">
        <span class="num">{{ it.value }}</span>
        <span v-if="it.unit" class="unit">{{ it.unit }}</span>
        <span v-if="it.delta" class="delta" :class="it.tone ?? 'up'">{{ it.delta }}</span>
      </div>
      <p v-if="it.note" class="u-stat-note">{{ it.note }}</p>
      <Sparkline v-if="it.spark?.length" :data="it.spark" :highlight="it.spark.length - 1" />

      <!-- FEAT-105：环形进度（Rainmeter 皮肤的数据面板标配）。
           纯 CSS conic-gradient，不用 SVG/图表库。 -->
      <span
        v-if="it.ring !== undefined"
        class="u-stat-ring"
        :style="{ '--ring-pct': `${Math.round(Math.min(1, Math.max(0, it.ring)) * 100)}%` }"
        aria-hidden="true"
      ></span>
    </div>

    <!-- 右侧内联区（月度节奏柱阵等）：由使用方塞 slot -->
    <div v-if="$slots.inline" class="u-stat-inline">
      <slot name="inline" />
    </div>
  </div>
</template>

<style scoped>
/* FEAT-105：Rainmeter 皮肤化 —— 每一格读数变成一块**独立玻璃小面板**。
   参考图里数据区的每一块都是一个小玻璃面板：大数字占主体，
   标签压在顶部左侧，下面挂一个微型图表，板与板之间留缝露出壁纸。

   为什么数据带是玻璃的最优先位置：
     它是整页唯一“每块都有数字”的区域，正好对应参考图的仪表盘；
     而且格子数少（5~6 块），backdrop-filter 的性能代价可接受。

   尺寸：数字 25px → 30px（面板给了更大的视觉预算），
   标签仍是 9.5px 但提到面板顶部（参考图就是压在顶上的）。 */
.u-statband {
  display: flex;
  align-items: stretch;
  gap: 10px;
  padding: 0 var(--sp-8);
}

.u-stat {
  position: relative;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 6px;
  min-width: 0;
  /* FEAT-105：面板给一个**紧凑但有底**的宽度。
     参考图的仪表块是"窄而高"（一个数 + 一个环），不是"宽而扁"。
     flex 用 0 1 auto：面板按内容宽度分配，但不超过 200px；
     剩下的空间用 flex 推到右侧（u-stat-inline 那一列），
     这样一排数据面板不会拉成一整条。 */
  flex: 0 1 auto;
  min-width: 128px;
  max-width: 200px;
  padding: 12px 14px 10px;
  border-radius: var(--panel-glass-radius);
  /* 玻璃板 */
  background-color: var(--panel-glass-fill);
  -webkit-backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  box-shadow: var(--panel-glass-shadow);
  border: 1px solid var(--panel-glass-edge);
}

/* 板顶镜面高光：与 .panel-glass 同一道光，但这里更窄（数据格小） */
.u-stat::before {
  content: "";
  position: absolute;
  inset: 0;
  border-radius: inherit;
  pointer-events: none;
  background: linear-gradient(
    135deg,
    rgba(255, 255, 255, 0.13) 0%,
    rgba(255, 255, 255, 0.035) 20%,
    rgba(255, 255, 255, 0) 42%
  );
}

/* 发丝分隔线删掉：参考图里格与格之间是**缝**（露出壁纸），不是线。 */

.u-stat-label {
  font-size: var(--fs-label);
  letter-spacing: var(--ls-label);
  text-transform: uppercase;
  /* label 是**要读**的（用户靠它判断这个数字是什么），所以用 fg-2 而不是 fg-3。
     初稿曾用 fg-3（设计稿的 26% 白），实测 2.09:1 —— 9.5px 的标签在这个对比度下
     基本看不见。只靠小字号 + 字距 + 大写建立层次，不靠低对比。 */
  color: var(--c-fg-2);
  font-weight: var(--fw-strong);
  white-space: nowrap;
}

.u-stat-value {
  display: flex;
  align-items: baseline;
  gap: 7px;
  line-height: 1;
}

.num {
  /* FEAT-105：25px → 30px。参考图里数据面板的��字是整块的视觉主体，
     25px 在玻璃面板上偏小，30px 才像"仪表读数"。 */
  font-size: 30px;
  font-weight: var(--fw-em);
  letter-spacing: var(--ls-metric);
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
  line-height: 1.05;
}

.unit {
  font-size: 10.5px;
  font-weight: 500;
  /* FEAT-105 实测：--c-fg-3 在浅色预设三段渐变上只有 3.16:1。
     单位的含义虽已由前面的数字 + 标签给出，但 10.5px + 3.16:1 在浅底上
     已接近"看不见"，而它占的横向空间几乎为零 —— 提到 fg-2 不损失密度。 */
  color: var(--c-fg-2);
}

.delta {
  font-size: 10px;
  font-weight: var(--fw-strong);
  letter-spacing: 0;
}
.delta.up {
  /* FEAT-105：玻璃板把底提亮了，金色在亮底上会变淡（3.97:1 → 2.28:1）。
     正确做法是**反向**处理：深色底上提亮金色，浅色底上压暗金色。
     用 color-mix 跟着预设走（不改色相，只改明度）。 */
  color: var(--c-accent-bright, var(--c-accent));
}
.delta.mute {
  color: var(--c-fg-2);
}
.delta.down {
  color: var(--c-danger);
}

.u-stat-note {
  margin: 0;
  font-size: 10px;
  color: var(--c-fg-2);
  letter-spacing: var(--ls-meta);
}

/* 环形进度：22px 的小圆环，贴在面板右下角。
   conic-gradient 直接画 —— 不引 SVG 也不用图表库（全项目零图表依赖）。
   环内留空（只画弧），中心透出玻璃底，与参考图的空心环一致。 */
.u-stat-ring {
  position: absolute;
  right: 11px;
  bottom: 11px;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: conic-gradient(
    var(--c-accent) 0 var(--ring-pct, 0%),
    color-mix(in srgb, var(--c-fg-3) 26%, transparent) var(--ring-pct, 0%) 100%
  );
  /* 挖空成环 */
  -webkit-mask: radial-gradient(circle, transparent 0 5px, #000 5.5px);
  mask: radial-gradient(circle, transparent 0 5px, #000 5.5px);
  pointer-events: none;
}

/* 面板右侧给环留位：数字 + delta 不能被环压住 */
.u-stat-value {
  padding-right: 24px;
}

.u-stat-inline {
  margin-left: auto;
  display: flex;
  align-items: flex-end;
  padding-bottom: 2px;
}

/* 窄窗：数据带允许横向滚动而不是换行挤压。
   为什么不换行：换行后分隔竖线会落在"行首"，读起来像新分组，语义错。
   也不隐藏：统计信息是这一页的核心内容，隐藏等于删除。 */
@media (max-width: 900px) {
  .u-statband {
    overflow-x: auto;
    padding: 0 var(--sp-5);
    scrollbar-width: none;
  }
  .u-statband::-webkit-scrollbar {
    display: none;
  }
  .u-stat {
    flex: 0 0 auto;
    min-width: 118px;
  }
}
</style>
