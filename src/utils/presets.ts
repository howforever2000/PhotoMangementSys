/**
 * 预定义效果库（FEAT-086）
 *
 * 5 套三段渐变背景，色值取自设计图（高级感渐变色 · 鲁森视觉）。
 * 每套联动：三段渐变 + 配套组件色调 + 组件玻璃透明度 + 磨砂材质（预设一律磨砂）。
 *
 * 组件色设计原则（按背景合理设计）：
 *   - 渐变均为「上浅下深」，等效背景落在中调 → 组件一律取该套色系**提亮的浅色**，
 *     玻璃 = mix(等效背景, 组件色, compAlpha) 后为浅底，文字自动取深色档；
 *   - compAlpha 按背景明度微调（背景越浅越高，防止玻璃被背景吃掉）；
 *   - 派生文字对比度由 presets.test.ts 断言 ≥ 4.5:1。
 */
import type { Prefs } from "./prefs.ts";

export interface Preset {
  id: string;
  name: string;
  /** 三段渐变：[起, 中, 止] */
  colors: [string, string, string];
  /** 渐变角度：180 = 上浅下深（与设计图一致） */
  angle: number;
  /** 配套组件色调（浅色系提亮） */
  compColor: string;
  /** 配套组件玻璃透明度 */
  compAlpha: number;
  /** 预设材质：一律磨砂 */
  material: "frosted";
}

export const PRESETS: Preset[] = [
  {
    id: "sage-rose",
    name: "粉绿晨光",
    colors: ["#ffd7d7", "#7e9963", "#195e53"],
    angle: 180,
    compColor: "#f2f9f4",
    compAlpha: 0.55,
    material: "frosted",
  },
  {
    id: "cream-violet",
    name: "奶油莓紫",
    colors: ["#fbfecb", "#d37aa0", "#3f507d"],
    angle: 180,
    compColor: "#fcf5f8",
    compAlpha: 0.56,
    material: "frosted",
  },
  {
    id: "mint-indigo",
    name: "薄荷靛蓝",
    colors: ["#ceffd1", "#638196", "#3a2e67"],
    angle: 180,
    compColor: "#f4f8f6",
    compAlpha: 0.55,
    material: "frosted",
  },
  {
    id: "butter-plum",
    name: "黄油蓝紫",
    colors: ["#fcfecb", "#687fb9", "#5d3263"],
    angle: 180,
    compColor: "#f7f8fd",
    compAlpha: 0.54,
    material: "frosted",
  },
  {
    id: "lemon-teal",
    name: "柠檬青蓝",
    colors: ["#f6ffb1", "#6ab9b3", "#10324a"],
    angle: 180,
    compColor: "#f4fbfa",
    compAlpha: 0.55,
    material: "frosted",
  },
];

/** 预设 → 待写入的偏好片段（bgStyle 固定为渐变） */
export function presetPrefs(p: Preset): Partial<Prefs> {
  return {
    bgStyle: "gradient",
    gradFrom: p.colors[0],
    gradMid: p.colors[1],
    gradTo: p.colors[2],
    gradAngle: p.angle,
    compColor: p.compColor,
    compAlpha: p.compAlpha,
    material: p.material,
  };
}

/** 当前偏好是否命中该预设（决定选中态高亮） */
export function matchesPreset(p: Preset, prefs: Prefs): boolean {
  return (
    prefs.bgStyle === "gradient" &&
    prefs.gradFrom === p.colors[0] &&
    prefs.gradMid === p.colors[1] &&
    prefs.gradTo === p.colors[2] &&
    prefs.compColor === p.compColor &&
    Math.abs(prefs.compAlpha - p.compAlpha) < 0.001
  );
}

/** 预设卡片上的渐变预览 CSS（默认 180° 竖向；传 angle 则同步当前背景角度） */
export function presetGradient(p: Preset, angle = p.angle): string {
  return `linear-gradient(${angle}deg, ${p.colors[0]}, ${p.colors[1]}, ${p.colors[2]})`;
}
