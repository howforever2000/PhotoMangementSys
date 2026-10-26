/**
 * 设计令牌的可访问性约束（FEAT-103 · P0/P1）
 *
 * 为什么这条测试必须存在：
 *   初稿的令牌值是从效果图上取的（46% / 26% 白），上屏实测 fg-3 只有 2.09:1，
 *   浅色更糟（1.95:1）。这类问题**肉眼在浅色下能看出"灰得发虚"，但说不出差多少**，
 *   所以必须用数值守住 —— 否则下一轮"再调淡一点更好看"就会静默复发。
 *
 * 校验对象是**合成后**的颜色（前景 alpha 叠在背景上），不是字面值 ——
 * rgba(255,255,255,.26) 本身没有对比度，叠在不同底色上结果完全不同。
 *
 * 运行：node --experimental-strip-types --test src/assets/tokens.test.ts
 */
import test from "node:test";
import assert from "node:assert/strict";

type Rgb = [number, number, number];

function relLum(c: Rgb): number {
  const f = (x: number) => {
    const v = x / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
}

function contrast(a: Rgb, b: Rgb): number {
  const l1 = relLum(a);
  const l2 = relLum(b);
  const hi = Math.max(l1, l2);
  const lo = Math.min(l1, l2);
  return (hi + 0.05) / (lo + 0.05);
}

/** 把带 alpha 的前景叠到不透明的背景上 */
function blend(fg: Rgb, alpha: number, bg: Rgb): Rgb {
  return [
    Math.round(fg[0] * alpha + bg[0] * (1 - alpha)),
    Math.round(fg[1] * alpha + bg[1] * (1 - alpha)),
    Math.round(fg[2] * alpha + bg[2] * (1 - alpha)),
  ];
}

/* —— 令牌值（与 tokens.css 保持同步；改动任一方向都要更新这里） —— */
const DARK = {
  bg: [0x0a, 0x09, 0x0d] as Rgb,
  fg: [0xf2, 0xf0, 0xf4] as Rgb,
  fg2Alpha: 0.58,
  fg3Alpha: 0.44,
  accent: [0xd4, 0xa2, 0x4a] as Rgb,
  danger: [0xd9, 0x6a, 0x6a] as Rgb,
};

const LIGHT = {
  bg: [0xfa, 0xf8, 0xf5] as Rgb,
  fg: [0x17, 0x16, 0x1a] as Rgb,
  /* FEAT-116：预设重排后最暗的浅色端降到 #d5af8a（陶土），
     .62 在该底上只有 3.9:1 —— 提到 .70 才回到 5.0:1 一档。 */
  fg2Alpha: 0.70,
  fg3Alpha: 0.60,
  accent: [0x7d, 0x57, 0x1a] as Rgb,
  danger: [0xb4, 0x45, 0x3f] as Rgb,
};

for (const [label, t] of [
  ["深色", DARK],
  ["浅色", LIGHT],
] as const) {
  test(`${label}：fg-2（可读次级文字）达到 AA 4.5:1`, () => {
    const c = contrast(blend(t.fg, t.fg2Alpha, t.bg), t.bg);
    assert.ok(c >= 4.5, `实际 ${c.toFixed(2)}:1，低于 AA 要求`);
  });

  test(`${label}：fg-3（装饰单位）不低于 3.0:1`, () => {
    const c = contrast(blend(t.fg, t.fg3Alpha, t.bg), t.bg);
    assert.ok(c >= 3.0, `实际 ${c.toFixed(2)}:1，装饰单位也不能低于 3:1`);
  });

  test(`${label}：fg-3 必须明显弱于 fg-2（否则失去分档意义）`, () => {
    const c2 = contrast(blend(t.fg, t.fg2Alpha, t.bg), t.bg);
    const c3 = contrast(blend(t.fg, t.fg3Alpha, t.bg), t.bg);
    assert.ok(c2 - c3 >= 0.8, `两档差异仅 ${(c2 - c3).toFixed(2)}:1，分不出层次`);
  });

  test(`${label}：主前景 fg 达到 AAA 7:1`, () => {
    const c = contrast(t.fg, t.bg);
    assert.ok(c >= 7, `实际 ${c.toFixed(2)}:1`);
  });

  test(`${label}：强调色在背景上达到 AA 4.5:1（它承载变化值等信息）`, () => {
    const c = contrast(t.accent, t.bg);
    assert.ok(c >= 4.5, `实际 ${c.toFixed(2)}:1`);
  });

  test(`${label}：危险色在背景上达到 AA 4.5:1`, () => {
    const c = contrast(t.danger, t.bg);
    assert.ok(c >= 4.5, `实际 ${c.toFixed(2)}:1`);
  });
}

test("深浅两套的 fg-2 对比度接近（切换主题时层次不应跳变）", () => {
  const d = contrast(blend(DARK.fg, DARK.fg2Alpha, DARK.bg), DARK.bg);
  const l = contrast(blend(LIGHT.fg, LIGHT.fg2Alpha, LIGHT.bg), LIGHT.bg);
  assert.ok(Math.abs(d - l) < 0.6, `深 ${d.toFixed(2)} vs 浅 ${l.toFixed(2)}，差异过大`);
});
