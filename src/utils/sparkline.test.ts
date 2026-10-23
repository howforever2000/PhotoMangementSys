/**
 * Sparkline 归一化规则（FEAT-103 · P1）
 *
 * 这些断言锁的是"形状语言"而不是实现细节 —— 一旦有人把 fromMin 改成
 * 从 0 起，或者把全等值的 60% 改成 0%，持平的读数会突然变成一排空白柱，
 * 用户会以为"没有数据"。所以必须由测试守住。
 *
 * 运行：node --experimental-strip-types --test src/utils/sparkline.test.ts
 */
import test from "node:test";
import assert from "node:assert/strict";

import { toBarPercents } from "./sparkline.ts";

test("空序列 → 空结果（调用方据此不渲染）", () => {
  assert.deepEqual(toBarPercents([]), []);
});

test("单值 → 统一 60%（没有可比对象时不表达方向）", () => {
  assert.deepEqual(toBarPercents([42]), [60]);
});

test("全等值 → 统一 60%，而不是 0% 或 100%", () => {
  const out = toBarPercents([12, 12, 12, 12]);
  assert.deepEqual(out, [60, 60, 60, 60]);
});

test("线性映射到 [floor, 100]：最小值落在 floor，最大值落在 100", () => {
  const out = toBarPercents([10, 20, 30], { floorPct: 8 });
  assert.equal(Math.round(out[0] * 100) / 100, 8);
  assert.equal(out[2], 100);
  // 中值应落在 (8+100)/2 = 54
  assert.equal(Math.round(out[1] * 100) / 100, 54);
});

test("fromMin=false 时从 0 起：数值全在 90~100 会给出接近满格的柱", () => {
  /* 注意：即便从 0 起，仍会映射到 [floorPct, 100] 而不是 [0, 100] ——
     因为 floorPct 是“最小柱高”的硬约束，全系数值都要遵守。
     所以 90/100 会得到 8 + 0.9*(100-8) = 90.8，而不是裸的 90。 */
  const out = toBarPercents([90, 100], { fromMin: false, floorPct: 8 });
  assert.equal(out[0], 90.8);
  assert.equal(out[1], 100);
});

test("fromMin=false 时 0 值落在 floorPct（基准线可见，而不是柱子消失）", () => {
  const out = toBarPercents([0, 50, 100], { fromMin: false, floorPct: 8 });
  assert.equal(out[0], 8);
  assert.equal(out[1], 54);
  assert.equal(out[2], 100);
});

test("fromMin=true 时贴近的数值也能拉开差异（这正是默认行为的目的）", () => {
  const out = toBarPercents([90, 100], { fromMin: true, floorPct: 8 });
  assert.equal(out[0], 8);
  assert.equal(out[1], 100);
});

test("极小差异不会被抹平：90 与 91 之间仍有可见高度差", () => {
  const [a, b] = toBarPercents([90, 91], { floorPct: 8 });
  assert.ok(b - a > 50, `高度差应远大于视觉阈值，实际 ${b - a}`);
});

test("非有限值（NaN / Infinity）退化为 floor，不污染整列", () => {
  const out = toBarPercents([NaN, 10, Infinity, 20], { floorPct: 8 });
  assert.equal(out[0], 8);
  assert.equal(out[2], 8);
  // 合法值仍按有限值的 min/max 归一化
  assert.equal(out[1], 8);
  assert.equal(out[3], 100);
});

test("负数序列也能正确归一化（不假设数据非负）", () => {
  const out = toBarPercents([-5, 0, 5], { floorPct: 10 });
  assert.equal(out[0], 10);
  assert.equal(out[1], 55);
  assert.equal(out[2], 100);
});

test("所有输出恒在 [floorPct, 100] 内（夹紧保证）", () => {
  for (const data of [[1, 2, 3], [0, 0, 0], [-1e9, 1e9], [1e-12, 2e-12]]) {
    for (const v of toBarPercents(data)) {
      assert.ok(v >= 8 && v <= 100, `越界值 ${v}（数据 ${JSON.stringify(data)}）`);
    }
  }
});
