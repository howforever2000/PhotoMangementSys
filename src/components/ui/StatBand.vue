<script setup lang="ts">
/**
 * StatBand —— 仪表数据带（FEAT-106）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 它是什么
 * ──────────────────────────────────────────────────────────────────────────
 * 页面顶部的一排**仪表小面板**：每块面板里有
 *   · 顶部一行微标签（9.5px 大写）
 *   · 一个大读数（30px）+ 可选单位 / 变化值
 *   · 底部一条**进度条**（不是环）—— 表达"占比 / 完成度"
 * 面板与面板之间留缝，缝里露出壁纸 —— 这是参考图里仪表盘区的读感。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么改掉了环形仪表（FEAT-105 曾经是环）
 * ──────────────────────────────────────────────────────────────────────────
 * 两个理由，第一个是布局，第二个是语义：
 *
 * ① 布局：环是**绝对定位**在面板右下角的，宽度固定 20px。
 *    当面板里的 delta 文本（如"需要扫描"）换行到第二行、
 *    或者面板被压窄到 128px 以下时，读数就会压到环上 ——
 *    这就是用户截图里"数字和环叠在一起"的成因。
 *    改成**底部进度条**后，仪表有了自己的行（flex 布局里的一块），
 *    不可能再和文字重叠：宁可把面板加高，也不允许叠字。
 *
 * ② 语义：环读的是"完成度"，和 sparkline 读"趋势"本来就是两件事。
 *    但在这个数据带里，所有读数都是**静态快照**（此刻有多少），
 *    没有时间维度 —— 用进度条表达"这一项占总量的多少"更诚实，
 *    而且进度条比环更容易读数（长度 vs 角度）。
 */
export interface StatItem {
  /** 标签（9.5px 大写灰） */
  label: string;
  /** 主读数（30px） */
  value: string | number;
  /** 读数后的单位（10.5px，弱化） */
  unit?: string;
  /** 变化值（如 "+28" / "↓31%"） */
  delta?: string;
  /** 变化值语气：up = 强调色 / mute = 弱化 / down = 危险色 */
  tone?: "up" | "mute" | "down";
  /** 趋势序列（缺省则不画 sparkline） */
  spark?: number[];
  /** 可选的第二行说明（出现在读数下方、进度条之上，10px 弱化） */
  note?: string;
  /**
   * 进度（0~1）。缺省则不画进度条。
   * 与 spark 的区别：spark 表示**趋势**（一段时间的序列），
   * 进度条表示**占比**（完成度）。两者不可互换。
   */
  progress?: number;
  /** 进度条颜色语义：accent = 强调色 / muted = 弱化（默认） */
  progressTone?: "accent" | "muted";
  /**
   * FEAT-106：面板底部的迷你柱阵。
   *
   * 为什么需要它：数据带的面板高 96px，而“标签 + 数字 + 进度条”只占 ~70px，
   * 剩下 26px 是空的 —— 面板读起来像一个装着大数字的空盒子。
   * 参考图里每块仪表面板的底部都有一条微型数据带（柱阵 / 环），
   * 那不是装饰，它填满了面板并给出“这一项有波动”的暗示。
   *
   * 数据来源由使用方提供（各页自己算），组件只负责画形状。
   */
  bars?: number[];
}

withDefaults(
  defineProps<{
    items: StatItem[];
    /** 带区高度（默认 96，与设计稿一致） */
    height?: number;
  }>(),
  { height: 96 },
);
</script>

<template>
  <div class="u-statband" :style="{ minHeight: `${height}px` }">
    <div v-for="(it, i) in items" :key="it.label + i" class="u-stat">
      <span class="u-stat-label">{{ it.label }}</span>

      <!-- 读数行：数字 + 单位 + 变化值。
           flex-wrap 允许变化值换行到下一行，而不是被压缩成竖排单字；
           因为下方有进度条独立占行，读数换行也不会撞到任何东西。 -->
      <div class="u-stat-value">
        <span class="num">{{ it.value }}</span>
        <span v-if="it.unit" class="unit">{{ it.unit }}</span>
        <span v-if="it.delta" class="delta" :class="it.tone ?? 'up'">{{ it.delta }}</span>
      </div>

      <p v-if="it.note" class="u-stat-note">{{ it.note }}</p>

      <!-- 微型柱阵：占据面板底部，把剩下的竖向空间填满。
           与 sparkline 的区别：sparkline 是独立一行的趋势图；
           这里的柱阵是面板的**底饰**（短、高、密），读作仪表的一部分。 -->
      <span v-if="it.bars?.length" class="u-stat-bars" aria-hidden="true">
        <i
          v-for="(h, bi) in it.bars"
          :key="bi"
          :style="{ height: `${Math.max(6, Math.min(100, h))}%` }"
          :class="{ on: bi === it.bars.length - 1 }"
        ></i>
      </span>

      <!-- 进度条：占据面板底部一整行（不是绝对定位），因此永不与文字重叠。
           轨道用 --c-gauge-track（纯图形色），填充用强调色。 -->
      <span
        v-if="it.progress !== undefined"
        class="u-stat-gauge"
        :class="it.progressTone ?? 'accent'"
        :style="{ '--pct': `${Math.round(Math.min(1, Math.max(0, it.progress)) * 100)}%` }"
        role="img"
        :aria-label="`${it.label} 完成度 ${Math.round(Math.min(1, Math.max(0, it.progress)) * 100)}%`"
      >
        <i></i>
      </span>
    </div>

    <!-- 右侧内联区（月度节奏柱阵等）：由使用方塞 slot -->
    <div v-if="$slots.inline" class="u-stat-inline">
      <slot name="inline" />
    </div>
  </div>
</template>

<style scoped>
/* ══════════════════════════════════════════════════════════════════════════
   数据带 = 一排独立的磨砂玻璃小面板
   ──────────────────────────────────────────────────────────────────────────
   与参考图的对应关系：
     · 参考图里仪表区的每一格都是一块小面板，格与格之间是**缝**（露出壁纸）；
     · 面板顶部压着微标签，下方是大数字，最底下挂一个微型仪表；
     · 数字是整块面板的视觉主体（30px），标签只做定位。
   为什么是玻璃而不是纸片：数据带是每页**唯一**"每块都有数字"的区域，
   正好对应参考图的仪表盘区；格子数少（5~6 块），backdrop-filter 的代价可接受。
   ══════════════════════════════════════════════════════════════════════════ */
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
  gap: 7px;
  min-width: 0;
  /* 面板按**等分**铺满整行（参考图里仪表区的那排面板就是占满宽度的）。
     旧版是 flex: 0 1 auto + max-width 210 —— 5 块面板只占到 1050px，
     右侧留下一条空白，读起来像"表格只写了一半"。
     现在 flex: 1 1 0 让它们平分可用宽度，max-width 只防极端宽屏下单独一块被拉得太长。 */
  flex: 1 1 0;
  min-width: 132px;
  max-width: 320px;
  padding: 13px 15px 14px;
  border-radius: var(--panel-glass-radius);
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

/* 微标签：压在面板顶部，是"这个数字是什么"的唯一标识 —— 要读，所以用 fg-2 */
.u-stat-label {
  font-size: var(--fs-label);
  letter-spacing: var(--ls-label);
  text-transform: uppercase;
  color: var(--c-fg-2);
  font-weight: var(--fw-strong);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.u-stat-value {
  display: flex;
  align-items: baseline;
  gap: 7px;
  line-height: 1;
  /* 变化值过长时换行到下一行，而不是把数字挤扁 */
  flex-wrap: wrap;
}

.num {
  /* 30px：参考图里数据面板的数字是整块的视觉主体，
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
  /* 单位的含义虽已由前面的数字 + 标签给出，但 10.5px 的弱化文字
     在浅底上接近"看不见"，而它占的横向空间几乎为零 —— 提到 fg-2 不损失密度。 */
  color: var(--c-fg-2);
}

.delta {
  font-size: 10px;
  font-weight: var(--fw-strong);
  letter-spacing: 0;
}
.delta.up {
  /* 玻璃板把底提亮了，金色在亮底上会变淡。
     用 --c-accent-bright（深底提亮 / 浅底压暗，方向相反）。 */
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

/* ── 微型柱阵（面板底饰）──────────────────────────────────────────────
   短、高、密：读作仪表的一部分，而不是一条独立趋势图。
   最后一根用强调色（“最新”），其余用弱化色 —— 与全站的 sparkline 语言一致。 */
.u-stat-bars {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 20px;
  margin-top: auto;
}
.u-stat-bars i {
  display: block;
  flex: 1 1 0;
  min-width: 2px;
  border-radius: 1px 1px 0 0;
  background: var(--c-gauge-track);
}
.u-stat-bars i.on {
  background: var(--c-accent);
}

/* ── 进度条（FEAT-106 取代环形仪表）────────────────────────────────────
   独占面板底部一整行，因此在流里不可能与读数重叠 ——
   这正是用户截图里"数字压在环上"的那个问题的根治办法：
   不是靠调 z-index 或加间距，而是**根本不让它们共处一行**。 */
.u-stat-gauge {
  display: block;
  height: 3px;
  margin-top: 6px;
  border-radius: 2px;
  background: var(--c-gauge-track);
  overflow: hidden;
}
.u-stat-gauge i {
  display: block;
  height: 100%;
  width: var(--pct, 0%);
  border-radius: 2px;
  background: var(--c-accent);
  transition: width var(--dur-3) var(--ease-out);
}
.u-stat-gauge.muted i {
  /* 弱化进度：用前景色的低透明档而不是 --c-fg-2。
     --c-fg-2 是**文字**色（浅色下是深灰 0.66），直接画 3px 粗线在浅底上
     会显得像一条黑线；color-mix 到 42% 才是"仪表轨道"该有的强度。 */
  background: color-mix(in srgb, var(--c-fg) 42%, transparent);
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
    min-width: 122px;
  }
}
</style>
