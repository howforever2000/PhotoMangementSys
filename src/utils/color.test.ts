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
  LINK_GOLD_HUE,
  LINK_GOLD_LIGHT,
  LINK_GOLD_SAT,
  TEMP_COOL,
  TEMP_WARM,
  TEXT_DARK,
  TEXT_LIGHT,
  applyTemperature,
  componentTone,
  contrastRatio,
  glassBandContrast,
  hexToRgb,
  hexToRgba,
  hslAtTargetLum,
  hslToRgb,
  isDarkText,
  linkOnGlass,
  mixRgb,
  normalizeHex,
  relLum,
  rgbToHsl,
  saturateColor,
  type Rgb,
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

/* ---------- FEAT-087：全局饱和度管线 ---------- */

test("rgbToHsl ↔ hslToRgb：往返一致", () => {
  for (const hex of ["#396cd8", "#16443a", "#d13438", "#ffffff", "#000000", "#7e9963"]) {
    const [h, s, l] = rgbToHsl(hexToRgb(hex));
    const back = hslToRgb(h, s, l);
    const orig = hexToRgb(hex);
    for (let i = 0; i < 3; i += 1) {
      assert.ok(Math.abs(back[i] - orig[i]) <= 1, `${hex} 往返偏差过大: ${back} vs ${orig}`);
    }
  }
});

test("saturateColor：factor=1 原样返回（默认外观零变化）", () => {
  for (const hex of ["#396cd8", "#16443a", "#eef2ff"]) {
    assert.equal(saturateColor(hex, 1), normalizeHex(hex));
  }
});

test("saturateColor：降饱和单调递减、提饱和单调递增（以 HSL 饱和度计）", () => {
  const base = "#396cd8";
  const s0 = rgbToHsl(hexToRgb(base))[1];
  const lo = rgbToHsl(hexToRgb(saturateColor(base, 0.5)))[1];
  const hi = rgbToHsl(hexToRgb(saturateColor(base, 1.6)))[1];
  assert.ok(lo < s0 && s0 < hi, `期望 ${lo} < ${s0} < ${hi}`);
  assert.ok(Math.abs(lo - s0 * 0.5) < 0.02, "降饱和应接近线性");
});

test("saturateColor：色相基本保持，**WCAG 亮度锁定**（对比度不随滑块漂移）", () => {
  for (const hex of ["#396cd8", "#d13438", "#16a34a", "#16443a", "#4ade80"]) {
    const [h0] = rgbToHsl(hexToRgb(hex));
    const l0 = relLum(hexToRgb(hex));
    for (const f of [0.4, 0.7, 1.3, 2]) {
      const out = hexToRgb(saturateColor(hex, f));
      const [h1] = rgbToHsl(out);
      const dh = Math.min(Math.abs(h1 - h0), 360 - Math.abs(h1 - h0));
      assert.ok(dh < 4 || rgbToHsl(out)[1] === 0, `${hex}@${f} 色相漂移 ${dh}`);
      /* 容差 0.005：8bit 栅格在“高饱和亮色”（如 #4ade80 推到全饱和）上
         相邻可选色的 WCAG 亮度步长就能到 ~0.0024，已是该网格的极限；
         真正要钉死的不变量是对比度（见下一条测试）。 */
      assert.ok(
        Math.abs(relLum(out) - l0) < 0.005,
        `${hex}@${f} WCAG 亮度漂移 ${relLum(out) - l0}`,
      );
    }
  }
});

test("saturateColor：白字实底按钮的对比度全档恒定（回归扫描发现的问题）", () => {
  for (const base of ["#396cd8", "#d13438", "#15803d", "#b45309"]) {
    const r0 = contrastRatio(relLum([255, 255, 255]), relLum(hexToRgb(base)));
    for (const f of [0.4, 0.6, 1.5, 2]) {
      const r = contrastRatio(relLum([255, 255, 255]), relLum(hexToRgb(saturateColor(base, f))));
      assert.ok(Math.abs(r - r0) < 0.05, `${base}@${f}: ${r0} → ${r}`);
    }
  }
});

test("saturateColor：灰度色（S=0）任何系数下仍是灰度", () => {
  for (const f of [0.4, 2]) {
    const [r, g, b] = hexToRgb(saturateColor("#808080", f));
    assert.ok(Math.abs(r - g) < 2 && Math.abs(g - b) < 2, `灰度被染上颜色: ${r},${g},${b}`);
  }
});
/* ---------- Q3：色温管线（applyTemperature） ---------- */

test("hslAtTargetLum：求出的色 WCAG 亮度落在目标附近", () => {
  for (const [h, s, target] of [
    [38, 0.77, 0.35],
    [210, 0.4, 0.12],
    [300, 0.9, 0.6],
  ] as const) {
    const got = relLum(hslAtTargetLum(h, s, target));
    assert.ok(Math.abs(got - target) < 0.003, `h=${h} s=${s} 目标 ${target} 实得 ${got}`);
  }
});

test("applyTemperature：t=0 原样返回（老用户零位移，外观逐像素不变）", () => {
  for (const hex of ["#e8a33d", "#2b2140", "#f1eee9", "#2c3e4e"]) {
    assert.equal(applyTemperature(hex, 0), normalizeHex(hex));
  }
});

/* 色温模块已停用（Q3）：theme store 的 temp() 恒以 t=0 调用，
   所以「t=0 是严格恒等」这条性质从「设计意图」升级成了「停用正确性的依赖」——
   它保证停用后九个接入点逐像素零变化。这条测试是停用状态的安全网。 */
test("色温停用：applyTemperature(x, 0) 对全站实际用到的每个色值都严格恒等", () => {
  const IN_USE = [
    // 背景三色标 / 组件色调
    "#f1eee9", "#c6bba9", "#5d503c", "#ededed", "#2a2630", "#1a1428", "#3b2b52", "#120e1c",
    // 品牌金与 candy 六色（applyAccentVars 里走 temp() 的那批）
    "#e8a33d", "#f2b355", "#ffd9a0",
    "#fffcbd", "#f075c7", "#65d5f9", "#fb6d9b", "#505fdd", "#3a36e4",
    // 经典组预设色（老用户 localStorage 可能正命中它们）
    "#ffd7d7", "#7e9963", "#195e53", "#d37aa0", "#3f507d", "#6ab9b3",
  ];
  for (const hex of IN_USE) {
    assert.equal(applyTemperature(hex, 0), normalizeHex(hex), `${hex} 未保持恒等（停用会引入视觉变化）`);
  }
});

test("applyTemperature：暖推与冷推方向相反，且**WCAG 亮度锁定**", () => {
  for (const hex of ["#e8a33d", "#7e9963", "#92acbf", "#505fdd"]) {
    const [h0, s0] = rgbToHsl(hexToRgb(hex));
    const l0 = relLum(hexToRgb(hex));
    const warmH = rgbToHsl(hexToRgb(TEMP_WARM))[0];
    const coolH = rgbToHsl(hexToRgb(TEMP_COOL))[0];
    const toWarm = rgbToHsl(hexToRgb(applyTemperature(hex, 1)))[0];
    const toCool = rgbToHsl(hexToRgb(applyTemperature(hex, -1)))[0];
    const arc = (a: number, b: number) => {
      const d = Math.abs(a - b) % 360;
      return Math.min(d, 360 - d);
    };
    // 判据是「朝目标锚靠拢」而非「离另一个锚更远」——原色可能本就贴近某端
    // （如 #e8a33d 的色相 37 就已在暖锚 38 上），此时位移本就趋近 0。
    // 容差 1° 给色相量化留余量。
    assert.ok(
      arc(toWarm, warmH) <= arc(h0, warmH) + 1,
      `${hex} 暖推应朝暖锚靠: ${h0.toFixed(1)}→${toWarm.toFixed(1)}（锚 ${warmH}）`,
    );
    assert.ok(
      arc(toCool, coolH) <= arc(h0, coolH) + 1,
      `${hex} 冷推应朝冷锚靠: ${h0.toFixed(1)}→${toCool.toFixed(1)}（锚 ${coolH}）`,
    );
    assert.ok(arc(toWarm, toCool) > 1, `${hex} 暖/冷两端必须真的分开`);
    // 锁亮度是硬不变量：没有它，色温滑块会顺带改对比度
    for (const t of [-1, -0.5, 0.5, 1]) {
      const out = hexToRgb(applyTemperature(hex, t));
      assert.ok(Math.abs(relLum(out) - l0) < 0.005, `${hex}@${t} 亮度漂移 ${relLum(out) - l0}`);
    }
    // 饱和度只朝锚点靠拢 60%，不能被染色相推成彩色
    const sWarm = rgbToHsl(hexToRgb(applyTemperature(hex, 1)))[1];
    assert.ok(sWarm <= s0 + (rgbToHsl(hexToRgb(TEMP_WARM))[1] - s0) * 0.6 + 0.01);
  }
});

test("applyTemperature：灰度色（S=0）不被染上颜色；越界 t 被夹到 ±1", () => {
  for (const t of [-1, 1, -99, 99, NaN]) {
    const [r, g, b] = hexToRgb(applyTemperature("#808080", t));
    assert.ok(Math.abs(r - g) < 2 && Math.abs(g - b) < 2, `灰度被染色: ${r},${g},${b}`);
  }
  assert.equal(applyTemperature("#e8a33d", 5), applyTemperature("#e8a33d", 1));
  assert.equal(applyTemperature("#e8a33d", -5), applyTemperature("#e8a33d", -1));
  assert.equal(applyTemperature("#e8a33d", NaN), "#e8a33d");
});

/* ---------- Q5=A：玻璃上的链接色（linkOnGlass） ---------- */

test("linkOnGlass：深玻璃沿用浅金档（默认外观观感零变化）", () => {
  assert.equal(linkOnGlass("#31253f"), LINK_GOLD_LIGHT);
  assert.equal(linkOnGlass("#251f3f"), LINK_GOLD_LIGHT);
});

test("linkOnGlass：浅玻璃现算深金，且**始终 ≥4.5:1**（修复前只有 2.20~2.64:1）", () => {
  for (const glass of ["#c1cebe", "#dec9d2", "#bac7cc", "#c5c6d6", "#bcd6ce", "#dad4cb", "#cacfd9"]) {
    const link = linkOnGlass(glass);
    const ratio = contrastRatio(relLum(hexToRgb(link)), relLum(hexToRgb(glass)));
    assert.ok(ratio >= 4.5, `${glass} 链接 ${link} 仅 ${ratio.toFixed(2)}:1`);
    // 必须保住「金」这个品牌相：色相锁定在 LINK_GOLD_HUE 附近
    const [h, s] = rgbToHsl(hexToRgb(link));
    assert.ok(Math.abs(h - LINK_GOLD_HUE) <= 1, `${glass} 链接色相 ${h} 偏离金相`);
    assert.ok(s > 0.5, `${glass} 链接饱和度 ${s} 低了，不像金`);
  }
});

test("linkOnGlass：目标越高 → 链接越深（单调），且不会退回非金色", () => {
  const glass = "#dad4cb";
  const at45 = relLum(hexToRgb(linkOnGlass(glass, 4.5)));
  const at7 = relLum(hexToRgb(linkOnGlass(glass, 7)));
  assert.ok(at7 < at45, `target=7 应更深: ${at7} vs ${at45}`);
  assert.ok(contrastRatio(relLum(hexToRgb(linkOnGlass(glass, 7))), relLum(hexToRgb(glass))) >= 7);
});

/* ---------- FEAT-094 glassBandContrast ---------- */

test("FEAT-094 glassBandContrast：深色背景 α=0 仍安全，浅顶带渐变则必须靠填充兑底", () => {
  // ① 新默认外观（启动封面 55% + 遮罩）三带都很暗 —— α 拉到 0 也能达标
  const cover = ["#69515a", "#4b3f4e", "#1b1d43"].map(hexToRgb);
  const dark = glassBandContrast(cover, hexToRgb("#4b3f4e"), "#2b2140", 0);
  assert.ok(dark.worst >= 6, `深色背景 α=0 应仍有足够对比，实测 ${dark.worst.toFixed(2)}:1`);

  // ② 预设「粉绿晨光」（顶带亮 #ffd7d7 / 底带暗 #195e53）：α=0 时文字只有一个色，
  //    均值推出墨字 → 底带上就崩了；把填充加回来才收敛。这就是「透明度下限」的真实理由。
  const bands: Rgb[] = ["#ffd7d7", "#7e9963", "#195e53"].map(hexToRgb);
  const avg: Rgb = [
    Math.round((bands[0][0] + 2 * bands[1][0] + bands[2][0]) / 4),
    Math.round((bands[0][1] + 2 * bands[1][1] + bands[2][1]) / 4),
    Math.round((bands[0][2] + 2 * bands[1][2] + bands[2][2]) / 4),
  ];
  const zero = glassBandContrast(bands, avg, "#f2f9f4", 0);
  const mid = glassBandContrast(bands, avg, "#f2f9f4", 0.3);
  const full = glassBandContrast(bands, avg, "#f2f9f4", 0.6);
  assert.ok(zero.worst < 3, `α=0 应暴露明暗差，实测 ${zero.worst.toFixed(2)}:1`);
  assert.ok(mid.worst > zero.worst, "α 提高应单调收敛（0.3 > 0）");
  assert.ok(full.worst >= mid.worst, "α 提高应单调收敛（0.6 > 0.3）");
  assert.ok(full.worst >= 4.5, `设计值附近应达标，实测 ${full.worst.toFixed(2)}:1`);
  assert.equal(zero.text, "#1f2733", "文字色应仍由均值玻璃决定（与 componentTone 同源）");
});
