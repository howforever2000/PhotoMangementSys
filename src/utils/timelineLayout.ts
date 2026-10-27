/**
 * 时间线的「行式密排 + 高度账本」（FEAT-122）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么要重写（而不是继续用 utils/justifiedRows）
 * ───────────────────────────────────────────────────────────────────────────
 * 改前时间线把**整条序列**（24,980 张）一次性喂给 `justifyRows`，并且传了
 * `breakPoints`（按月强制断行）。而 `justifyRows` 收到 breakPoints 时会先跑完
 * 「枚举行数 R 再打分」那段 O(n²) 的搜索（`for R in 1..=n/2`，每个 R 都
 * `splitEvenly` + `scoreGroups` 各扫一遍 n），最后把结果**整段丢弃**，
 * 改走「逐段贪心」。实测这就是首屏 2~4 s 里最重的一块。
 *
 * 本模块只服务于时间线，做三件事：
 *   1. **按月独立排**（O(n) 贪心，不枚举行数）—— 因为虚拟化要求每月的
 *      高度必须是「只由 (张数, 容器宽) 决定的纯函数」，不能依赖邻居；
 *   2. 给出**像素级精确的月高度**，供虚拟列表算占位与跳转落点；
 *   3. 保持与原来一致的观感：同一套 gap / 名义高 / 宽幅周期（周期跨月连续）。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么高度必须可预算
 * ───────────────────────────────────────────────────────────────────────────
 * 虚拟列表要在「明细还没加载」时就把滚动条长度算准，否则：
 *   · 占位高度不准 → 跳转落点漂移（点一下左侧月份，跳过去发现偏了几百像素）；
 *   · 加载后真实高度变化 → 滚动位置被动跳。
 * 好消息是：时间线的宽高比**本来就不用真实图片尺寸**（`justifyRows` 一直用
 * `baseAspect` 名义比例），所以 `(count, width)` 就能算出精确行高，不需要
 * 任何照片元数据 —— 这也是这条优化能成立的前提。
 */

/** 一行里的一个格子（index 是在**本月内**的下标） */
export interface LayoutCell {
  index: number;
  width: number;
}

export interface LayoutRow {
  cells: LayoutCell[];
  /** 行高 = 封面高（时间线图注叠在封面上，不额外占高） */
  height: number;
}

export interface MonthLayout {
  rows: LayoutRow[];
  /**
   * 行区域的总高度（不含月份标题）。
   *
   * 每行后面都有 8px 行距（与 `.tl-row { margin-bottom: 8px }` 一致），
   * 所以 = Σ(row.height + ROW_GAP)。最后一行的那 8px 也算在内，
   * 由 `MONTH_TRAIL` 里一并消化（见 `MONTH_PAD` 的注释）。
   */
  rowsHeight: number;
}

/** 与改前 `JUSTIFY_OPTS` 完全一致的视觉参数（改口径会改变观感，别乱动） */
export const TL_LAYOUT = {
  gap: 8,
  nominalHeight: 200,
  baseAspect: 1.28,
  wideEvery: 5,
  wideFactor: 1.8,
  minCoverH: 130,
  maxCoverH: 260,
} as const;

// ---------------------------------------------------------------------------
// 三个「结构高度」常量：DOM 与账本共用同一组数字
// ---------------------------------------------------------------------------
//
// ⚠️ 这三个值必须与 Timeline.vue 里 `.tl-year-title` / `.tl-month-title`
// 的**显式高度**保持一致（那里不用 margin/line-height 撑高，而是写死 px，
// 就是为了让账本与真实布局永远对得上 —— margin 还会父子合并，算不准）。

/** 年标题块高度（含标题下方留白） */
export const YEAR_UNIT_H = 66;
/** 月标题块高度（标题 22 + 下方 8） */
export const MONTH_HEAD_H = 30;
/** 行间距（与 `.tl-row { margin-bottom }` 一致） */
export const ROW_GAP = 8;
/** 月份尾部留白（含最后一行的 8px 行距 + 与下一个月之间的呼吸） */
export const MONTH_TRAIL = 20;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * 名义宽高比。宽幅周期**跨月连续**（用全局序号 `phaseOffset` 定位），
 * 这样每个月第 5 张恰好是宽幅的观感不会在每个月初重新起跳。
 */
function aspectAt(globalIndex: number): number {
  const wide =
    TL_LAYOUT.wideEvery > 0 && globalIndex % TL_LAYOUT.wideEvery === TL_LAYOUT.wideEvery - 1;
  return TL_LAYOUT.baseAspect * (wide ? TL_LAYOUT.wideFactor : 1);
}

/**
 * 排一个月的行（O(n) 贪心：往当前行塞，直到这一行「高过名义高」就收行）
 *
 * 与 `justifyRows` 的 breakPoints 分支逐字同源，只是去掉了那段被丢弃的
 * 行数枚举 —— 月与月之间本来就是硬断点，枚举结果从来没被用过。
 *
 * @param count       该月照片数
 * @param width       容器宽（px）
 * @param phaseOffset 该月第一张在**整条序列**里的全局下标（只影响宽幅周期）
 */
export function layoutMonth(
  count: number,
  width: number,
  phaseOffset = 0,
): MonthLayout {
  const { gap, nominalHeight: H0, minCoverH, maxCoverH } = TL_LAYOUT;
  if (count <= 0) return { rows: [], rowsHeight: 0 };

  // 容器还没量出来（首帧 / 隐藏）时给保守值，避免算出 0 宽把布局撑坏
  const W = Math.max(160, Math.round(width) || 1200);

  const aspects: number[] = new Array(count);
  for (let i = 0; i < count; i++) aspects[i] = aspectAt(phaseOffset + i);

  const rows: LayoutRow[] = [];
  let i = 0;
  while (i < count) {
    // 1. 贪心装行：塞到「行高已低于名义高」为止
    const from = i;
    let sumA = 0;
    while (i < count) {
      const k = i - from + 1;
      const hAfter = (W - gap * (k - 1)) / (sumA + aspects[i]);
      if (i > from && hAfter < H0) break;
      sumA += aspects[i];
      i += 1;
    }
    const k = i - from;
    const avail = W - gap * (k - 1);
    const natural = avail / sumA;
    const coverH = Math.round(clamp(natural, minCoverH, maxCoverH));
    // 被上下限夹住时不能再强行拉满 W（否则会把单块行拉成横幅）；
    // 未夹住时按比例分配，让整行精确铺满容器 —— 与 FEAT-115 的口径一致。
    const clampedRow = natural > maxCoverH || natural < minCoverH;

    const cells: LayoutCell[] = [];
    let used = 0;
    for (let j = from; j < i; j++) {
      const w = clampedRow
        ? Math.round(aspects[j] * coverH)
        : Math.round((aspects[j] / sumA) * avail);
      cells.push({ index: j, width: w });
      used += w;
    }
    // 取整误差补给最后一块（只在精确铺满的分支）
    if (!clampedRow && cells.length) {
      const want = W - gap * (k - 1);
      const diff = want - used;
      if (diff !== 0) {
        const last = cells[cells.length - 1];
        cells[cells.length - 1] = { ...last, width: last.width + diff };
      }
    }
    rows.push({ cells, height: coverH });
  }

  const rowsHeight = rows.reduce((n, r) => n + r.height + ROW_GAP, 0);
  return { rows, rowsHeight };
}

/** 一个月的整体高度（标题 + 行 + 尾部留白）：虚拟列表的占位与落点都用它 */
export function monthUnitHeight(layout: MonthLayout): number {
  return MONTH_HEAD_H + layout.rowsHeight + MONTH_TRAIL;
}

/**
 * 二分：给定滚动位置（相对 board 顶部），返回命中的单元下标。
 *
 * 账本 `tops` 单调递增，所以可以在 O(log n) 里定位 ——
 * 这是替掉改前「每帧遍历 97 个节点做 getElementById + getBoundingClientRect」的关键。
 *
 * @returns 最后一个 `tops[i] <= y` 的下标；`y` 在首个单元之前时返回 `-1`
 */
export function findUnitAt(tops: number[], y: number): number {
  let lo = 0;
  let hi = tops.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (tops[mid] <= y) {
      ans = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return ans;
}
