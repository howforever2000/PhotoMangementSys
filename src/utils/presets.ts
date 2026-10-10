/**
 * 预定义效果库（FEAT-086，素化后扩为两组）
 *
 * 两组并存：
 *   - PRESETS（经典渐变 5 套）：色值取自设计图，**一字未改**。它们是多色相的，
 *     用来留住「设计图原貌」这条线索，也是老用户 localStorage 里可能正命中的那套。
 *   - MUTED_PRESETS（素色系 5 套，Q6=C 暖 3 冷 2）：为「更素但不失去高级感」而加。
 *
 * 素色系的设计口径（三条，全部由 presets.test.ts 断言，不靠肉眼）：
 *   ① **同色相三段**：一套预设只用**一个色相**在明度轴上拉开（上 L≈0.92 → 下 L≈0.27）。
 *      经典组是三色相（粉→橄榄绿→墨绿），一屏三个色相必然"艳"；
 *      单色相渐变的"素"来自色相数少，不是来自把饱和度调低（那样只是变灰、变脏）。
 *   ② **饱和度 ≤ 0.36**：素色系的上限。经典组里 #d37aa0（S≈0.55）、#6ab9b3（S≈0.5）
 *      这类中高饱和色正是"艳"的来源。
 *   ③ **组件色是真中性灰（S=0，r=g=b）**：卡片底色不带任何色相，层次完全由明度 L
 *      承担，于是背景那一个色相不会被卡片"再添一个"。
 *
 * 每套联动：三段渐变 + 组件色调 + 组件玻璃透明度 + 磨砂材质（预设一律磨砂）。
 * 派生文字/链接对比度由 presets.test.ts 断言 ≥ 4.5:1。
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

/** 分组 id：主题弹窗据此渲染两个分组标题（C10） */
export type PresetGroup = "classic" | "muted";

/** 经典渐变组：设计图原貌，多色相，色值自 FEAT-086 起未变 */
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

/**
 * 素色系组（Q6=C：暖 3 套 + 冷 2 套）。
 *
 * 色值由 HSL 直接构造（同色相 + 定 S + 定 L 三档），不是手调 RGB ——
 * 这样「同色相」这条不变量在构造上就成立，不会因为手调跑偏。
 *   暖三套：h = 38（米金，延续落日金主心骨）/ 24（赭）/ 34（琥珀褐）
 *   冷两套：h = 205（墨青）/ 222（石板灰蓝）
 *
 * 组件色是**真中性灰**（r=g=b，S=0）：暖三套共用 #ededed、冷两套共用 #ececec。
 * 最初给每套配了带色相的米白（#efece6 S=0.22 等），实测那等于给背景的色相
 * 「再添一个」—— 卡片底自己也带了暖/冷，卡面与背景的关系立刻变成两个色相对话，
 * 这正是要消掉的东西。现在层次完全由明度 L 承担，色相只有背景那一个。
 *
 * 组件透明度统一 0.62（高于经典组的 0.54~0.56）：背景明暗跨度更大（L 跨度 ≥0.62），
 * 需要更强的填充把三条色带收敛，否则最差带上的文字会掉到 4.5:1 以下。
 */
export const MUTED_PRESETS: Preset[] = [
  {
    id: "linen-warm",
    name: "亚麻暖灰",
    colors: ["#f1eee9", "#c6bba9", "#5d503c"],
    angle: 180,
    compColor: "#ededed",
    compAlpha: 0.62,
    material: "frosted",
  },
  {
    id: "clay-warm",
    name: "砂陶赭红",
    colors: ["#f0e9e5", "#c2a18e", "#634536"],
    angle: 180,
    compColor: "#ededed",
    compAlpha: 0.62,
    material: "frosted",
  },
  {
    id: "amber-warm",
    name: "琥珀深褐",
    colors: ["#efe9e1", "#bfa07d", "#5c452e"],
    angle: 180,
    compColor: "#ededed",
    compAlpha: 0.62,
    material: "frosted",
  },
  {
    id: "ink-cool",
    name: "墨青冷灰",
    colors: ["#e9eef1", "#92acbf", "#2c3e4e"],
    angle: 180,
    compColor: "#ececec",
    compAlpha: 0.62,
    material: "frosted",
  },
  {
    id: "slate-cool",
    name: "石板灰蓝",
    colors: ["#eaecf1", "#9ba7bf", "#343d55"],
    angle: 180,
    compColor: "#ececec",
    compAlpha: 0.62,
    material: "frosted",
  },
];

/**
 * 弹窗分组视图：一组经典 + 一组素色。
 * 由 ThemeDialog 直接渲染（C10 的分组排版），避免组件里再写一遍两张卡的循环。
 */
export const PRESET_GROUPS: { group: PresetGroup; title: string; note: string; items: Preset[] }[] = [
  {
    group: "classic",
    title: "经典渐变",
    note: "设计图原貌 · 多色相",
    items: PRESETS,
  },
  {
    group: "muted",
    title: "素色系",
    note: "暖 3 套 · 冷 2 套 · 每套单一色相",
    items: MUTED_PRESETS,
  },
];

/** 全部预设（测试与命中判定用：跨组命中才算选中） */
export const ALL_PRESETS: Preset[] = [...PRESETS, ...MUTED_PRESETS];

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

/**
 * 命中判定**只读这 6 个字段**，所以入参类型是 Pick<Prefs, ...> 而非整个 Prefs。
 *
 * 为什么收窄：预设命中与否，与饱和度/色温/材质/背景模式都无关。
 * 若要求传整个 Prefs，调用方就得把 store 的全部状态摊平传进来 ——
 * 一旦某个字段从 store 里消失（色温模块停用就发生过），
 * 类型就会因为「少一个不相关的字段」而报错，把无关的耦合暴露成编译错误。
 */
export type PresetMatchFields = Pick<
  Prefs,
  "bgStyle" | "gradFrom" | "gradMid" | "gradTo" | "compColor" | "compAlpha"
>;

/** 当前偏好是否命中该预设（决定选中态高亮） */
export function matchesPreset(p: Preset, prefs: PresetMatchFields): boolean {
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
