/**
 * 预定义外观（FEAT-116 重排：每套一个"侧重"，浅色组不再互相撞车）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 本轮为什么又改（FEAT-112 的 6 套浅色里，有 3 套是"暖白"的细微变体）
 * ───────────────────────────────────────────────────────────────────────────
 * 实测（.visual/tune-presets.ts 同一套 color.ts 管线）：
 *   linen #f4f1ec / paper #faf8f5 / clay #f7f0e8 —— 三者顶段色相都在 30°±5，
 *   明度差 < 0.03。预设卡只有 96×56 的一格渐变，这点差别在弹窗里几乎读不出来，
 *   于是"浅色有六套"在用户眼里等于"浅色有三套差不多的白"。
 *
 * 现在浅色组按**两条轴**分工，任意两套至少在一条轴上明显分开：
 *   ① 色相（暖中性 / 纯中性 / 冷中性 / 绿 / 陶土 / 紫）—— 互补、不重叠
 *   ② 彩度（paper 近乎 0 → clay 0.65）—— 即使同为暖调，浓淡也一眼可分
 * 每套还带 `role`（弹窗里显示"这套适合什么"）与 `accent`（只在仪表墙生效的
 * 强调色，见 theme.ts applyWidgetAccent）。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 兼容策略（决定老用户会不会"开屏变样"）
 * ───────────────────────────────────────────────────────────────────────────
 * 被删/改的色值**仍然能被识别为自定义**：`matchesPreset` 只做字段比对，
 * 旧值不再命中任何预设 → `activePresetId` 为 null（弹窗里没有卡高亮），
 * 但**外观不变**（色值还在 localStorage 里）。绝不把旧配色映射到最近的保留项。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 三条硬约束（presets.test.ts 锁死，别绕过）
 * ───────────────────────────────────────────────────────────────────────────
 *   · 单套三段渐变**单一色相**（色相跨度 ≤ 14°，含低饱和时的量化抖动）；
 *   · 浅色顶段 L ≥ 0.88、明度跨度 ≥ 0.10；深色顶段 L ≤ 0.45、底段 L ≤ 0.16；
 *   · 组件色是"近中性"（RGB 通道极差 ≤ 12）—— 彩色只由背景与强调色承担。
 * 另外 tokens.css 的浅色 --fg-2 按**本组最暗的一端**（陶土 #ddbc9c，L≈0.72）
 * 重新标定为 .70：六套浅色里最暗的那一格上仍有 ≥4.9:1，其余更高。
 */
import type { Prefs, ThemeMode } from "./prefs.ts";

export interface Preset {
  id: string;
  name: string;
  /** 这套预设所属的深浅模式：切换时一并下发 */
  mode: ThemeMode;
  /**
   * 侧重（弹窗里显示在名称下方）：用户选预设时真正要回答的是
   * "这套适合什么场景 / 什么照片"，而不是"它是第几号预设"。
   */
  role: string;
  /** 三段渐变：[起, 中, 止] */
  colors: [string, string, string];
  /** 渐变角度：180 = 上浅下深 */
  angle: number;
  /** 配套组件色调 */
  compColor: string;
  /** 配套组件玻璃透明度 */
  compAlpha: number;
  /**
   * 预设材质：FEAT-103 起全部为 `plain`（普通卡片）。
   *
   * 为什么不留液态选项：液态玻璃在现行方案里已被限定为**浮层材质**
   * （见 design/design-system.md §3），卡片不再是玻璃。
   * 字段保留是为了不破坏 normalizePrefs 的结构兼容。
   */
  material: "plain";
  /**
   * 仪表墙 widget 的强调色对（只影响 --w-accent / --w-accent-2，
   * 不触碰全站 --c-accent）。缺省时回落到 widget.css 的落日金。
   *
   * 为什么浅色也要给：六套浅色的背景色相各不相同，如果强调色统一是金，
   * 青瓷/菊芋两套上金线会显得"外来"；给一套同族的低彩强调色，
   * 墙面的"这套预设"感才成立。彩度一律压在 0.45 以下 —— 它是强调，不是第二品牌色。
   */
  accent?: string;
  accent2?: string;
  /** 概览页叠加的三团有色光晕（只有需要"混色"的预设才给） */
  washes?: [string, string, string];
}

/** 分组 id：主题弹窗据此渲染分组标题 */
export type PresetGroup = "dark" | "light";

/**
 * 深色组（4 套）
 *
 * 色相分工：紫（品牌）/ 中性冷（专业感）/ 暖褐（亲和感）/ 松绿混色（仪表墙彩色档）。
 */
export const DARK_PRESETS: Preset[] = [
  {
    id: "sunset-dusk",
    name: "落日紫夜",
    mode: "dark",
    role: "品牌默认 · 夜间沉浸",
    // 与登录页启动封面（covers/login-sunset.jpg）配套，是产品默认外观
    colors: ["#3b2b52", "#271d37", "#120e1c"],
    angle: 165,
    compColor: "#2a2630",
    compAlpha: 0.62,
    material: "plain",
  },
  {
    id: "ink-slate",
    name: "墨色石板",
    mode: "dark",
    role: "中性冷灰 · 不干扰照片色",
    // 真中性冷灰：适合长时间浏览照片，不干扰内容本身的色彩判断
    colors: ["#2c3440", "#1d232d", "#0e1116"],
    angle: 165,
    compColor: "#26282c",
    compAlpha: 0.62,
    material: "plain",
  },
  {
    id: "ember-warm",
    name: "余烬暖褐",
    mode: "dark",
    role: "暖调 · 与落日金同族",
    // 暖调：与落日金强调色同色相家族，金色高亮最不突兀
    colors: ["#3a2a22", "#261a16", "#120c0a"],
    angle: 165,
    compColor: "#2b2521",
    compAlpha: 0.62,
    material: "plain",
  },
  {
    id: "pine-night",
    name: "松绿混色",
    mode: "dark",
    role: "彩色档 · 翡翠青蓝光晕",
    // FEAT-110：仪表墙专用彩色预设。预设渐变本身遵守全站「单一色相 + 明度
    // 跨度 ≥0.08」铁律（组件色近中性、文字对比才有保证）；真正的“混色”交给
    // 概览页的三团有色光晕 washes（翡翠青/青蓝/柠光）+ 强调绿→青异色渐变。
    colors: ["#162e25", "#0f1f1a", "#080f0c"],
    angle: 165,
    // 组件色保持近中性（通道极差 ≤12，全站预设的可读性铁律）：
    // 松绿的“彩色”只由背景混色渐变 + 强调绿→青承担，卡片底不染色。
    compColor: "#232a26",
    compAlpha: 0.62,
    material: "plain",
    accent: "#5fd8a4",
    accent2: "#46c2d2",
    washes: [
      "rgba(95, 214, 178, 0.22)",  // 左上 · 翡翠青
      "rgba(96, 178, 214, 0.13)",  // 右上 · 青蓝
      "rgba(214, 230, 150, 0.08)", // 右下 · 柠光
    ],
  },
];

/**
 * 浅色组（6 套 · 一轴一套，互不撞色）
 *
 * ┌────────────┬──────┬──────┬──────────────┬──────────────────────┐
 * │ 预设       │ 色相 │ 彩度 │ 明度跨度     │ 侧重                 │
 * ├────────────┼──────┼──────┼──────────────┼──────────────────────┤
 * │ 亚麻暖灰   │ 36°  │ 0.37 │ 0.90 → 0.70  │ 暖中性 · 日间浏览    │
 * │ 纸白中性   │ —    │ ≈0   │ 0.95 → 0.80  │ 纯中性 · 打印前校对  │
 * │ 雾光冷灰   │ 214° │ 0.37 │ 0.93 → 0.76  │ 冷中性 · 长时阅读    │
 * │ 青瓷浅绿   │ 150° │ 0.23 │ 0.92 → 0.71  │ 绿调 · 自然植物      │
 * │ 陶土暖砂   │ 27°  │ 0.61 │ 0.91 → 0.69  │ 暖彩 · 食物室内      │
 * │ 菊芋淡紫   │ 283° │ 0.42 │ 0.94 → 0.75  │ 紫调 · 人像创作      │
 * └────────────┴──────┴──────┴──────────────┴──────────────────────┘
 *
 * FEAT-116 还整体**压深**了一档（顶段 L 从 0.95~0.97 降到 0.90~0.95）：
 * 上一版六套浅色的顶段几乎都是纯白，于是"白玻璃板叠在白底上"——
 * 板只能靠描边存在，一屏十一块读起来全是描边。
 * 压到 0.90 之后，板（白霜 58%）比页面底更亮，浮起感来自明度差而不是线；
 * 同时照片的暗部有了对照，整屏不再"发飘"。
 *
 * 关键：亚麻与陶土同为暖调，但**彩度差 0.30、明度跨度差 0.04**，
 * 一块 96×56 的色卡上一眼就能分开；纸白与雾光的区别在色相（无 vs 214°），
 * 不再是"两个都挺白的"。
 *
 * 浅色的透明度必须**高于**深色（0.72 vs 0.62）：
 * 同样的 alpha 在浅底上得到的对比度远低于深底，不能共用一套数值。
 */
export const LIGHT_PRESETS: Preset[] = [
  {
    id: "linen-light",
    name: "亚麻暖灰",
    mode: "light",
    role: "暖中性 · 日间浏览",
    // 纸张的暖灰：色相 36°、彩度 0.35 —— 暖但不过火，长时间看眼睛不累。
    colors: ["#efe8dc", "#e0d5c4", "#c5b6a1"],
    angle: 165,
    compColor: "#f2efea", // 通道极差 8（微暖）
    compAlpha: 0.72,
    material: "plain",
    accent: "#8a5a12", // 深金：与暖中性底同族
    accent2: "#b07a22",
  },
  {
    id: "paper-light",
    name: "纸白中性",
    mode: "light",
    role: "纯中性 · 打印前校对",
    // 三通道完全相等 —— 真正的"无色"。判断照片色偏时最不需要的就是界面底色带色相。
    colors: ["#f4f4f2", "#e5e4e0", "#cecdc8"],
    angle: 165,
    compColor: "#f4f4f4", // 通道极差 0
    compAlpha: 0.72,
    material: "plain",
    accent: "#55524c", // 石墨：无彩的强调，像铅笔批注
    accent2: "#7a756c",
  },
  {
    id: "mist-light",
    name: "雾光冷灰",
    mode: "light",
    role: "冷中性 · 长时阅读",
    // 214° 冷灰：与纸白的"无彩色"在同一明度带，但色相明确偏蓝，
    // 两种摆放并排时能立刻分出"这套偏冷"。
    colors: ["#e7edf4", "#d2dce7", "#b2c0d0"],
    angle: 165,
    compColor: "#eff1f4", // 通道极差 5（微冷）
    compAlpha: 0.72,
    material: "plain",
    accent: "#3f5f86", // 钢蓝
    accent2: "#5a7da6",
  },
  {
    id: "celadon-light",
    name: "晨雾青瓷",
    mode: "light",
    role: "绿调 · 自然与植物",
    // 浅色里唯一的冷绿：树/水/草占比高的库用暖白或冷灰都会把绿色压成灰绿。
    colors: ["#e7f0eb", "#cde1d7", "#a4c4b4"],
    angle: 165,
    compColor: "#f0f4f1", // 通道极差 4（微绿）
    compAlpha: 0.72,
    material: "plain",
    accent: "#2f6b4f", // 青瓷绿
    accent2: "#468a67",
  },
  {
    id: "clay-light",
    name: "陶土暖砂",
    mode: "light",
    role: "暖彩 · 食物与室内",
    // 六套里彩度最高（0.65）：衣服 / 食物 / 室内这类暖色主体占多的库，
    // 低彩的"白墙"会把画面衬得发灰，反而需要一层有暖意的底色。
    colors: ["#f6e9da", "#eacfb3", "#d5af8a"],
    angle: 165,
    compColor: "#f5f0ea", // 通道极差 11（微暖，压住彩度不外溢）
    compAlpha: 0.72,
    material: "plain",
    accent: "#9a5526", // 赤陶
    accent2: "#b4713d",
  },
  {
    id: "lilac-light",
    name: "菊芋淡紫",
    mode: "light",
    role: "紫调 · 人像与创作",
    // 与深色默认「落日紫夜」同色相家族，但没有它的重量 —— 浅色里的唯一紫。
    colors: ["#f0e9f6", "#ded1ed", "#bfaad7"],
    angle: 165,
    compColor: "#f3f0f6", // 通道极差 6（微紫）
    compAlpha: 0.72,
    material: "plain",
    accent: "#6b4a8c", // 紫罗兰
    accent2: "#8a68ab",
  },
];

/**
 * 弹窗分组视图：按深浅分两组。
 *
 * 分组标题仍保留，因为两者视觉体系相反（深度对比 vs 轻盈明亮），
 * 放在一起反而难浏览；但**模式不再由用户单独选**，而是跟预设走。
 */
export const PRESET_GROUPS: { group: PresetGroup; title: string; note: string; items: Preset[] }[] = [
  {
    group: "dark",
    title: "深色",
    note: "紫夜 / 石板 / 余烬 / 松绿 · 暗环境与夜间沉浸",
    items: DARK_PRESETS,
  },
  {
    group: "light",
    title: "浅色",
    note: "暖中性 / 纯中性 / 冷中性 / 绿 / 陶土 / 紫 · 每套一个侧重",
    items: LIGHT_PRESETS,
  },
];

/** 全部预设（测试与命中判定用：跨组命中才算选中） */
export const ALL_PRESETS: Preset[] = [...DARK_PRESETS, ...LIGHT_PRESETS];

/** 预设 → 待写入的偏好片段（bgStyle 固定为渐变；mode 一并下发） */
export function presetPrefs(p: Preset): Partial<Prefs> {
  return {
    mode: p.mode,
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
 *
 * FEAT-103 注意：判定**不看 mode**。原因：老用户可能把深色预设的色值
 * 与浅色模式组合存下来（历史行为允许），此时若要求 mode 也匹配才算命中，
 * 他们的选中态会突然消失。色值是这套预设的**身份**，模式是可搭的。
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

/** 预设卡片上的渐变预览 CSS */
export function presetGradient(p: Preset, angle = p.angle): string {
  return `linear-gradient(${angle}deg, ${p.colors[0]}, ${p.colors[1]}, ${p.colors[2]})`;
}
