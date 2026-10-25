/**
 * 预定义外观（FEAT-103 重做：10 套 → 6 套，深浅配对）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么从 10 套砍到 6 套
 * ───────────────────────────────────────────────────────────────────────────
 * 旧版两组（经典渐变 5 + 素色 5）的共同问题不是"不好看"，而是**没有分类维度**：
 *   · 10 套全都是"浅顶 → 深底"的同向渐变，用户看不出它们分别适合什么场景；
 *   · 经典组里 sage-rose / cream-violet / mint-indigo / butter-plum / lemon-teal
 *     用了 3 个色相（粉→橄榄→墨绿），一屏三个色相必然"艳"，
 *     与"单强调色（落日金）"的色彩预算直接冲突；
 *   · 素色组的 5 套里有 3 套暖褐（linen / clay / amber）几乎同色相，
 *     选了之后分不清自己选的是哪一套。
 *
 * 现在按**深浅模式**分类（这是用户唯一真正需要先决定的维度），
 * 每类 3 套、每类内色相互不重叠：
 *   深色：紫夜（默认/品牌）/ 石板（中性冷）/ 余烬（暖调）
 *   浅色：亚麻（暖中性）/ 雾光（冷中性）/ 纸白（近纯中性）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 兼容策略（关键，决定老用户会不会"开屏变样"）
 * ───────────────────────────────────────────────────────────────────────────
 * 删掉的 6 套色值**仍然能被识别**：`matchesPreset` 只做字段比对，
 * 已删除的套自然不再命中，于是 `activePresetId` 为 null —— 弹窗里
 * 一张卡都不高亮。这是**正确**的：用户的外观没变（色值还在 localStorage 里），
 * 只是不再被标记为"某个预设"。如果强行把所有旧配色映射到最近的保留项，
 * 用户的开屏外观会真的变掉 —— 那是破坏性的。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 新增 `mode` 字段
 * ───────────────────────────────────────────────────────────────────────────
 * 旧版 `DEFAULTS.mode` 硬编码为 "dark"，`ThemeDialog` 里也没有模式开关，
 * 于是"浅色主题"这个维度实际上不存在（模板里有 `body:not(.theme-dark)`，
 * 但用户无法切过去）。现在每套预设带 `mode`，`presetPrefs()` 一并下发，
 * 点浅色预设即同时切换模式与配色 —— 这才是"预设"该有的语义。
 */
import type { Prefs, ThemeMode } from "./prefs.ts";

export interface Preset {
  id: string;
  name: string;
  /** 这套预设所属的深浅模式：切换时一并下发 */
  mode: ThemeMode;
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
   * 为什么不留液态选项：液态玻璃在新方案里已被限定为**浮层材质**
   * （见 design/design-system.md §3），卡片不再是玻璃。
   * 因此"预设带不带液态"这个问题本身消失了 —— 材质不再由预设控制，
   * 而由组件层决定。字段保留是为了不破坏 normalizePrefs 的结构兼容。
   */
  material: "plain";
  /**
   * FEAT-110（可选）：仪表板 widget 的专属外观。
   *
   * 这些字段**只影响智慧相册「全局概览」的 widget 墙**（--w-* 令牌），
   * 不触碰全站卡片（--color-* 与 --glass-* 令牌）与背景渐变本身 —— 所以即使缺省
   * 也不会改变任何现有界面：widget 基类在字段缺失时回落到按渐变派生的默认值。
   *
   * · accent / accent2：widget 强调渐变色对（进度条 / 高亮柱 / 环）。
   *   老预设不填 → 用全局落日金（深）/ 深金（浅），与效果图之前一致。
   * · washA/washerB/washC：叠在页面背景上的三团有色光晕（混色感），
   *   只在概览页挂载期间由组件挂到一个 scoped 层上，离开页面即移除，
   *   不改全局 body 背景，避免"选个预设把别的页也染了"。
   */
  accent?: string;
  accent2?: string;
  washes?: [string, string, string];
}

/** 分组 id：主题弹窗据此渲染分组标题 */
export type PresetGroup = "dark" | "light";

/**
 * 深色组（4 套）
 *
 * 色相分工：紫（品牌）/ 中性冷（专业感）/ 暖褐（亲和感）/ 松绿混色（仪表墙彩色档）。
 *
 * FEAT-112：删掉「深空石墨」—— 它与「墨色石板」同为中性冷、只差 4° 色相与 3% 明度，
 * 实测两张预设卡肉眼几乎同色，用户选不出来，等于白占一个位置。
 * （取舍：保留范围更通用的「石板」，它覆盖“专业 / 不干扰照片色”这个常见需求。）
 */
export const DARK_PRESETS: Preset[] = [
  {
    id: "sunset-dusk",
    name: "落日紫夜",
    mode: "dark",
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
      "rgba(214, 230, 150, 0.08)",  // 右下 · 柠光
    ],
  },
];

/**
 * 浅色组（4 套）
 *
 * 浅色的透明度必须**高于**深色（0.72 vs 0.62）：
 * 同样的 alpha 在浅底上得到的对比度远低于深底（实测 26% 黑在 #faf8f5 上
 * 只有 1.95:1，而 26% 白在 #0a090d 上有 2.09:1 —— 两者接近，但浅色需要
 * 更高的 alpha 才能达到同样的"可读"档），所以不能共用一套数值。
 */
export const LIGHT_PRESETS: Preset[] = [
  {
    id: "linen-light",
    name: "亚麻浅灰",
    mode: "light",
    colors: ["#f4f1ec", "#e6e2da", "#cfc9be"],
    angle: 165,
    compColor: "#efeeec",  // S=0.086（微暖）
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "mist-light",
    name: "雾光冷灰",
    mode: "light",
    colors: ["#eef1f4", "#dae0e7", "#bcc5cf"],
    angle: 165,
    compColor: "#ededef",  // S=0.059（微冷）
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "paper-light",
    name: "纸白暖中性",
    mode: "light",
    // 最接近"白纸"的一套：卡片底与页面底几乎同色，层次全靠描边与留白
    colors: ["#faf8f5", "#f0ece6", "#ddd7cd"],
    angle: 165,
    compColor: "#f4f3f1",  // S=0.120（近中性；浅色底上这点饱和度肉眼不可辨）
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "celadon-light",
    name: "晨雾青瓷",
    mode: "light",
    // 浅色里唯一的冷绿：树/水/草占比高的库用暖白或冷灰都会把绿色压成灰绿。
    // 明度跨度 17（#eaf1ee→#b6cbc2），比纸白（12）更立体，仍属于“素”的范围。
    colors: ["#eaf1ee", "#d8e5df", "#b6cbc2"],
    angle: 165,
    compColor: "#eef4f1",  // S=0.025（近中性，微绿）
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "lilac-light",
    name: "菊芋淡紫",
    mode: "light",
    // FEAT-112 新增：浅色组原本四套全落在“中性/暖/冷/绿”，没有任何彩度高的色相，
    // 于是用户在浅色里选来选去都是“差不多的白”。这套补一个紫调 ——
    // 与深色默认「落日紫夜」同色相家族，但没有它的重量。
    // 明度跨度 0.14（#f3eef7 → #c9bede），色相跨度 <3°。
    colors: ["#f3eef7", "#e4dcef", "#c9bede"],
    angle: 165,
    compColor: "#f1eef4",  // 通道极差 6（近中性，微紫）
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "clay-light",
    name: "陶土暖砂",
    mode: "light",
    // FEAT-112 新增：与「亚麻」同为暖中性但明度差大得多（亚麻 0.14 / 陶土 0.20），
    // 一看就是两套；适合衣服 / 食物 / 室内这类暖色主体占多的库。
    colors: ["#f7f0e8", "#eddccb", "#d8bda2"],
    angle: 165,
    compColor: "#f4f0ea",  // 通道极差 10（近中性，微暖）
    compAlpha: 0.72,
    material: "plain",
  },
];

/**
 * 弹窗分组视图：按深浅分两组。
 *
 * FEAT-112：深浅模式开关已从弹窗移除（用户反馈“深色/浅色切换没用”），
 * 分组标题仍保留，因为两者视觉体系相反（深度对比 vs 轻盈明亮），
 * 放在一起反而难浏览；但**模式不再由用户单独选**，而是跟预设走。
 */
export const PRESET_GROUPS: { group: PresetGroup; title: string; note: string; items: Preset[] }[] = [
  {
    group: "dark",
    title: "深色",
    note: "紫夜 / 石板 / 余烬 / 松绿混色 · 适合暗环境浏览",
    items: DARK_PRESETS,
  },
  {
    group: "light",
    title: "浅色",
    note: "亚麻 / 雾光 / 陶土 / 纸白 / 青瓷 / 菊芋 · 适合日间与打印前校对",
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
