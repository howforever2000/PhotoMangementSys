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
    </div>

    <!-- 右侧内联区（月度节奏柱阵等）：由使用方塞 slot -->
    <div v-if="$slots.inline" class="u-stat-inline">
      <slot name="inline" />
    </div>
  </div>
</template>

<style scoped>
.u-statband {
  display: flex;
  align-items: stretch;
  padding: 0 var(--sp-8);
}

.u-stat {
  position: relative;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 7px;
  padding-right: var(--sp-5);
  margin-right: var(--sp-5);
}

/* 发丝分隔：绝对定位，不参与布局，不影响各格内容的对齐 */
.u-stat + .u-stat::before {
  content: "";
  position: absolute;
  left: calc(var(--sp-5) * -1);
  top: 50%;
  transform: translateY(-50%);
  width: 1px;
  height: 38px;
  background: var(--c-rule);
}

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
  font-size: var(--fs-metric);
  font-weight: var(--fw-em);
  letter-spacing: var(--ls-metric);
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
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
  color: var(--c-accent);
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
  }
}
</style>
