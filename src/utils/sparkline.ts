/**
 * Sparkline 高度归一化（FEAT-103 · P1）
 *
 * 为什么把这段从 .vue 里抽出来：
 *   ① CS1（功能解耦）：组件只负责渲染，数值变换属于纯逻辑；
 *   ② 项目测试跑在 node:test（无 DOM），SFC 测不了，纯函数可测；
 *   ③ 归一化规则有几个边界情况（全等值 / 单值 / NaN / 负数）必须被断言锁住，
 *      否则将来改一版"从 0 起"就会让一排持平的数据变成一片空白柱。
 */

/** 归一化选项 */
export interface SparkOptions {
  /** 最低柱高（%）：保证"接近 0"也能看见 */
  floorPct?: number;
  /** 是否从 min 起（false = 从 0 起） */
  fromMin?: boolean;
}

/**
 * 把数值序列映射为高度百分比序列。
 *
 * 规则：
 *   - 空数组 → 空数组（调用方据此决定不渲染）
 *   - 全等值（span≈0）→ 统一 60%（既不是满格也不是空，语义是"没有变化"）
 *   - 正常区间 → 线性映射到 [floorPct, 100]
 *   - 非有限值（NaN/Infinity）→ 落到 floorPct，不让它把整列顶破
 */
export function toBarPercents(data: readonly number[], opts: SparkOptions = {}): number[] {
  const { floorPct = 8, fromMin = true } = opts;
  if (data.length === 0) return [];

  const finite = data.filter((v) => Number.isFinite(v));
  if (finite.length === 0) return data.map(() => floorPct);

  const max = Math.max(...finite);
  const min = fromMin ? Math.min(...finite) : 0;
  const span = max - min;

  return data.map((v) => {
    if (!Number.isFinite(v)) return floorPct;
    if (span < 1e-9) return 60;
    const pct = ((v - min) / span) * (100 - floorPct) + floorPct;
    // 夹紧：浮点误差不该让柱子捅出容器
    return Math.min(100, Math.max(floorPct, pct));
  });
}
