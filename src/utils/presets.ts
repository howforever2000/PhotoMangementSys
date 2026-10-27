/**
 * 预定义外观（FEAT-117 重建：10 套 → 8 套，深浅各 4，按色相分工）
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么推倒重来（而不是再挪几个色值）
 * ───────────────────────────────────────────────────────────────────────────
 * 上一版 10 套的问题是**没有"风格"这个维度**：每套只调了三段渐变的色相，
 * 组件色、透明度、强调色几乎一样，于是 10 张预览卡看起来像同一张图的 10 个滤镜。
 * 用户选预设时想要的不是"再蓝一点"，而是"换成另一种调子"——
 * 那需要同时动三样东西：背景的明度跨度（决定轻重）、组件色的冷暖（决定界面性格）、
 * 强调色（决定高亮落在哪里）。
 *
 * 这一版每套都按这三样参数化，深色 4 套 = 紫 / 蓝 / 绿 / 褐四个色相各占一角，
 * 浅色 4 套 = 中性 / 暖 / 冷绿 / 暖砂四个温度各占一角 —— 相邻两套永远不会"看起来差不多"。
 *
 * 新增「藏青夜」：参考图的深藏青+青色仪表盘就是这一套。它的存在也把深色从
 * "全是暖调"里拉出来一套真正冷的。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 三条硬约束（presets.test.ts 逐套断言，改数值必须先看测试）
 * ───────────────────────────────────────────────────────────────────────────
 * 1. **单套只有一个色相**：渐变三色标的色相极差 < 10°，否则一屏几个色相必然艳，
 *    会与"单强调色"的色彩预算打架。
 * 2. **明度跨度 ≥ 0.08**：跨度太小 = 背景是一块平色，容器浮不起来，界面会"糊"。
 * 3. **组件色近中性**（RGB 通道极差 ≤ 12）：卡片底一旦带色，就会把背景的色相污染，
 *    文字对比度也从"可算"变成"看运气"。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 兼容策略（关键，决定老用户会不会"开屏变样"）
 * ───────────────────────────────────────────────────────────────────────────
 * 删掉的预设色值**仍然能被识别**：`matchesPreset` 只做字段比对，
 * 已删除的套自然不再命中，于是 `activePresetId` 为 null —— 弹窗里
 * 一张卡都不高亮。这是**正确**的：用户的外观没变（色值还在 localStorage 里），
 * 只是不再被标记为"某个预设"。如果强行把所有旧配色映射到最近的保留项，
 * 用户的开屏外观会真的变掉 —— 那是破坏性的。
 *
 * 保留「落日紫夜」的理由：它是 DEFAULTS 与登录三页（登录/注册/忘记密码）共用的
 * 品牌外观，删掉会让"开屏看到的那套"在弹窗里无卡可高亮。
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
 * 深色组（4 套）：紫 / 蓝 / 绿 / 褐
 *
 * 四个色相各占一角，任意两套放在一起都能一眼分辨；四套的明度跨度都在 0.11~0.16，
 * 同属"深而不黑"的区间（纯黑背景会让照片边框失去参照）。
 */
export const DARK_PRESETS: Preset[] = [
  {
    id: "sunset-dusk",
    name: "落日紫夜",
    mode: "dark",
    // 品牌默认：与登录页启动封面（covers/login-sunset.jpg）及 DEFAULTS 同一套色值。
    // 明度跨度 0.16（#3b2b52 → #120e1c）。
    colors: ["#3b2b52", "#271d37", "#120e1c"],
    angle: 165,
    compColor: "#2a2630",
    compAlpha: 0.62,
    material: "plain",
  },
  {
    id: "night-navy",
    name: "藏青夜",
    mode: "dark",
    // FEAT-117 新增，参考图（Rainmeter 深藏青皮肤）的那套调子。
    // 深色组里唯一真正**冷**的一套：紫夜偏品红、墨褐偏暖，只有它能配冷色照片与
    // 长时间夜间浏览（蓝光感知最弱）。明度跨度 0.155。
    colors: ["#2a3548", "#1c2534", "#0c1017"],
    angle: 165,
    compColor: "#272b31",
    compAlpha: 0.62,
    material: "plain",
    // 青蓝强调：与"藏青"同色相家族但不重复（蓝底上的蓝条会消失），
    // 所以第二色往青绿偏 20°，进度条/环形图才有层次。
    accent: "#7aa8f0",
    accent2: "#5fd0c8",
    washes: [
      "rgba(122, 168, 240, 0.20)", // 左上 · 靛蓝
      "rgba(95, 208, 200, 0.12)", // 右上 · 青绿
      "rgba(150, 170, 220, 0.08)", // 右下 · 蓝灰
    ],
  },
  {
    id: "pine-night",
    name: "松夜绿",
    mode: "dark",
    // 低保和墨绿：与照片里最常见的绿色主体（树/草/水）同族，
    // 界面不会与内容抢色相。
    // FEAT-118：整体提亮一档 —— 原值（L 13.3% → 4.5%）在大多数屏幕上会读成"近黑"，
    // 绿相只在色彩管理好的显示器上才看得出来。提亮后 L 21.4% → 7.5%，
    // 明度跨度 13.9%（仍在"深而不黑"的区间，纯黑背景会让照片边框失去参照）。
    colors: ["#24493c", "#17332b", "#0c1a15"],
    angle: 165,
    // 组件色同步提亮半档并保持近中性（通道极差 10）：松绿的"彩色"
    // 只由背景混色 + 强调绿→青承担，卡片底不染色。
    compColor: "#26302b",
    compAlpha: 0.62,
    material: "plain",
    accent: "#5fd8a4",
    accent2: "#46c2d2",
    washes: [
      "rgba(95, 214, 178, 0.22)", // 左上 · 翡翠青
      "rgba(96, 178, 214, 0.13)", // 右上 · 青蓝
      "rgba(214, 230, 150, 0.08)", // 右下 · 柠光
    ],
  },
  {
    id: "ink-umber",
    name: "墨褐",
    mode: "dark",
    // 暖调，但比旧的「余烬暖褐」更暗更沉（旧值 L 起点 0.19 偏"咖啡"，
    // 这里 0.163 更接近"氧化过的墨"）。与落日金强调色同色相家族，
    // 金色高亮落上去最不突兀。明度跨度 0.114。
    colors: ["#33261f", "#221a16", "#100b09"],
    angle: 165,
    compColor: "#2a2521",
    compAlpha: 0.62,
    material: "plain",
  },
];

/**
 * 浅色组（4 套）：中性 / 暖 / 冷绿 / 暖砂
 *
 * 浅色的透明度必须**高于**深色（0.72 vs 0.62）：同样的 alpha 在浅底上得到的
 * 对比度远低于深底（实测 26% 黑在 #faf8f5 上只有 1.95:1，而 26% 白在 #0a090d
 * 上有 2.09:1），浅色需要更高的 alpha 才能达到同样的可读档。
 *
 * FEAT-117 删掉「纸白暖中性」与「菊芋淡紫」：
 *   · 纸白的组件色与页面底几乎同色（层次全靠描边），在去卡片化之后等于没有承载面；
 *   · 淡紫是浅色组唯一的彩度来源，但紫底 + 落日金 = 两套色相互相污染，
 *     且与深色默认「落日紫夜」重复了同一个色相 —— 深浅两组不该撞色相。
 *   补上「雾白」把中性档做实（冷暖之间那档原来是空的）。
 */
export const LIGHT_PRESETS: Preset[] = [
  {
    id: "mist-light",
    name: "雾白",
    mode: "light",
    // 真中性：三色标色相一致，通道差 <4。看照片时最不干扰色彩判断的一套。
    // 明度跨度 0.135（#f4f5f7 → #c2c9d3）。
    colors: ["#f4f5f7", "#e2e6eb", "#c2c9d3"],
    angle: 165,
    compColor: "#eef0f2",
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "linen-light",
    name: "亚麻",
    mode: "light",
    // 暖中性，纸张感。明度跨度 0.145。
    colors: ["#f4f1ec", "#e6e2da", "#cfc9be"],
    angle: 165,
    compColor: "#efeeec",
    compAlpha: 0.72,
    material: "plain",
  },
  {
    id: "celadon-light",
    name: "晨雾青瓷",
    mode: "light",
    // 浅色里唯一的冷绿：树/水/草占比高的库，用暖白或冷灰都会把绿色压成灰绿。
    // 明度跨度 0.115，仍属"素"的范围。
    colors: ["#eaf1ee", "#d8e5df", "#b6cbc2"],
    angle: 165,
    compColor: "#eef4f1",
    compAlpha: 0.72,
    material: "plain",
    // 青瓷配青绿强调（不配金）：冷底上的金色会显得脏，像旧纸上的黄斑。
    accent: "#2f9e86",
    accent2: "#4fb8ad",
  },
  {
    id: "clay-light",
    name: "陶砂",
    mode: "light",
    // 暖砂：明度跨度 0.20（浅色组里最大），立体感最强，适合衣服 / 食物 / 室内。
    colors: ["#f7f0e8", "#eddccb", "#d8bda2"],
    angle: 165,
    compColor: "#f4f0ea",
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
    note: "紫 / 蓝 / 绿 / 褐 四个色相 · 适合暗环境浏览",
    items: DARK_PRESETS,
  },
  {
    group: "light",
    title: "浅色",
    note: "中性 / 暖 / 冷绿 / 暖砂 四个温度 · 适合日间与打印前校对",
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
