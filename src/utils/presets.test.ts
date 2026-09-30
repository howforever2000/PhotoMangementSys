/**
 * `utils/presets.ts` 单元测试 —— FEAT-086 预定义效果
 *
 * 钉死三条不变量：
 *   1. 每套预设的「玻璃等效底色」上派生文字对比 ≥ 4.5:1（组件可读性硬指标）；
 *   2. 每套预设的「等效页面背景」上页面文字对比 ≥ 4.5:1（onBgText 黑白兜底后）；
 *   3. 预设一律磨砂材质；matchesPreset 对自身命中、对扰动不命中。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TEXT_DARK,
  TEXT_LIGHT,
  componentTone,
  contrastRatio,
  gradientAverage,
  hexToRgb,
  mixRgb,
  onBgText,
  relLum,
  rgbToHex,
} from "./color.ts";
import { DEFAULTS, normalizePrefs, type Prefs } from "./prefs.ts";
import { PRESETS, matchesPreset, presetGradient, presetPrefs } from "./presets.ts";

function toPrefs(p: Partial<Prefs>): Prefs {
  return normalizePrefs({ ...DEFAULTS, ...p });
}

test("5 套预设的玻璃等效底色：派生文字对比 ≥ 4.5:1", () => {
  for (const preset of PRESETS) {
    const prefs = toPrefs(presetPrefs(preset));
    const bg = gradientAverage(
      hexToRgb(prefs.gradFrom),
      hexToRgb(prefs.gradMid),
      hexToRgb(prefs.gradTo),
    );
    // 玻璃等效底色 = 组件色以 compAlpha 叠在等效背景上（与 theme store 同式）
    const glass = mixRgb(bg, hexToRgb(prefs.compColor), prefs.compAlpha);
    const tone = componentTone(rgbToHex(glass));
    const ratio = contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
    assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) 玻璃文字对比 ${ratio.toFixed(2)} < 4.5`);
  }
});

test("5 套预设的等效页面背景：页面文字对比 ≥ 4.5:1（onBgText 黑白兜底）", () => {
  for (const preset of PRESETS) {
    const prefs = toPrefs(presetPrefs(preset));
    const bg = gradientAverage(
      hexToRgb(prefs.gradFrom),
      hexToRgb(prefs.gradMid),
      hexToRgb(prefs.gradTo),
    );
    for (const prefersLight of [true, false]) {
      const textHex = onBgText(bg, prefersLight);
      const ratio = contrastRatio(relLum(hexToRgb(textHex)), relLum(bg));
      assert.ok(
        ratio >= 4.5,
        `${preset.name}(${preset.id}) prefersLight=${prefersLight} 页面文字 ${textHex} 对比 ${ratio.toFixed(2)} < 4.5`,
      );
    }
  }
});

test("文字取向遵守两级规则：首选可用则用首选，否则取更高一侧", () => {
  const bg = gradientAverage(hexToRgb("#ffd7d7"), hexToRgb("#7e9963"), hexToRgb("#195e53"));
  const lum = relLum(bg);
  const lightRatio = contrastRatio(relLum(TEXT_LIGHT), lum); // prefersLight=true 的首选
  const chosen = onBgText(bg, true);
  if (lightRatio >= 4.5) {
    assert.equal(chosen, "#f5f7ff", "首选浅色 ≥4.5:1 时必须用首选");
  } else {
    // 两级规则：首选不足 → 主题深/浅档里取更高的（≥4.5 即可，不必动用纯黑纯白）；
    // 只有两档双双不足才黑白兜底（onBgText 第 c 级）。
    const darkRatio = contrastRatio(relLum(TEXT_DARK), lum);
    const chosenRatio = contrastRatio(relLum(hexToRgb(chosen)), lum);
    assert.ok(chosenRatio >= 4.5, `chosen=${chosen} ratio=${chosenRatio.toFixed(2)} < 4.5`);
    assert.ok(
      chosenRatio >= Math.max(darkRatio, lightRatio) - 0.01,
      `chosen=${chosen} 应为两档中更高者`,
    );
  }
});

test("预设一律磨砂材质 + 三段渐变角度 180", () => {
  for (const p of PRESETS) {
    assert.equal(p.material, "frosted", `${p.name} 应为磨砂`);
    assert.equal(p.angle, 180);
    assert.equal(p.colors.length, 3);
    assert.equal(presetGradient(p).startsWith("linear-gradient(180deg"), true);
  }
});

test("matchesPreset：对自身命中，对任一字段扰动不命中", () => {
  const p = PRESETS[0];
  const exact = toPrefs(presetPrefs(p));
  assert.equal(matchesPreset(p, exact), true);

  const perturbations = [
    { compAlpha: exact.compAlpha + 0.05 },
    { compColor: "#000000" },
    { gradMid: "#123456" },
    { bgStyle: "color" as const },
  ];
  for (const patch of perturbations) {
    assert.equal(matchesPreset(p, { ...exact, ...patch }), false, `patch=${JSON.stringify(patch)}`);
  }
});
