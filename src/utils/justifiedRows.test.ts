/**
 * 行式密排算法（FEAT-106）
 *
 * 运行：node --experimental-strip-types --test src/utils/justifiedRows.test.ts
 *
 * 为什么这套算法必须有测试：
 *   它是**纯几何**（宽度怎么分、行怎么切），错了不会报错，只会"看起来有点空"。
 *   而"看起来有点空"正是上一版 dense 实现的老毛病 —— 没有人能从截图里
 *   断言出"这一行到底差几像素填满"，但这里可以。
 */
import test from "node:test";
import assert from "node:assert/strict";
import { justifyRows } from "./justifiedRows.ts";

const OPTS = { count: 24, containerWidth: 1200, gap: 12, captionH: 34, nominalHeight: 200 };

test("所有瓦片都出现且各出现一次", () => {
  const rows = justifyRows(OPTS);
  const idx = rows.flatMap((r) => r.cells.map((c) => c.index));
  assert.equal(idx.length, OPTS.count);
  assert.deepEqual([...idx].sort((a, b) => a - b), Array.from({ length: OPTS.count }, (_, i) => i));
});

test("每一行都精确铺满容器宽度（无空洞，末行也不例外）", () => {
  const rows = justifyRows(OPTS);
  for (let r = 0; r < rows.length; r++) {
    const used = rows[r].cells.reduce((s, c) => s + c.width, 0) + 12 * (rows[r].cells.length - 1);
    assert.equal(used, 1200, `第 ${r} 行宽度 ${used} ≠ 1200`);
  }
});

test("没有巨幅瓦片（任何一块宽度不超过容器的一半）", () => {
  // 回归用例：旧算法在封面高被上限夹住时，会把差额补给最后一块，
  // 出现 775px 宽的扁片。新算法必须杜绝这种情况。
  const rows = justifyRows(OPTS);
  for (const r of rows) {
    for (const c of r.cells) {
      assert.ok(c.width <= 1200 * 0.5, `出现巨幅瓦片：${c.width}px（行 ${r.cells.length} 块）`);
    }
  }
});

test("封面高度始终在上下限区间内", () => {
  const rows = justifyRows(OPTS);
  for (const r of rows) {
    for (const c of r.cells) {
      assert.ok(c.coverH >= 110, `封面高 ${c.coverH} 低于下限`);
      assert.ok(c.coverH <= 300, `封面高 ${c.coverH} 超过上限`);
    }
  }
});

test("同一行内所有瓦片的封面高度一致（否则图注会错位）", () => {
  const rows = justifyRows(OPTS);
  for (const r of rows) {
    const hs = new Set(r.cells.map((c) => c.coverH));
    assert.equal(hs.size, 1, `行内高度不一致：${[...hs].join(",")}`);
  }
});

test("每行的高度 = 封面高 + 图注高", () => {
  const rows = justifyRows(OPTS);
  for (const r of rows) {
    assert.equal(r.height, r.cells[0].coverH + 34);
  }
});

test("宽幅瓦片按固定周期出现（可预期的错落，不是随机）", () => {
  const rows = justifyRows(OPTS);
  const wideIdx = rows.flatMap((r) => r.cells.filter((c) => c.wide).map((c) => c.index));
  // wideEvery=5 → 宽幅出现在 index 4, 9, 14, 19…
  assert.deepEqual(wideIdx, [4, 9, 14, 19]);
});

test("宽幅瓦片的宽度约为窄幅的两倍", () => {
  const rows = justifyRows(OPTS);
  const row = rows[0];
  const narrow = row.cells.find((c) => !c.wide)!;
  const wide = row.cells.find((c) => c.wide);
  if (wide) {
    const ratio = wide.width / narrow.width;
    assert.ok(ratio > 1.7 && ratio < 2.3, `宽/窄比例 ${ratio.toFixed(2)} 偏离 2 倍`);
  }
});
test("容器宽度变化时行数随之变化（宽度自适应）", () => {
  const narrow = justifyRows({ ...OPTS, containerWidth: 800 });
  const wide = justifyRows({ ...OPTS, containerWidth: 1600 });
  assert.ok(narrow.length > wide.length, "窄容器应产生更多行");
});

test("封面高度被上下限夹住（末行不会变成巨幅横幅）", () => {
  const rows = justifyRows({ ...OPTS, count: 3, containerWidth: 1200 });
  for (const r of rows) {
    for (const c of r.cells) {
      assert.ok(c.coverH >= 110, `封面高 ${c.coverH} 低于下限`);
      assert.ok(c.coverH <= 300, `封面高 ${c.coverH} 超过上限`);
    }
  }
});

test("末行不会只剩一块（不会变成巨幅横幅）", () => {
  // 回归用例：曾出现过"末行只剩一块 → 宽度等于整个容器 → 被夹到高度上限 →
  // 变成一条 1120×260 的扁片"。现在算法会枚举行数并惩罚单块行。
  for (const count of [5, 7, 9, 12, 13, 24, 25, 37, 129]) {
    const rows = justifyRows({ ...OPTS, count });
    if (count <= 2) continue;
    const singles = rows.filter((r) => r.cells.length === 1).length;
    assert.equal(singles, 0, `count=${count} 时出现了 ${singles} 个单块行`);
  }
});

test("任何一行都不超过容器的一半宽度（无巨幅）", () => {
  for (const count of [5, 9, 12, 24, 129]) {
    const rows = justifyRows({ ...OPTS, count });
    for (const r of rows) {
      for (const c of r.cells) {
        assert.ok(c.width <= OPTS.containerWidth * 0.55, `count=${count} 出现 ${c.width}px 的巨幅瓦片`);
      }
    }
  }
});

test("count=0 返回空数组", () => {
  assert.deepEqual(justifyRows({ ...OPTS, count: 0 }), []);
});

test("容器宽度极小时不产生 0 宽或负宽的瓦片", () => {
  const rows = justifyRows({ ...OPTS, count: 5, containerWidth: 40 });
  for (const r of rows) {
    for (const c of r.cells) {
      assert.ok(c.width > 0, `宽度 ${c.width} 非正`);
    }
  }
});
