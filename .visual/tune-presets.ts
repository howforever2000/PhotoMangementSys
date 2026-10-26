/**
 * 预设调参台：node --experimental-strip-types .visual/tune-presets.ts
 *
 * 用途：改预设色值前先跑一遍，把 design 的三条硬约束一次算清 ——
 *   ① 单套三段渐变单一色相（≤14°）  ② 浅色顶段 L ≥0.88、跨度 ≥0.10
 *   ③ 组件色近中性（RGB 通道极差 ≤12）
 * 外加两条可读性读数：
 *   glassTxt = 预设派生出的玻璃等效底上的文字对比（≥4.5）
 *   fg2 / fg3 = tokens.css 的次级/弱化文字压在**该套最暗端**上的对比
 *               （浅色档的标定基准就是"六套里最暗的那一格"，改了色值必须回看这里）
 *
 * 输出与 src/utils/presets.ts 里的注释表格应当一致；不一致说明有套色值漂了。
 */
import { hexToRgb, rgbToHsl, relLum, contrastRatio, mixRgb, gradientAverage, rgbToHex, componentTone } from "../src/utils/color.ts";

type P = { id: string; name: string; colors: [string, string, string]; compColor: string; compAlpha: number };

const LIGHT: P[] = [
  { id: "linen-light", name: "亚麻暖灰", colors: ["#efe8dc", "#e0d5c4", "#c5b6a1"], compColor: "#f2efea", compAlpha: 0.72 },
  { id: "paper-light", name: "纸白中性", colors: ["#f4f4f2", "#e5e4e0", "#cecdc8"], compColor: "#f4f4f4", compAlpha: 0.72 },
  { id: "mist-light", name: "雾光冷灰", colors: ["#e7edf4", "#d2dce7", "#b2c0d0"], compColor: "#eff1f4", compAlpha: 0.72 },
  { id: "celadon-light", name: "青瓷浅绿", colors: ["#e7f0eb", "#cde1d7", "#a4c4b4"], compColor: "#f0f4f1", compAlpha: 0.72 },
  { id: "clay-light", name: "陶土暖砂", colors: ["#f6e9da", "#eacfb3", "#d5af8a"], compColor: "#f5f0ea", compAlpha: 0.72 },
  { id: "lilac-light", name: "菊芋淡紫", colors: ["#f0e9f6", "#ded1ed", "#bfaad7"], compColor: "#f3f0f6", compAlpha: 0.72 },
];

const DARK: P[] = [
  { id: "sunset-dusk", name: "落日紫夜", colors: ["#3b2b52", "#271d37", "#120e1c"], compColor: "#2a2630", compAlpha: 0.62 },
  { id: "ink-slate", name: "墨色石板", colors: ["#2c3440", "#1d232d", "#0e1116"], compColor: "#26282c", compAlpha: 0.62 },
  { id: "ember-warm", name: "余烬暖褐", colors: ["#3a2a22", "#261a16", "#120c0a"], compColor: "#2b2521", compAlpha: 0.62 },
  { id: "pine-night", name: "松绿混色", colors: ["#162e25", "#0f1f1a", "#080f0c"], compColor: "#232a26", compAlpha: 0.62 },
];

const hueArc = (a: number, b: number) => { const d = Math.abs(a - b) % 360; return Math.min(d, 360 - d); };
const spread = (h: string) => { const c = hexToRgb(h); return Math.max(...c) - Math.min(...c); };

for (const p of [...DARK, ...LIGHT]) {
  const rgb = p.colors.map((c) => hexToRgb(c));
  const hsl = rgb.map((c) => rgbToHsl(c));
  const hues = hsl.map((h) => h[0]);
  const ls = hsl.map((h) => h[2]);
  const ss = hsl.map((h) => h[1]);
  const bg = gradientAverage(rgb[0], rgb[1], rgb[2]);
  const glass = mixRgb(bg, hexToRgb(p.compColor), p.compAlpha);
  const tone = componentTone(rgbToHex(glass));
  const glassTxt = contrastRatio(relLum(hexToRgb(tone.text)), relLum(glass));
  // 文字压在该套最暗端上的对比（深色组看最亮端 —— 深底上的白字是被"最亮处"吃掉的）
  const worst = p.id.endsWith("-light") ? rgb[2] : rgb[0];
  const ink = p.id.endsWith("-light") ? [22, 21, 26] : [242, 240, 244];
  const fg2 = contrastRatio(relLum(mixRgb(worst, ink, 0.70)), relLum(worst));
  const fg3 = contrastRatio(relLum(mixRgb(worst, ink, 0.60)), relLum(worst));
  const hueSpan = Math.max(...hues.map((h) => hueArc(h, hues[0])));
  console.log(
    `${p.id.padEnd(14)} hueSpan=${hueSpan.toFixed(0).padStart(3)}  L=[${ls.map((l) => l.toFixed(2)).join(",")}]  span=${(ls[0] - ls[2]).toFixed(2)}  S=[${ss.map((s) => s.toFixed(2)).join(",")}]  compSpread=${String(spread(p.compColor)).padStart(2)}  glassTxt=${glassTxt.toFixed(2)}  fg2=${fg2.toFixed(2)}  fg3=${fg3.toFixed(2)}`,
  );
}
