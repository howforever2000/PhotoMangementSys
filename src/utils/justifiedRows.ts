/**
 * 行式密排（justified rows）—— 相册墙的排布算法
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么不用 CSS grid 的 auto-fill + dense
 * ──────────────────────────────────────────────────────────────────────────
 * 旧实现用 `grid-auto-flow: dense` + 大瓦片跨 2 列 4 行，想靠 dense 回填制造错落。
 * 实测（Playwright 截图）有两个硬伤：
 *   ① 大瓦片横跨两列后，它"横跨"的那两列在**上方一行**已被占满，
 *      剩下的空位是奇数个单元格 —— dense 回填需要连续空位，
 *      于是行尾出现一整块无法回填的空洞（截图右侧那片空白）；
 *   ② dense 的回填顺序依赖元素流，用户滚动/筛选后同一个相册会"跳位置"，
 *      找不到上次看到的那张。
 *
 * 行式密排（Google Photos / Apple Photos 同款）没有这两个问题：
 *   · 每一行是一个 flex 容器，瓦片宽度按比例分配，行宽**恰好等于容器宽度**
 *     —— 不存在空洞，也不需要 dense；
 *   · 行的分配是纯函数（同样的输入必然得到同样的输出），
 *     位置稳定，筛选后也不会乱跳。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 算法（三步，无级联）
 * ──────────────────────────────────────────────────────────────────────────
 * 记：W = 容器宽，g = 缝，n = 瓦片数，a_i = 第 i 块的封面宽高比，
 *     A = Σa_i（总宽高比）。一行有 k 块、封面高 h 时，该行宽度 = h·ΣA_row + g·(k−1)。
 *     "所有行都铺满 W" ⇒ R·W = h·A + g·(n − R)，R 为行数 ⇒
 *
 *         h = (R·W − g·(n − R)) / A
 *
 * 1. **先定行数 R**：让 h 尽量接近名义高度 H0
 *        R = round((H0·A + g·n) / (W + g))
 *    夹到 [1, n]。A 只跟宽高比序列有关，所以 R 是确定值 ——
 *    不会因为"从头逐行贪心、末尾再补救"而在末尾累积误差。
 * 2. **按 h 切行**：往当前行加瓦片，直到 ΣA_row ≥ (W − g·(k−1)) / h
 *    （即"这一行已经够高了，不能再加"）。因为每加一块 ΣA 就涨，
 *    条件必然会在有限步内成立，不会死循环。
 * 3. **反算每行封面高**：h_row = (W − g·(k−1)) / ΣA_row，夹到
 *    [minCoverH, maxCoverH]；宽度按 a_i / ΣA_row 比例分配，
 *    再把取整误差补给最后一块 ⇒ 行宽精确 = W。
 *
 * ⚠️ 为什么不做"逐行调平"（把太高一行的末尾挪到下一行、下一行再往下一行挪）：
 *   实测过，那种做法会**逐行向上级联**：末行太高 → 从上一行抽一块 →
 *   上一行变太高 → 再从上一行的上一行抽 → 一路抽到第一行只剩一块，
 *   最后首行变成一条 1120px 宽的扁片（真实截图确认）。
 *   "先算全局行数"从根上没有这个问题。
 *
 * 为什么用**整数比例**而不是随机宽高：随机布局每次刷新都不一样，
 * 用户找不到"上次那个相册在哪"。周期性错落是可预期的。
 */

/** 一行里的一张瓦片 */
export interface JustifiedCell {
  /** 在原始序列中的下标（调用方用它去取数据） */
  index: number;
  /** 瓦片宽度（px，整数） */
  width: number;
  /** 封面高度（px，整数，整行共用） */
  coverH: number;
  /** 是否宽幅瓦片（封面宽高比是窄幅的 wideFactor 倍） */
  wide: boolean;
}

/** 一行 */
export interface JustifiedRow {
  /** 行内瓦片（按顺序） */
  cells: JustifiedCell[];
  /** 整行高度（封面 + 图注） */
  height: number;
}

export interface JustifyOptions {
  /** 瓦片总数 */
  count: number;
  /** 容器宽度（px） */
  containerWidth: number;
  /** 瓦片之间的缝（px） */
  gap?: number;
  /** 图注条高度（px）——固定值才能保证同行瓦片严格对齐 */
  captionH?: number;
  /** 名义封面高度（px）：实际封面高在其附近浮动 */
  nominalHeight?: number;
  /** 窄幅瓦片的封面宽高比（宽/高） */
  baseAspect?: number;
  /** 每隔几个放一块宽幅瓦片（0 = 不放宽幅） */
  wideEvery?: number;
  /** 封面高度下限（px） */
  minCoverH?: number;
  /** 封面高度上限（px）——防止瓦片数很小时被拉成巨幅横幅 */
  maxCoverH?: number;
  /**
   * 宽幅瓦片的宽高比倍率。
   * 2 = 宽幅是窄幅的两倍宽（3:1 的横幅）。
   * 1.5 = 只宽 1.5 倍（2.25:1，更克制）。
   */
  wideFactor?: number;
}

const DEFAULTS = {
  gap: 12,
  captionH: 34,
  nominalHeight: 200,
  baseAspect: 1.5,
  wideEvery: 5,
  minCoverH: 110,
  maxCoverH: 300,
  wideFactor: 1.5,
};

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/**
 * 计算行式密排。
 *
 * 返回的每一行都**精确铺满容器宽度**（宽度之和 + 缝隙 = 容器宽度），
 * 所以相册墙不会出现空洞，也不需要 grid 的 dense 回填。
 */
export function justifyRows(opts: JustifyOptions): JustifiedRow[] {
  const gap = opts.gap ?? DEFAULTS.gap;
  const captionH = opts.captionH ?? DEFAULTS.captionH;
  const H0 = opts.nominalHeight ?? DEFAULTS.nominalHeight;
  const base = opts.baseAspect ?? DEFAULTS.baseAspect;
  const wideEvery = opts.wideEvery ?? DEFAULTS.wideEvery;
  const wideFactor = opts.wideFactor ?? DEFAULTS.wideFactor;
  const minCoverH = opts.minCoverH ?? DEFAULTS.minCoverH;
  const maxCoverH = opts.maxCoverH ?? DEFAULTS.maxCoverH;
  const n = opts.count;

  const rows: JustifiedRow[] = [];
  if (n <= 0) return rows;
  // 容器宽度还没量出来（首帧 / 隐藏容器）时给一个保守值，
  // 避免算出 0 宽的行把布局撑坏。
  const W = Math.max(160, Math.round(opts.containerWidth) || 1200);

  // ── 每块瓦片的名义宽高比（宽幅周期是**全局**的，跨行连续）───────────
  const aspects: number[] = [];
  const wides: boolean[] = [];
  for (let i = 0; i < n; i++) {
    const wide = wideEvery > 0 && i % wideEvery === wideEvery - 1;
    aspects.push(base * (wide ? wideFactor : 1));
    wides.push(wide);
  }
  const totalAspect = aspects.reduce((s, a) => s + a, 0);

  // ── 1. 枚举行数 R，按"铺满后的形状"打分选最优 ────────────────────
  //
  // 为什么不能只靠一个公式定 R：
  //   公式假设"每行高度严格相等"，但切行是离散的（每行只能装整数块），
  //   少数时候会让末行只剩一块；而末行一块的宽度就是整个 W，
  //   会被夹到高度上限，于是变成一条扁片（实测过）。
  //   枚举 R 并打分能从根上避开这种形状，且结果仍是纯函数。
  //
  // 分行方式：**按宽高比均匀分配**（每行承担 totalAspect/R 的预算），
  // 而不是"贪心装到装不下为止" —— 后者天然会让前面的行偏大、末行偏小。
  let best: { groups: number[][]; score: number } | null = null;

  // R 的上界取 n/2（即每行至少 2 块）：单块成行在视觉上就是一条横幅。
  // n ≤ 2 时只能老老实实 1~2 行。
  const maxR = n <= 2 ? n : Math.floor(n / 2);
  for (let R = 1; R <= maxR; R++) {
    const groups = splitEvenly(R);
    const score = scoreGroups(groups);
    if (!best || score < best.score - 1e-9) best = { groups, score };
  }

  /**
   * 把 n 块瓦片按宽高比分给 R 行，每行预算 totalAspect/R。
   *
   * 为什么按宽高比而不用块数平均：宽幅瓦片的宽高比是窄幅的 1.8 倍，
   * 按块数平均会让含宽幅的那一行明显比别人宽（高度就矮），
   * 而按宽高比平均能让每行高度自然接近相等 —— 这正是行式密排的形态。
   */
  function splitEvenly(R: number): number[][] {
    const groups: number[][] = [];
    const budget = totalAspect / R;
    let i = 0;
    for (let r = 0; r < R; r++) {
      const row: number[] = [];
      let sumA = 0;
      const rowsLeft = R - r;
      const itemsLeft = n - i;
      while (i < n) {
        const a = aspects[i];
        // 每行最多拿 min(itemsLeft, ...) 块，必须给后面的行留至少 2 块
        const mustKeep = Math.max(0, (rowsLeft - 1) * 2);
        if (row.length > 0 && itemsLeft - row.length <= mustKeep) break;
        // 加入下一块会不会离预算更远？——“更远”就收行
        const after = Math.abs(sumA + a - budget);
        const before = Math.abs(sumA - budget);
        if (row.length > 0 && after >= before && sumA > 0) break;
        row.push(i);
        sumA += a;
        i += 1;
      }
      if (row.length) groups.push(row);
      if (i >= n) break;
    }
    // 剩下的（理论上不会发生）并到最后一行
    if (i < n && groups.length) {
      while (i < n) groups[groups.length - 1].push(i++);
    }
    return groups;
  }

  /** 给一组分行打分：越低越好。偏差按**瓦片数加权**（大行影响更大）。 */
  function scoreGroups(groups: number[][]): number {
    let score = 0;
    for (const row of groups) {
      const k = row.length;
      if (!k) continue;
      const avail = W - gap * (k - 1);
      const sumA = row.reduce((s, idx) => s + aspects[idx], 0);
      const natural = avail / sumA;
      const w = k / n; // 按瓦片数加权：4 块的行比 2 块的行更影响观感
      // ① 偏离名义高度（归一化）
      score += w * (Math.abs(natural - H0) / H0);
      // ② 被夹到上下限：形状被拉扁/挤瘦
      if (natural > maxCoverH) score += w * (1.5 + (natural - maxCoverH) / H0);
      if (natural < minCoverH) score += w * (1.5 + (minCoverH - natural) / H0);
      // ③ 单块成行（n>1 时）是横幅，重罚
      if (k === 1 && n > 1) score += 6;
      // ④ 两块成行只在 n 很小时才合理
      else if (k === 2 && n > 4) score += 0.8;
    }
    return score;
  }

  const groups = best?.groups ?? [[...Array(n).keys()]];

  // ── 2. 生成行（宽度按比例分配，行宽精确 = W）────────────────────────
  for (const row of groups) {
    const k = row.length;
    if (!k) continue;
    const avail = W - gap * (k - 1);
    const sumA = row.reduce((s, idx) => s + aspects[idx], 0);
    // 反算该行的封面高，并夹到允许区间
    const coverH = Math.round(clamp(avail / sumA, minCoverH, maxCoverH));

    const cells: JustifiedCell[] = row.map((idx) => ({
      index: idx,
      wide: wides[idx],
      coverH,
      width: Math.round((aspects[idx] / sumA) * avail),
    }));

    // 取整误差（每格 0~1px）补给最后一块，保证行宽精确 = W
    const used = cells.reduce((s, c) => s + c.width, 0) + gap * (k - 1);
    if (used !== W) cells[cells.length - 1].width += W - used;

    rows.push({ cells, height: coverH + captionH });
  }

  return rows;
}
