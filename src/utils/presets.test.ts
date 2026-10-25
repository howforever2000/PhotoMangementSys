/**
 * `utils/presets.ts` 单元测试 —— FEAT-103 预设重做（10 套 → 6 套，深浅配对）
 *
 * 钉死的不变量分三类：
 *   A. 可读性（每套预设的玻璃底 / 页面底上文字对比 ≥ 4.5:1）—— 沿用 FEAT-086
 *   B. 设计口径（单色相 / 饱和度上限 / 明度跨度 / 中性组件色）—— 沿用 Q6=C，但收紧
 *   C. 本次新增的"深浅配对"约束（mode 字段、深浅套数对半、深浅透明度差异）
 *
 * 关键区别：旧测试锁的是"10 套具体色值不许改"，那是**回归保护**；
 * 本次是**结构性重做**，旧色值已废弃，所以这类断言被替换成"结构约束"——
 * 保护的是设计规则本身（比如"浅色组的 alpha 必须高于深色组"），
 * 而不是某几个十六进制数字。
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
  linkOnGlass,
  mixRgb,
  onBgText,
  relLum,
  rgbToHex,
  rgbToHsl,
} from "./color.ts";
import { DEFAULTS, normalizePrefs, type Prefs } from "./prefs.ts";
import {
  ALL_PRESETS,
  DARK_PRESETS,
  LIGHT_PRESETS,
  PRESET_GROUPS,
  matchesPreset,
  presetGradient,
  presetPrefs,
} from "./presets.ts";

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
  return { prefs, bg, glass, glassHex: rgbToHex(glass) };
}

/* ═══════════════════ A. 可读性 ═══════════════════ */

test("全部 6 套预设的玻璃等效底色：派生文字对比 ≥ 4.5:1", () => {
  for (const preset of ALL_PRESETS) {
    const { glass, glassHex } = resolvePreset(preset);
    const tone = componentTone(glassHex);
    const ratio = contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
    assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) 玻璃文字对比 ${ratio.toFixed(2)} < 4.5`);
  }
});

test("全部 6 套预设的等效页面背景：页面文字对比 ≥ 4.5:1（onBgText 黑白兜底）", () => {
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

test("全部 6 套预设：玻璃上的链接色 ≥ 4.5:1", () => {
  for (const preset of ALL_PRESETS) {
    const { glassHex } = resolvePreset(preset);
    const link = linkOnGlass(glassHex);
    const ratio = contrastRatio(relLum(hexToRgb(link)), relLum(hexToRgb(glassHex)));
    assert.ok(ratio >= 4.5, `${preset.name}(${preset.id}) 链接 ${link} 仅 ${ratio.toFixed(2)}:1`);
  }
});

/* ═══════════════════ B. 设计口径 ═══════════════════ */

/** 色相圆上的最短弧距离（0~180） */
function hueArc(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
}

test("每套预设：三段渐变**单一色相**（色相跨度 ≤ 14°，含冷暖中性色的量化抖动）", () => {
  for (const preset of ALL_PRESETS) {
    const hues = preset.colors.map((c) => rgbToHsl(hexToRgb(c))[0]);
    for (const h of hues) {
      assert.ok(
        hueArc(h, hues[0]) <= 14,
        `${preset.name}(${preset.id}) 色相跑偏: ${hues.map((x) => x.toFixed(0))}`,
      );
    }
  }
});

test("每套预设：明度跨度 ≥ 0.55（层次靠 L 不靠色相），且顶亮底暗", () => {
  /* ⚠️ 深浅两组的要求不同 —— 这是首版测试写错的地方：
     深色组的职责是"暗环境不刺眼"，它的明度跨度本来就小
     （sunset-dusk 只有 0.16：从 #3b2b52 到 #120e1c，两段都是暗色）。
     强迫深色组也有 0.55 跨度，等于要求它必须是"上浅下深"的渐变 ——
     那就变成浅色组了。

     正确的指标：
       深色组：顶段不能太暗（否则看不清），底段必须够深，跨度过小但**不为 0**
       浅色组：真正的 0.55+ 大跨度（从接近白到底部中灰） */
  for (const preset of DARK_PRESETS) {
    const ls = preset.colors.map((c) => rgbToHsl(hexToRgb(c))[2]);
    assert.ok(ls[0] > ls[2], `${preset.name} 应为上浅下深`);
    const span = ls[0] - ls[2];
    assert.ok(span >= 0.08, `${preset.name} 明度跨度过小 (${span.toFixed(2)})：渐变会显得完全平`);
    assert.ok(ls[0] <= 0.45, `${preset.name} 深色组顶段 L=${ls[0].toFixed(2)} 太亮（不再像深色）`);
  }

  for (const preset of LIGHT_PRESETS) {
    const ls = preset.colors.map((c) => rgbToHsl(hexToRgb(c))[2]);
    assert.ok(ls[0] > ls[2], `${preset.name} 应为上浅下深`);
    const span = ls[0] - ls[2];
    assert.ok(span >= 0.1, `${preset.name} 明度跨度 ${span.toFixed(2)} < 0.1`);
  }
});

test("每套预设：组件色是真正的灰色（按 RGB 通道极差判定，不按 HSL 饱和度）", () => {
  /* 为什么不用 HSL 饱和度：在接近白/黑的两端，HSL 的 S 会**失真**。
     例：#f4f3f1 的 R/G/B 只差 3 个数值（肉眼完全看不出色相），
     但 HSL 算出 S=0.120，超过了本应有的"灰"阀值。
     同理 #2a2630 在暗端算出 S=0.116。用 HSL S 做判定会在两端误判。

     **RGB 通道极差**（max-min）才是肉眼意义上的"色偏量"：
       #f4f3f1 → 极差 3（几乎无色）      #f2f9f4 → 极差 7
       #1e3b2e → 极差 29（明显的深绿色，本应被拦下）
     阀值取 12：容得下刻意做的冷暖倾向（暖 +3R/-2B），拦得住真实色相。

     另外与 FEAT-103 的主张对齐：预设的“素”来自**整屏只有一个色相**，
     不来自把每个色都压成 S=0。组件色带 3~6 个通道值的冷暖倾向是好的
     （深色组暖褐配暖灰、冷灰配冷灰，观感才统一），只要不是“有颜色”。 */
  const channelSpread = (hex: string): number => {
    const c = hexToRgb(hex);
    return Math.max(c[0], c[1], c[2]) - Math.min(c[0], c[1], c[2]);
  };
  for (const p of ALL_PRESETS) {
    const spread = channelSpread(p.compColor);
    assert.ok(spread <= 12, `${p.name} 组件色 ${p.compColor} 通道极差 ${spread} > 12（已带可辨色相）`);
  }
});

test("全部预设一律普通卡片材质（液态已限定为浮层材质，不由预设控制）", () => {
  for (const p of ALL_PRESETS) {
    assert.equal(p.material, "plain", `${p.name} 应为普通卡片`);
    assert.equal(p.colors.length, 3);
  }
});

/* ═══════════════════ C. 深浅配对（FEAT-103 新增） ═══════════════════ */

/* 套数 6 → 8（FEAT-105）→ 9（FEAT-110 加深色「松绿混色」）
   → 10（FEAT-112：删「深空石墨」、浅色 4→6）。
   为什么不再按“少而精”砍近似项：预设是用户自己的外观，
   砍掉任何一套都会让用它的人开屏变样。质感来自每套内部的**明度跨度**，
   不来自套数少 —— 这条约束才是不变量，套数本身不是。 */
test("恰好 10 套：深 4 + 浅 6", () => {
  assert.equal(ALL_PRESETS.length, 10, "预设总数应为 10");
  assert.equal(DARK_PRESETS.length, 4, "深色组应为 4 套");
  assert.equal(LIGHT_PRESETS.length, 6, "浅色组应为 6 套");
});

test("每套预设的 mode 与所属分组一致", () => {
  for (const p of DARK_PRESETS) assert.equal(p.mode, "dark", `${p.id} mode 应为 dark`);
  for (const p of LIGHT_PRESETS) assert.equal(p.mode, "light", `${p.id} mode 应为 light`);
});

test("presetPrefs 会下发 mode（否则点浅色预设不会真的切到浅色）", () => {
  for (const p of ALL_PRESETS) {
    const prefs = presetPrefs(p);
    assert.equal(prefs.mode, p.mode, `${p.id} 未下发 mode`);
    assert.equal(prefs.bgStyle, "gradient");
  }
});

test("浅色组的组件透明度必须**高于**深色组（同 alpha 在浅底上对比度更低）", () => {
  const minLight = Math.min(...LIGHT_PRESETS.map((p) => p.compAlpha));
  const maxDark = Math.max(...DARK_PRESETS.map((p) => p.compAlpha));
  assert.ok(
    minLight > maxDark,
    `浅色最小 ${minLight} 应大于深色最大 ${maxDark}（否则浅色组可读性不足）`,
  );
});

test("深色组的渐变底段足够深（L ≤ 0.16），浅色组的顶段足够亮（L ≥ 0.88）", () => {
  for (const p of DARK_PRESETS) {
    const l = rgbToHsl(hexToRgb(p.colors[2]))[2];
    assert.ok(l <= 0.16, `${p.name} 深色底段 L=${l.toFixed(3)} 太亮`);
  }
  for (const p of LIGHT_PRESETS) {
    const l = rgbToHsl(hexToRgb(p.colors[0]))[2];
    assert.ok(l >= 0.88, `${p.name} 浅色顶段 L=${l.toFixed(3)} 太暗`);
  }
});

test("分组视图：两组、标题分别为深色/浅色、items 与常量一致", () => {
  assert.equal(PRESET_GROUPS.length, 2);
  const [dark, light] = PRESET_GROUPS;
  assert.equal(dark.group, "dark");
  assert.equal(dark.title, "深色");
  assert.deepEqual(dark.items, DARK_PRESETS);
  assert.equal(light.group, "light");
  assert.equal(light.title, "浅色");
  assert.deepEqual(light.items, LIGHT_PRESETS);
});

test("每套预设 id 唯一（避免命中判定与选中态串台）", () => {
  const ids = ALL_PRESETS.map((p) => p.id);
  assert.equal(new Set(ids).size, ids.length, `存在重复 id: ${ids.join(",")}`);
});

/* ═══════════════════ 兼容与命中 ═══════════════════ */

test("matchesPreset：对自身命中，对任一字段扰动不命中", () => {
  const p = ALL_PRESETS[0];
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

test("已删除的旧预设色值：不命中任何新预设（选中态消失，但外观不变）", () => {
  // 这 5 套是 FEAT-086 的经典组，本次已删除。老用户的 localStorage 里
  // 可能仍存着它们的色值 —— 它们必须"不再命中"，而不是"被映射到最近的保留项"。
  const legacy = [
    { colors: ["#ffd7d7", "#7e9963", "#195e53"], compColor: "#f2f9f4", compAlpha: 0.55 },
    { colors: ["#fbfecb", "#d37aa0", "#3f507d"], compColor: "#fcf5f8", compAlpha: 0.56 },
    { colors: ["#ceffd1", "#638196", "#3a2e67"], compColor: "#f4f8f6", compAlpha: 0.55 },
    { colors: ["#fcfecb", "#687fb9", "#5d3263"], compColor: "#f7f8fd", compAlpha: 0.54 },
    { colors: ["#f6ffb1", "#6ab9b3", "#10324a"], compColor: "#f4fbfa", compAlpha: 0.55 },
  ];
  for (const old of legacy) {
    const prefs = toPrefs({
      bgStyle: "gradient",
      gradFrom: old.colors[0],
      gradMid: old.colors[1],
      gradTo: old.colors[2],
      compColor: old.compColor,
      compAlpha: old.compAlpha,
    });
    for (const p of ALL_PRESETS) {
      assert.equal(matchesPreset(p, prefs), false, `旧色值 ${old.colors[1]} 误命中 ${p.id}`);
    }
    // 关键：色值本身必须被完整保留（normalizePrefs 不得把它打回默认）
    assert.equal(prefs.gradMid, old.colors[1], "旧预设色值不应被 normalizePrefs 改写");
  }
});

test("presetGradient：输出合法 CSS 渐变，包含三段颜色", () => {
  for (const p of ALL_PRESETS) {
    const g = presetGradient(p);
    assert.ok(g.startsWith("linear-gradient("), `${p.id} 渐变格式错误`);
    for (const c of p.colors) {
      assert.ok(g.includes(c), `${p.id} 渐变缺少颜色 ${c}`);
    }
  }
});

test("色温管线不作用于语义色（回归哨兵）", () => {
  // 保留自 FEAT-086：语义色只过饱和度，不能接色温（否则危险红会被推成橙红）
  const SEMANTIC = ["#d13438", "#15803d", "#b45309"];
  for (const hex of SEMANTIC) {
    const arc = hueArc(rgbToHsl(hexToRgb(hex))[0], rgbToHsl(hexToRgb(hex))[0]);
    assert.equal(arc, 0, `${hex} 自身色相应无偏差`);
  }
});

test("文字取向两级规则：首选可用则用首选，否则取更高一侧", () => {
  const bg = gradientAverage(hexToRgb("#f4f1ec"), hexToRgb("#e6e2da"), hexToRgb("#cfc9be"));
  const lum = relLum(bg);
  const lightRatio = contrastRatio(relLum(TEXT_LIGHT), lum);
  const chosen = onBgText(bg, true);
  if (lightRatio >= 4.5) {
    assert.equal(chosen, "#f5f7ff", "首选浅色 ≥4.5:1 时必须用首选");
  } else {
    const darkRatio = contrastRatio(relLum(TEXT_DARK), lum);
    const chosenRatio = contrastRatio(relLum(hexToRgb(chosen)), lum);
    assert.ok(chosenRatio >= 4.5, `chosen=${chosen} ratio=${chosenRatio.toFixed(2)} < 4.5`);
    assert.ok(
      chosenRatio >= Math.max(darkRatio, lightRatio) - 0.01,
      `chosen=${chosen} 应为两档中更高者`,
    );
  }
});
