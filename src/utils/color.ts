/**
 * 颜色数学 + 「组件色调」派生（FEAT-084）
 *
 * 从 stores/theme.ts 抽出：主题里的背景对比度计算与组件色调派生要用同一套
 * RGB/WCAG 工具，放在纯函数模块里既避免两份实现漂移，也能被 node:test 直接单测
 * （不依赖 DOM / Pinia）。
 */

export type Rgb = [number, number, number];

/** 浅色模式主文字（组件底色对比基准） */
export const TEXT_DARK: Rgb = [0x1f, 0x27, 0x33];
/** 深色模式主文字 */
export const TEXT_LIGHT: Rgb = [0xf5, 0xf7, 0xff];

const HEX_RE = /^#?[0-9a-fA-F]{3,8}$/;

/** 解析 #rgb / #rrggbb；不合法时返回 fallback（默认白），避免把坏值写进 CSS 变量 */
export function normalizeHex(hex: string, fallback = "#ffffff"): string {
  const raw = String(hex ?? "").trim();
  if (!HEX_RE.test(raw)) return fallback;
  let h = raw.replace("#", "");
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  if (h.length !== 6) return fallback;
  return `#${h.toLowerCase()}`;
}

export function hexToRgb(hex: string): Rgb {
  const h = normalizeHex(hex).slice(1);
  const n = Number.parseInt(h, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgbToHex([r, g, b]: Rgb): string {
  const p = (v: number) =>
    Math.max(0, Math.min(255, Math.round(v))).toString(16).padStart(2, "0");
  return `#${p(r)}${p(g)}${p(b)}`;
}

/** #rrggbb + alpha → rgba() 文本（供内联 style 使用） */
export function hexToRgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r},${g},${b},${alpha})`;
}

/** WCAG 相对亮度（sRGB 线性化） */
export function relLum(rgb: Rgb): number {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * f(rgb[0]) + 0.7152 * f(rgb[1]) + 0.0722 * f(rgb[2]);
}

export function contrastRatio(a: number, b: number): number {
  const hi = Math.max(a, b);
  const lo = Math.min(a, b);
  return (hi + 0.05) / (lo + 0.05);
}

/** 线性插值：t=0 → a，t=1 → b */
export function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
  return [
    Math.round(a[0] + (b[0] - a[0]) * t),
    Math.round(a[1] + (b[1] - a[1]) * t),
    Math.round(a[2] + (b[2] - a[2]) * t),
  ];
}

/**
 * 三段渐变（0 → mid → 1 各占一半）的平均色：(from + 2·mid + to) / 4。
 * 分段线性插值在整个区间上的积分均值——用于把三段渐变折算成
 * 「等效纯色背景」，供文字对比度计算（FEAT-086 三段渐变预设）。
 */
export function gradientAverage(from: Rgb, mid: Rgb, to: Rgb): Rgb {
  return [
    Math.round((from[0] + 2 * mid[0] + to[0]) / 4),
    Math.round((from[1] + 2 * mid[1] + to[1]) / 4),
    Math.round((from[2] + 2 * mid[2] + to[2]) / 4),
  ];
}

/**
 * 直接落在页面背景上的文字取色（标题/副标题）：
 *   a. 首选色（跟随模式）对比 ≥4.5:1 → 用首选；
 *   b. 首选不足 → 深/浅两档里取 ≥4.5:1 的一侧；
 *   c. 两档都不足（等效背景落在中灰区）→ 黑/白两端取对比更高者，
 *      保证任何背景 ≥4.5:1（FEAT-086：预设中灰背景可达 4.26:1，需黑白兜底）。
 * 返回颜色文本（#rrggbb 或常量色）。
 */
export function onBgText(bg: Rgb, prefersLight: boolean): string {
  const lum = relLum(bg);
  const preferred = prefersLight ? TEXT_LIGHT : TEXT_DARK;
  const preferredHex = prefersLight ? "#f5f7ff" : "#1f2733";
  if (contrastRatio(relLum(preferred), lum) >= 4.5) return preferredHex;

  const cLight = contrastRatio(relLum(TEXT_LIGHT), lum);
  const cDark = contrastRatio(relLum(TEXT_DARK), lum);
  if (Math.max(cLight, cDark) >= 4.5) return cLight >= cDark ? "#f5f7ff" : "#1f2733";

  // 双双不足 → 纯黑/纯白两端取更高（纯色端点对比总 ≥ 5.9:1，除非背景恰为中灰）
  const cWhite = contrastRatio(relLum([255, 255, 255]), lum);
  const cBlack = contrastRatio(relLum([0, 0, 0]), lum);
  return cWhite >= cBlack ? "#ffffff" : "#000000";
}

/** 文字色是否属于「深色文字」（决定阴影方向） */
export function isDarkText(color: string): boolean {
  const [r, g, b] = hexToRgb(color);
  return r + g + b < 384;
}

/** 组件色调派生结果：一次算出要下发的整组令牌 */
export interface ComponentTone {
  /** 组件底色偏深 → 文字需翻成浅色 */
  onDark: boolean;
  /** 组件底色（归一化 hex） */
  surface: string;
  /** 次级面板 / 控件底色：浅底压暗 4%，深底提亮 8%，保持层级 */
  surface2: string;
  /** 描边：浅底混黑 12%，深底用半透明白 */
  border: string;
  text: string;
  text2: string;
  text3: string;
}

/**
 * 由「组件色调」单色派生整组组件令牌。
 * 文字不硬编码深/浅，而是在深/浅两档文字里取**对比度更高**的一档
 * —— 用户选到中间灰时两档都谈不上 4.5:1，至少取可读性更好的一侧。
 */
export function componentTone(hex: string): ComponentTone {
  const surface = normalizeHex(hex, "#ffffff");
  const rgb = hexToRgb(surface);
  const lum = relLum(rgb);
  const onDark =
    contrastRatio(relLum(TEXT_LIGHT), lum) > contrastRatio(relLum(TEXT_DARK), lum);

  return {
    onDark,
    surface,
    surface2: rgbToHex(
      onDark ? mixRgb(rgb, [255, 255, 255], 0.08) : mixRgb(rgb, [0, 0, 0], 0.04),
    ),
    border: onDark ? "rgba(255,255,255,.16)" : rgbToHex(mixRgb(rgb, [0, 0, 0], 0.12)),
    text: onDark ? "#f5f7ff" : "#1f2733",
    text2: onDark ? "rgba(225,232,255,.86)" : "rgba(36,48,68,.88)",
    text3: onDark ? "rgba(214,221,240,.82)" : "#4b5b6e",
  };
}
