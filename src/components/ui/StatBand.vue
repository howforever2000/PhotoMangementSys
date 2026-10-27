<script setup lang="ts">
/**
 * StatBand —— 数据带（FEAT-107 重写：从「一排玻璃板」改为「文本列」）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么又改了一次（FEAT-103 建、FEAT-106 改、本轮再改）
 * ──────────────────────────────────────────────────────────────────────────
 * FEAT-106 把它做成一排独立的磨砂玻璃小面板，对应参考图的仪表盘区。
 * 在真实数据下暴露两个硬问题：
 *
 * ① **折行是算术必然，不是意外。**
 *    `.u-stat { flex: 1 1 0; min-width: 132px }` —— 5 块刚好（132×5+8×4=692），
 *    但地点页有 9 块：132×9 + 10×8 = 1268px > 内容宽。
 *    flex 只能换行；换行后每块被 `flex: 1` 拉到**更大的宽度**，
 *    于是两行的板尺寸不一致 —— 眼睛读成"表格第二行只剩了一格"。
 *    这不是调 min-width 能解决的：只要项数随数据变化，就没有安全值。
 *
 * ② **金色预算被一次用光。**
 *    tokens.css §2.7 明明写着"强调色只允许出现在当前导航项 / 变化值 / 图表高亮
 *    三处"，但每块板都挂了一条金色进度条 —— 一屏 9 处强调 = 没有强调。
 *
 * 现在的做法：**容器归零**。
 *   一列 = 标签（9.5px 大写弱色）+ 读数（30px 细字重）+ 注释（10.5px 弱色），
 *   列与列之间一根 12px 高的发丝竖线，**没有底板、没有亮边、没有投影**。
 *   宽度按信息量加权（w-1 / w-2），所以任意项数都恒为一行。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么读数用字重 340 而不是 560
 * ──────────────────────────────────────────────────────────────────────────
 * 大字号 + 粗字重 = PPT 图表（每一块都在喊"看我"）；
 * 大字号 + 细字重 = 排印（读数是被"摆"在那里的，不需要喊）。
 * 一屏里有 5 个读数，全部加粗就等于全部没有重点。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 与旧版 props 的关系：**完全兼容**，一个字段都没删
 * ──────────────────────────────────────────────────────────────────────────
 * 6 个消费页（AlbumList / Memories / ScanHub / SmartAlbum / Timeline /
 * LocationGallery）不用改一行。`bars` 与 `progress` 都保留：
 *   · progress → 底部 2px 占比线（默认弱色，`progressTone:'accent'` 才用金色）
 *   · bars     → 底部微型柱阵（只建议给"分布"真正有意义的那一列）
 * 旧的 `delta` 不再跟读数挤同一行，而是移到标签行右侧（见下方 .u-stat-aux）：
 *   delta 是"标签的解释"，不是"数字的同伴"。
 */
export interface StatItem {
  /** 标签（9.5px 大写弱色） */
  label: string;
  /** 主读数（30px 细字重） */
  value: string | number;
  /** 读数后的单位（11px 弱色） */
  unit?: string;
  /** 变化值 / 补充说明（如 "+28" / "128 / 129"），落在**标签行右侧** */
  delta?: string;
  /**
   * 变化值语气：`up` = 强调色 / `mute` = 弱化 / `down` = 危险色。
   *
   * ⚠️ 缺省是 `mute`，**不是 `up`**（FEAT-115 修）。
   * 旧版缺省是 "up"，于是所有"没写 tone"的列（也就是绝大多数静态计数）
   * 都被染成 --c-accent-bright 的金色 —— 一屏 3 处金，强调色预算（tokens.css
   * §2.7 只允许"当前项 / 变化值 / 图表高亮"三处）直接爆掉。
   * 金色是"这数在变"的信号，静态计数不该用它。
   */
  tone?: "up" | "mute" | "down";
  /** 趋势序列（缺省则不画 sparkline） */
  spark?: number[];
  /** 可选的说明行（出现在读数下方，10.5px 弱色） */
  note?: string;
  /**
   * 进度（0~1）。缺省则不画占比线。
   * 与 spark 的区别：spark 表示**趋势**（一段时间的序列），
   * 占比线表示**占比**（完成度）。两者不可互换。
   */
  progress?: number;
  /**
   * 占比线颜色语义：`muted`（默认，弱色）/ `accent`（强调色）。
   * ⚠️ 一屏只应有一处 `accent` —— 这是金色预算的落地点。
   */
  progressTone?: "accent" | "muted";
  /** 面板底部的迷你柱阵（建议只给"分布"真正有意义的那一列） */
  bars?: number[];
}

const props = withDefaults(
  defineProps<{
    items: StatItem[];
    /**
     * 带区最小高度（默认 96）。保留是为了兼容旧调用方；
     * 新版只把它当作 `min-height` —— 内容更多时由内容撑开，不裁剪。
     */
    height?: number;
  }>(),
  { height: 96 },
);

/**
 * 一列占多少横向预算。
 *
 * 为什么不是等分：读数长度天然不等（"129" vs "25078"），
 * 等分会让短读数两边空出一大片、长读数几乎贴线。
 * 这里按「数字位数 + 是否有注释」给一个粗档：
 *   · 有 bars / 有较长 note / 数字 ≥5 位 → 1.5 档
 *   · 其余 → 1 档
 * 刻意只做两档：三档以上就开始像"按内容排版"，反而不整齐。
 */
function widthClass(it: StatItem): string {
  const len = String(it.value).length;
  const heavy = (it.bars?.length ?? 0) > 0 || len >= 5;
  return heavy ? "w-2" : "w-1";
}

const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
</script>

<template>
  <div class="u-statband" :style="{ minHeight: `${props.height}px` }">
    <div
      v-for="(it, i) in items"
      :key="it.label + i"
      class="u-stat"
      :class="widthClass(it)"
    >
      <!-- 标签行：标签 + delta（delta 靠右，是标签的解释而不是数字的同伴） -->
      <div class="u-stat-head">
        <span class="u-stat-label">{{ it.label }}</span>
        <span
          v-if="it.delta"
          class="u-stat-aux"
          :class="[it.tone ?? 'mute', { dim: !it.tone || it.tone === 'mute' }]"
        >{{ it.delta }}</span>
      </div>

      <!-- 读数行：只有数字 + 单位，不再塞 delta -->
      <div class="u-stat-value">
        <span class="num">{{ it.value }}</span>
        <span v-if="it.unit" class="unit">{{ it.unit }}</span>
      </div>

      <p v-if="it.note" class="u-stat-note">{{ it.note }}</p>

      <!-- 微型柱阵：只占据「没有占比线也没有注释」时的底部空间 -->
      <span v-if="it.bars?.length" class="u-stat-bars" aria-hidden="true">
        <i
          v-for="(h, bi) in it.bars"
          :key="bi"
          :style="{ height: `${Math.max(6, Math.min(100, h))}%` }"
          :class="{ on: bi === it.bars.length - 1 }"
        ></i>
      </span>

      <!-- 占比线：2px，长度即真实比例。默认弱色 —— 金色留给 progressTone:'accent' -->
      <span
        v-if="it.progress !== undefined"
        class="u-stat-line"
        :class="it.progressTone === 'accent' ? 'hot' : ''"
        :style="{ '--pct': `${Math.round(clamp01(it.progress) * 100)}%` }"
        role="img"
        :aria-label="`${it.label} 占比 ${Math.round(clamp01(it.progress) * 100)}%`"
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
   数据带 = 一排文本列（无容器）
   ──────────────────────────────────────────────────────────────────────────
   读成一件事的顺序：**标签（这列是什么）→ 读数（多少）→ 注释（补充）**。
   三行同基线，横向一眼扫完。

   为什么发丝线只画在「列与列之间」而不是「每列都有」：
   每列都画框就是表格；只有间隙才是"分组"。第一列左侧刻意不画线 ——
   它贴着内容左边缘，画了会多出一条与页面留白平行的线。
   ══════════════════════════════════════════════════════════════════════════ */
.u-statband {
  display: flex;
  align-items: stretch;
  /* 左右留白与页面同一套（--gutter）。旧版用 --sp-8(52px)，
     比页面其他元素宽 4px，长屏下能看出一条错位。 */
  padding: 18px var(--gutter) 16px;
  border-bottom: 1px solid var(--rule);
}

.u-stat {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}
.u-stat.w-1 { flex: 1 1 0; }
.u-stat.w-2 { flex: 1.5 1 0; }

/* 列间隙 = 26px 空白 + 1px 发丝线。线只在列的左边缘，所以只给非首列 */
.u-stat + .u-stat {
  position: relative;
  padding-left: 26px;
  margin-left: 26px;
}
.u-stat + .u-stat::before {
  content: "";
  position: absolute;
  left: 0;
  top: 3px;
  bottom: 3px;
  width: 1px;
  background: var(--rule);
}

/* 标签行：标签在左、delta 靠右，同一基线 */
.u-stat-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
  min-width: 0;
}

/* 微标签：9.5px 大写 + .15em 字距 —— 与左栏分组头、组头读数**同一套字阶**。

   ⚠️ 颜色用 --fg-2 而不是 --fg-3。
   初版为了“弱化”用了 --fg-3，实测深色 3.22:1 / 浅色 2.83:1 —— 两个都不达标。
   判断依据：标签是“这个数字是什么”的唯一标识，没有它数字没有意义；
   它属于**要读的文字**，不是单位后缀那种装饰元素。
   9.5px 字号下只要 3:1 就是硬伤，何况 2.83。 */
.u-stat-label {
  font-size: 9.5px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* delta：标签的解释，落在标签行右侧。
   up / down 才上色（强调/危险），其余用 --fg-2。 */
.u-stat-aux {
  margin-left: auto;
  flex-shrink: 0;
  font-size: 10.5px;
  letter-spacing: 0.01em;
  color: var(--fg-2);
  white-space: nowrap;
}
.u-stat-aux.up {
  color: var(--c-accent-bright, var(--c-accent));
  font-weight: 600;
}
.u-stat-aux.down { color: var(--c-danger); }

.u-stat-value {
  display: flex;
  align-items: baseline;
  gap: 5px;
  line-height: 1;
}

.num {
  /* 30px / 字重 340：大字号 + 细字重才是排印（见文件头说明）。
     tabular-nums 保证多列之间数字宽度一致，小数点能竖着对齐。 */
  font-size: 30px;
  font-weight: 340;
  letter-spacing: -0.7px;
  color: var(--fg);
  font-variant-numeric: tabular-nums;
  line-height: 1.04;
}

.unit {
  font-size: 11px;
  font-weight: 500;
  /* 单位是纯装饰（信息已由前面的数字 + 标签承载），可以用弱色；
     但 --fg-3 在浅色实测 2.83:1，太接近“看不见”。
     单位实际是**帮读**的（“7.8” 还是 “7.8 GB” 差别很大），所以用 --fg-2。 */
  color: var(--fg-2);
}

.u-stat-note {
  margin: 0;
  font-size: 10.5px;
  /* 注释行同样是要读的文字（“需要扫描”“已聚类”是状态信息），用 --fg-2 */
  color: var(--fg-2);
  letter-spacing: 0.01em;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* ── 微型柱阵（底部）────────────────────────────────────────────────────
   短、高、密：读作这段文字的收尾，而不是一条独立趋势图。
   最后一根用强调色（"最新"），其余用发丝线色 —— 与全站 sparkline 同语言。 */
.u-stat-bars {
  display: flex;
  align-items: flex-end;
  gap: 2px;
  height: 20px;
  margin-top: auto;
  padding-top: 6px;
}
.u-stat-bars i {
  display: block;
  flex: 1 1 0;
  min-width: 2px;
  border-radius: 1px 1px 0 0;
  background: var(--fg-2);
  opacity: 0.45;
}
.u-stat-bars i.on {
  background: var(--c-accent);
  opacity: 1;
}

/* ── 占比线（2px）──────────────────────────────────────────────────────
   ⚠️ 默认**不用金色**：只要有一条满格金线，同排其余四列就全部变成
   "没被选中的"。金色只给 progressTone:'accent' —— 即调用方明确
   指出的那一个"这一项是重点"。 */
.u-stat-line {
  display: block;
  margin-top: 4px;
  height: 2px;
  border-radius: 2px;
  background: var(--rule);
  position: relative;
  overflow: hidden;
}
.u-stat-line i {
  position: absolute;
  inset: 0 auto 0 0;
  width: var(--pct, 0%);
  border-radius: 2px;
  /* 占比线是**图形**不是文字，不受 4.5:1 约束，但仍需可见
     （--f-3 在浅底上只有 2.83:1，2px 细线会看不太清）。 */
  background: var(--fg-2);
  transition: width var(--dur-3) var(--ease-out);
}
.u-stat-line.hot i {
  background: linear-gradient(90deg, rgba(212, 162, 74, 0.5), var(--c-accent));
}

.u-stat-inline {
  margin-left: auto;
  display: flex;
  align-items: flex-end;
  padding-bottom: 2px;
}

/* 窄窗：允许横向滚动，**绝不换行**。
   为什么不换行：换行后发丝竖线会落在"行首"，读起来像新分组，语义错；
   而且第二行的列会被拉到更宽，两行尺寸不一致（这正是旧版的问题）。
   也不隐藏：统计是这一屏的核心内容，隐藏等于删除。 */
@media (max-width: 980px) {
  .u-statband {
    overflow-x: auto;
    padding: 16px var(--sp-5) 14px;
    scrollbar-width: none;
    /* 滚动到底时最后一条线不该贴边消失，留一点余量 */
    scroll-padding-right: var(--sp-5);
  }
  .u-statband::-webkit-scrollbar {
    display: none;
  }
  .u-stat {
    flex: 0 0 auto;
    min-width: 118px;
  }
  .u-stat.w-2 { min-width: 150px; }
  .num {
    font-size: 26px;
  }
}
</style>
