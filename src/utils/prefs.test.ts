/**
 * `utils/prefs.ts` 单元测试 —— FEAT-086 旧数据兼容
 *
 * 钉死三条不变量：
 *   1. 旧结构（无 gradMid/compAlpha/material）→ 自动补齐且视觉等价；
 *   2. 坏值（非法 hex / 越界数值 / 非法枚举）一律回落默认，绝不抛错；
 *   3. 完整新结构原样保留。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULTS, normalizePrefs } from "./prefs.ts";

test("FEAT-087 saturation：越界被夹到 0.4~1.5，坏值回落 1", () => {
  assert.equal(normalizePrefs({}).saturation, 1);
  assert.equal(normalizePrefs({ saturation: 0.1 }).saturation, 0.4);
  assert.equal(normalizePrefs({ saturation: 9 }).saturation, 1.5);
  assert.equal(normalizePrefs({ saturation: "xx" }).saturation, 1);
  assert.equal(normalizePrefs({ saturation: 1.35 }).saturation, 1.35);
});

test("旧结构（FEAT-084 时代）→ 补出 gradMid 中点 + 默认 compAlpha/material", () => {
  const legacy = {
    mode: "dark",
    compColor: "#123456",
    bgStyle: "gradient",
    bgColor: "#0e211b",
    gradFrom: "#000000",
    gradTo: "#ffffff",
    gradAngle: 90,
    bgOpacity: 0.5,
  };
  const p = normalizePrefs(legacy);
  // 中点 = mix(black, white, 0.5) = #808080
  assert.equal(p.gradMid, "#808080");
  assert.equal(p.compAlpha, DEFAULTS.compAlpha);
  assert.equal(p.material, "frosted");
  // FEAT-087：旧结构无 saturation → 补 1（饱和度管线默认不干预旧外观）
  assert.equal(p.saturation, 1);
  assert.equal(p.compColor, "#123456");
  assert.equal(p.gradAngle, 90);
});

test("空对象 / null / 坏 JSON 字段 → 全量回落默认，不抛错", () => {
  for (const input of [null, undefined, {}, { compColor: "not-a-color", compAlpha: "xx", material: "wood" }]) {
    const p = normalizePrefs(input);
    assert.deepEqual(p, DEFAULTS, `input=${JSON.stringify(input)}`);
  }
});

test("越界数值被夹紧到合法区间", () => {
  const p = normalizePrefs({ compAlpha: 5, gradAngle: -100, bgOpacity: 0 });
  assert.equal(p.compAlpha, 0.9);
  assert.equal(p.gradAngle, 0);
  assert.equal(p.bgOpacity, 0.05);

  const q = normalizePrefs({ compAlpha: 0.01, bgOpacity: 3 });
  assert.equal(q.compAlpha, 0.15);
  assert.equal(q.bgOpacity, 1);
});

test("完整新结构原样保留（含 liquid 材质与三段渐变）", () => {
  const full = {
    mode: "light",
    compColor: "#abcdeg", // 非法 → 回落
    compAlpha: 0.6,
    material: "liquid",
    bgStyle: "gradient",
    bgColor: "#101010",
    gradFrom: "#ff0000",
    gradMid: "#00ff00",
    gradTo: "#0000ff",
    gradAngle: 45,
    bgOpacity: 0.8,
  };
  const p = normalizePrefs(full);
  assert.equal(p.mode, "light");
  assert.equal(p.compColor, DEFAULTS.compColor);
  assert.equal(p.compAlpha, 0.6);
  assert.equal(p.material, "liquid");
  assert.equal(p.gradMid, "#00ff00");
  assert.equal(p.gradAngle, 45);
  assert.equal(p.bgOpacity, 0.8);
});
