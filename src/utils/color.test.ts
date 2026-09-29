/**
 * `utils/color.ts` 单元测试 —— FEAT-084 组件色调
 *
 * 钉死三条不变量：
 *   1. 默认白色派生结果 == 既有令牌（默认外观零变化）；
 *   2. 深色组件底必须翻成浅色文字，且翻的方向永远是「对比更高的一侧」；
 *   3. 次级底/描边必须与主底保持层级（浅底更暗、深底更亮）。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TEXT_DARK,
  TEXT_LIGHT,
  componentTone,
  contrastRatio,
  hexToRgb,
  hexToRgba,
  isDarkText,
  mixRgb,
  normalizeHex,
  relLum,
} from "./color.ts";

test("normalizeHex：归一化 #rgb/#RRGGBB，坏值回落白色", () => {
  assert.equal(normalizeHex("#FFFFFF"), "#ffffff");
  assert.equal(normalizeHex("fff"), "#ffffff");
  assert.equal(normalizeHex("  #E6F6EA  "), "#e6f6ea");
  assert.equal(normalizeHex("not-a-color"), "#ffffff");
  assert.equal(normalizeHex("#12"), "#ffffff");
});

test("默认白色：派生值与 main.css 浅色令牌一致（默认外观零变化）", () => {
  const tone = componentTone("#ffffff");
  assert.equal(tone.onDark, false);
  assert.equal(tone.surface, "#ffffff");
  assert.equal(tone.surface2, "#f5f5f5"); // 255*0.96
  assert.equal(tone.border, "#e0e0e0"); // 255*0.88
  assert.equal(tone.text, "#1f2733");
  assert.equal(tone.text3, "#4b5b6e");
});

test("深色组件底 → 浅色文字；浅色底 → 深色文字", () => {
  assert.equal(componentTone("#1c202b").onDark, true);
  assert.equal(componentTone("#242a38").text, "#f5f7ff");
  assert.equal(componentTone("#eef3fb").onDark, false);
  assert.equal(componentTone("#e6f6ea").onDark, false);
});

test("文字取向恒为对比度更高的一侧（中间灰也不选更差的那档）", () => {
  for (const hex of ["#808080", "#396cd8", "#c0c0c0", "#f5f6f8", "#101418"]) {
    const tone = componentTone(hex);
    const l = relLum(hexToRgb(hex));
    const chosen = tone.onDark
      ? contrastRatio(relLum(TEXT_LIGHT), l)
      : contrastRatio(relLum(TEXT_DARK), l);
    const other = tone.onDark
      ? contrastRatio(relLum(TEXT_DARK), l)
      : contrastRatio(relLum(TEXT_LIGHT), l);
    assert.ok(chosen >= other, `${hex} 应选对比更高的一侧`);
  }
});

test("次级底与描边保持层级：浅底更暗、深底更亮", () => {
  const light = componentTone("#eef3fb");
  assert.ok(
    relLum(hexToRgb(light.surface2)) < relLum(hexToRgb(light.surface)),
    "浅色组件的次级底应更暗",
  );

  const dark = componentTone("#1c202b");
  assert.ok(
    relLum(hexToRgb(dark.surface2)) > relLum(hexToRgb(dark.surface)),
    "深色组件的次级底应更亮",
  );
  assert.equal(dark.border, "rgba(255,255,255,.16)");
});

test("mixRgb 端点与插值", () => {
  assert.deepEqual(mixRgb([10, 20, 30], [110, 120, 130], 0), [10, 20, 30]);
  assert.deepEqual(mixRgb([10, 20, 30], [110, 120, 130], 1), [110, 120, 130]);
  assert.deepEqual(mixRgb([0, 0, 0], [255, 255, 255], 0.5), [128, 128, 128]);
});

test("hexToRgba / isDarkText", () => {
  assert.equal(hexToRgba("#ffffff", 0.94), "rgba(255,255,255,0.94)");
  assert.equal(hexToRgba("not-a-color", 1), "rgba(255,255,255,1)");
  assert.equal(hexToRgba("bad", 1), "rgba(187,170,221,1)"); // 3 位 hex 合法：bad→bbaadd
  assert.equal(isDarkText("#000000"), true);
  assert.equal(isDarkText("#ffffff"), false);
});

test("黑对白的 WCAG 对比度 ≈ 21:1（工具函数自检）", () => {
  const r = contrastRatio(relLum([255, 255, 255]), relLum([0, 0, 0]));
  assert.ok(Math.abs(r - 21) < 0.1, `got ${r}`);
});
