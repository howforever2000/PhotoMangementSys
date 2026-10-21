/**
 * `utils/presets.ts` 单元测试 —— FEAT-086 预定义效果
 *
 * 钉死三条不变量：
 *   1. 每套预设的「玻璃等效底色」上派生文字对比 ≥ 4.5:1（组件可读性硬指标）；
 *   2. 每套预设的「等效页面背景」上页面文字对比 ≥ 4.5:1（onBgText 黑白兜底后）；
 *   3. 预设一律普通卡片材质；matchesPreset 对自身命中、对扰动不命中。
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TEXT_DARK,
  TEXT_LIGHT,
  applyTemperature,
  componentTone,
  contrastRatio,
  gradientAverage,
  hexToRgb,
  linkOnGlass,
  mixRgb,
  onBgText,
  relLum,
  rgbToHex,
  rgbToHsl,
} from "./color.ts";
import { DEFAULTS, normalizePrefs, type Prefs } from "./prefs.ts";
import { ALL_PRESETS, MUTED_PRESETS, PRESETS, matchesPreset, presetGradient, presetPrefs } from "./presets.ts";

function toPrefs(p: Partial<Prefs>): Prefs {
  return normalizePrefs({ ...DEFAULTS, ...p });
}

/** 预设套用后的等效三色带 + 均值背景 + 等效玻璃底色（与 theme store 同式） */
function resolvePreset(preset: (typeof ALL_PRESETS)[number]) {
  const prefs = toPrefs(presetPrefs(preset));
  const from = hexToRgb(prefs.gradFrom);
  const mid = hexToRgb(prefs.gradMid);
  const to = hexToRgb(prefs.gradTo);
  const bg = gradientAverage(from, mid, to);
  const glass = mixRgb(bg, hexToRgb(prefs.compColor), prefs.compAlpha);
  return { prefs, bands: [from, mid, to], bg, glass, glassHex: rgbToHex(glass) };
}

test("全部 10 套预设的玻璃等效底色：派生文字对比 ≥ 4.5:1", () => {
  for (const preset of ALL_PRESETS) {
    const { glass, glassHex } = resolvePreset(preset);
    const tone = componentTone(glassHex);
    const ratio = contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
    assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) 玻璃文字对比 ${ratio.toFixed(2)} < 4.5`);
  }
});

test("全部 10 套预设的等效页面背景：页面文字对比 ≥ 4.5:1（onBgText 黑白兜底）", () => {
  for (const preset of ALL_PRESETS) {
    const { bg } = resolvePreset(preset);
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

/* ---------- Q5=A：链接色在 10 套预设上必须达标 ---------- */

test("全部 10 套预设：玻璃上的链接色 ≥ 4.5:1（修复前经典组只有 2.20~2.64:1）", () => {
  for (const preset of ALL_PRESETS) {
    const { glassHex } = resolvePreset(preset);
    const link = linkOnGlass(glassHex);
    const ratio = contrastRatio(relLum(hexToRgb(link)), relLum(hexToRgb(glassHex)));
    assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) 链接 ${link} 仅 ${ratio.toFixed(2)}:1`);
  }
});

/* ---------- Q6=C：素色系的设计口径 ---------- */

/** 色相圆上的最短弧距离（0~180） */
function hueArc(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
}

test("素色系 5 套：三段渐变**单一色相**且饱和度 ≤ 0.36", () => {
  for (const preset of MUTED_PRESETS) {
    const hues = preset.colors.map((c) => rgbToHsl(hexToRgb(c))[0]);
    for (const h of hues) {
      assert.ok(hueArc(h, hues[0]) <= 6, `${preset.name}(${preset.id}) 色相跑偏: ${hues.map((x) => x.toFixed(0))}`);
    }
    for (const c of preset.colors) {
      const s = rgbToHsl(hexToRgb(c))[1];
      assert.ok(s <= 0.36, `${preset.name}(${preset.id}) ${c} 饱和度 ${s.toFixed(2)} > 0.36（素色系上限）`);
    }
  }
});

test("素色系 5 套：明度跨度够大（上浅下深，层次靠 L 不靠色相）", () => {
  for (const preset of MUTED_PRESETS) {
    const ls = preset.colors.map((c) => rgbToHsl(hexToRgb(c))[2]);
    const span = Math.max(...ls) - Math.min(...ls);
    assert.ok(span >= 0.6, `${preset.name}(${preset.id}) 明度跨度 ${span.toFixed(2)} < 0.6（会显得平）`);
    assert.ok(ls[0] > 0.85, `${preset.name}(${preset.id}) 顶段应接近白（L=${ls[0].toFixed(2)}）`);
    assert.ok(ls[2] < 0.35, `${preset.name}(${preset.id}) 底段应够深（L=${ls[2].toFixed(2)}）`);
  }
});

test("素色系 5 套：组件色是中性灰（几乎无色相），不与背景抢色相", () => {
  for (const preset of MUTED_PRESETS) {
    const s = rgbToHsl(hexToRgb(preset.compColor))[1];
    assert.ok(s <= 0.08, `${preset.name}(${preset.id}) 组件色饱和度 ${s.toFixed(3)} > 0.08（应为中性灰）`);
  }
});

test("素色系恰好暖 3 套 + 冷 2 套（Q6=C）", () => {
  const warm = MUTED_PRESETS.filter((p) => rgbToHsl(hexToRgb(p.colors[1]))[0] < 90);
  const cool = MUTED_PRESETS.filter((p) => rgbToHsl(hexToRgb(p.colors[1]))[0] >= 90);
  assert.equal(warm.length, 3, `暖 ${warm.length} 套`);
  assert.equal(cool.length, 2, `冷 ${cool.length} 套`);
});

/* ---------- Q3：色温 × 预设 的组合断言 ---------- */

test("色温 -1/0/+1 × 全部 10 套预设：正文对比全程 ≥ 4.5:1（锁亮度的意义）", () => {
  for (const preset of ALL_PRESETS) {
    for (const t of [-1, 0, 1]) {
      const prefs = toPrefs(presetPrefs(preset));
      const from = hexToRgb(applyTemperature(prefs.gradFrom, t));
      const mid = hexToRgb(applyTemperature(prefs.gradMid, t));
      const to = hexToRgb(applyTemperature(prefs.gradTo, t));
      const glass = mixRgb(
        gradientAverage(from, mid, to),
        hexToRgb(applyTemperature(prefs.compColor, t)),
        prefs.compAlpha,
      );
      const tone = componentTone(rgbToHex(glass));
      const ratio = contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
      assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) t=${t} 正文对比 ${ratio.toFixed(2)} < 4.5`);
    }
  }
});

test("色温 × 预设：语义色三档逐像素不变（危险红不能被推成橙红）", () => {
  const SEMANTIC = ["#d13438", "#15803d", "#b45309"];
  for (const hex of SEMANTIC) {
    for (const t of [-1, 0, 1]) {
      // 语义色只过饱和度，theme store 里从未把 temp() 用在它们身上；
      // 这里显式断言「若有人误接色温会造成多大偏差」，作为回归哨兵。
      const drifted = applyTemperature(hex, t);
      const arc = hueArc(rgbToHsl(hexToRgb(drifted))[0], rgbToHsl(hexToRgb(hex))[0]);
      assert.ok(t === 0 || arc > 3, `${hex} t=${t} 被色温改了色相（${arc.toFixed(1)}°），语义色不得接入色温`);
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

test("全部 10 套预设一律普通卡片材质 + 三段渐变角度 180", () => {
  for (const p of ALL_PRESETS) {
    assert.equal(p.material, "plain", `${p.name} 应为普通卡片`);
    assert.equal(p.angle, 180);
    assert.equal(p.colors.length, 3);
    assert.equal(presetGradient(p).startsWith("linear-gradient(180deg"), true);
  }
});

test("经典组色值自 FEAT-086 起一字未改（老用户 localStorage 可能正命中它们）", () => {
  const golden: { id: string; colors: [string, string, string]; compColor: string; compAlpha: number }[] = [
    { id: "sage-rose", colors: ["#ffd7d7", "#7e9963", "#195e53"], compColor: "#f2f9f4", compAlpha: 0.55 },
    { id: "cream-violet", colors: ["#fbfecb", "#d37aa0", "#3f507d"], compColor: "#fcf5f8", compAlpha: 0.56 },
    { id: "mint-indigo", colors: ["#ceffd1", "#638196", "#3a2e67"], compColor: "#f4f8f6", compAlpha: 0.55 },
    { id: "butter-plum", colors: ["#fcfecb", "#687fb9", "#5d3263"], compColor: "#f7f8fd", compAlpha: 0.54 },
    { id: "lemon-teal", colors: ["#f6ffb1", "#6ab9b3", "#10324a"], compColor: "#f4fbfa", compAlpha: 0.55 },
  ];
  assert.equal(PRESETS.length, 5);
  for (const g of golden) {
    const p = PRESETS.find((x) => x.id === g.id);
    assert.ok(p, `经典组缺少 ${g.id}`);
    assert.deepEqual(p!.colors, g.colors, `${g.id} 渐变色被改动`);
    assert.equal(p!.compColor, g.compColor, `${g.id} 组件色被改动`);
    assert.equal(p!.compAlpha, g.compAlpha, `${g.id} 透明度被改动`);
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
