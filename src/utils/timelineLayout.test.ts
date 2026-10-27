// 时间线按月布局（FEAT-122）：行式密排的纯函数单测
//
// 跑法：npm test（node --experimental-strip-types --test "src/**/*.test.ts"）

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  TL_LAYOUT,
  findUnitAt,
  layoutMonth,
  monthUnitHeight,
  MONTH_HEAD_H,
  MONTH_TRAIL,
  ROW_GAP,
} from "./timelineLayout.ts";

const W = 1322;

test("每一行都精确铺满容器宽（未被夹住时）", () => {
  const { rows } = layoutMonth(40, W);
  assert.ok(rows.length > 0);
  for (const row of rows) {
    const sum = row.cells.reduce((n, c) => n + c.width, 0) + TL_LAYOUT.gap * (row.cells.length - 1);
    assert.equal(sum, W, `行宽应等于容器宽，实际 ${sum}`);
  }
});

test("行高始终落在 [minCoverH, maxCoverH] 区间", () => {
  for (const count of [1, 2, 3, 5, 8, 17, 60, 300]) {
    for (const row of layoutMonth(count, W).rows) {
      assert.ok(row.height >= TL_LAYOUT.minCoverH, `行高 ${row.height} 低于下限`);
      assert.ok(row.height <= TL_LAYOUT.maxCoverH, `行高 ${row.height} 高于上限`);
    }
  }
});

test("所有照片恰好出现一次（不重不漏、顺序递增）", () => {
  const count = 37;
  const { rows } = layoutMonth(count, W);
  const seen: number[] = [];
  for (const row of rows) for (const c of row.cells) seen.push(c.index);
  assert.deepEqual(seen, [...Array(count).keys()]);
});

test("月高度 = 标题 + Σ(行高+行距) + 尾部", () => {
  const layout = layoutMonth(23, W);
  const expect =
    MONTH_HEAD_H + layout.rows.reduce((n, r) => n + r.height + ROW_GAP, 0) + MONTH_TRAIL;
  assert.equal(monthUnitHeight(layout), expect);
});

test("高度只由 (张数, 容器宽) 决定 —— 同输入必须同输出（虚拟化落点精度的前提）", () => {
  const a = layoutMonth(97, 1322, 12);
  const b = layoutMonth(97, 1322, 12);
  assert.equal(monthUnitHeight(a), monthUnitHeight(b));
  assert.deepEqual(a.rows, b.rows);
  // 换宽度必须变（否则说明宽度没参与计算）
  const c = layoutMonth(97, 900, 12);
  assert.notEqual(monthUnitHeight(a), monthUnitHeight(c));
});

test("空月份不产生行、高度为 0", () => {
  const layout = layoutMonth(0, W);
  assert.equal(layout.rows.length, 0);
  assert.equal(layout.rowsHeight, 0);
});

test("单张也成行，且不会被拉成满宽横幅", () => {
  const { rows } = layoutMonth(1, W);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].cells.length, 1);
  assert.ok(rows[0].cells[0].width < W, "单块行宽度应小于容器宽（按比例出宽）");
});

test("宽幅周期跨月连续：第 5、10 张是宽幅", () => {
  const { rows } = layoutMonth(10, W);
  const widths = rows.flatMap((r) => r.cells.map((c) => c.width));
  // 宽幅格更宽；用「同标题行内」不方便比较，改用相对关系：
  // 至少能验证第 5 张（index=4）比相邻的第 4 张宽
  assert.ok(widths.length === 10);
  const wideFactors = layoutMonth(10, 600).rows
    .flatMap((r) => r.cells)
    .map((c) => c.width);
  assert.ok(wideFactors[4] > wideFactors[3], "第 5 张应为宽幅（更宽）");
  assert.ok(wideFactors[9] > wideFactors[8], "第 10 张应为宽幅（更宽）");
});

test("findUnitAt 二分定位：边界与越界都正确", () => {
  const tops = [0, 100, 250, 400, 900];
  assert.equal(findUnitAt(tops, 0), 0);
  assert.equal(findUnitAt(tops, 99), 0);
  assert.equal(findUnitAt(tops, 100), 1);
  assert.equal(findUnitAt(tops, 260), 2);
  assert.equal(findUnitAt(tops, 400), 3);
  assert.equal(findUnitAt(tops, 10_000), 4);
  assert.equal(findUnitAt(tops, -5), -1);
  assert.equal(findUnitAt([], 10), -1);
});

test("账本总高量级合理（按「每张占多少像素」判，而不是拍一个绝对数）", () => {
  // 24,980 张 / 97 个月 ≈ 258 张每月。容器 1322px、名义高 200 时一行约 5 张
  // → 理论上每张约 (200+8)/5 ≈ 42px。算出总量后再除回去，能直接看出行高是否算飞。
  let total = 0;
  const photos = 258 * 97;
  for (let i = 0; i < 97; i++) total += monthUnitHeight(layoutMonth(258, W, i * 258));
  const perPhoto = total / photos;
  assert.ok(perPhoto > 25, `每张 ${perPhoto.toFixed(1)}px 过小，说明行被压得太扁`);
  assert.ok(perPhoto < 80, `每张 ${perPhoto.toFixed(1)}px 过大，说明行高算错`);
});
