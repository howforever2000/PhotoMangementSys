/**
 * 登录/注册/忘记密码三页共用的「落日紫金」配色（FEAT-093）
 *
 * 为什么单独一个模块：这三个页面用一个**固定的启动壁纸**（covers/login-sunset.jpg，
 * 日落山峦），配色必须对着这张图的实测亮度来定，而不是跟着用户主题色走
 * （登录页在主题生效之前就要显示，也是产品第一眼形象）。
 * 抽成纯函数模块的原因与 prefs.ts / presets.ts 一致：不依赖 DOM，node:test 直接断言
 * 对比度——换壁纸时先跑测试就知道新图会不会把字吃掉。
 *
 * 壁纸实测（covers/login-sunset.jpg，源图 2730×1536 按 6px 步长分区采样）：
 *   - 顶部暖橙 #f7bf9d（相对亮度 0.55，全图最亮，且正落在卡片上沿）
 *   - 中部玫紫 #a56e95 / 卡片区均值 #be798b
 *   - 底部深靛 #29347b（0.07）
 * 对比度一律按**最坏情况**断言：卡片压在最亮粉带上时仍须 ≥4.5:1。
 */
import {
  componentTone,
  hexToRgb,
  hexToRgba,
  mixRgb,
  normalizeHex,
  rgbToHex,
  type Rgb,
} from "./color.ts";

export interface AuthPalette {
  /** 组件色调：卡片玻璃的基色（与壁纸的紫山同色系） */
  cardTone: string;
  /** 卡片玻璃不透明度（容器基准） */
  cardAlpha: number;
  /** 卡片内标题（纯白，避让壁纸亮部用满对比） */
  title: string;
  /** 副标题 / 说明 */
  sub: string;
  /** 字段标签 */
  label: string;
  /** 链接 / 次级强调（暖金，呼应壁纸的落日） */
  link: string;
  /** 输入框占位符 */
  placeholder: string;
  /** 输入框填料：卡片玻璃之上再叠一层组件色调 */
  fieldAlpha: number;
  /** 输入框描边色 + 不透明度（WCAG 1.4.11 控件边界 ≥3:1，实测 0.48 档达标） */
  fieldBorder: string;
  fieldBorderAlpha: number;
  /** 主按钮：落日金 + 墨字（金底上白字仅 2.16:1，必须用深色字） */
  accent: string;
  accentHover: string;
  accentInk: string;
  /** 提示条：自身底色叠在卡片玻璃之上 */
  dangerText: string;
  dangerBg: string;
  okText: string;
  okBg: string;
  /** 提示条底色的不透明度 */
  messageAlpha: number;
}

export const AUTH_PALETTE: AuthPalette = {
  cardTone: "#2b2140",
  cardAlpha: 0.64,
  title: "#ffffff",
  sub: "#f5f3fd",
  label: "#f0eefb",
  link: "#ffd9a0",
  placeholder: "#d6d0e2",
  fieldAlpha: 0.32,
  fieldBorder: "#ffffff",
  fieldBorderAlpha: 0.48,
  accent: "#e8a33d",
  accentHover: "#f2b355",
  accentInk: "#2a1a08",
  dangerText: "#ffd6d6",
  dangerBg: "#7a1a2a",
  okText: "#d8f5dd",
  okBg: "#14532d",
  messageAlpha: 0.55,
};

/**
 * 页面遮罩（scrim）：压暗壁纸、把视线收进卡片。
 * 遮罩只会让字更清楚，所以这里给的是**下限**——卡片区最亮处仍 ≥0.35。
 * 两层渐变在卡片上沿（视口 y≈19%）的合成值 ≈ 0.34 + 0.19×0.14 + 0.09 ≈ 0.45，
 * 模型按 0.35 保守断言，实测由 .impeccable/verify-auth.cjs 读渲染像素复核。
 * 也不能压太狠：BUG-2026-0919-001 曾因遮罩过浓把封面盖死。
 */
export const AUTH_SCRIM = {
  color: "#0a0814",
  /** 卡片区的等效不透明度下限（对比度断言用的就是它） */
  alphaUnderCard: 0.35,
  /** 竖向渐变的起止不透明度：上浅下深 */
  top: 0.36,
  bottom: 0.5,
  /** 边角暗角强度（卡片在中心，不受它影响） */
  vignette: 0.35,
} as const;

/** 壁纸采样点（改壁纸时同步更新，测试按最亮值重新断言） */
export const AUTH_WALLPAPER_SAMPLES = {
  /** 全图最亮处，且落在卡片上沿 —— 对比度的最坏情况 */
  brightest: "#f7bf9d",
  /** 卡片区均值 */
  cardZone: "#be798b",
  /** 全图最暗处 */
  darkest: "#29347b",
} as const;

/** 壁纸底色 → 卡片玻璃的等效纯色：先压遮罩，再叠组件色调 */
export function authGlass(
  wallpaper: string = AUTH_WALLPAPER_SAMPLES.brightest,
  p: AuthPalette = AUTH_PALETTE,
  scrimAlpha: number = AUTH_SCRIM.alphaUnderCard,
): Rgb {
  const under = mixRgb(hexToRgb(normalizeHex(wallpaper)), hexToRgb(AUTH_SCRIM.color), scrimAlpha);
  return mixRgb(under, hexToRgb(normalizeHex(p.cardTone, "#ffffff")), p.cardAlpha);
}

/** 输入框底：卡片玻璃之上再叠一层组件色调（与主题弹窗 compFill(0.34) 同一手法） */
export function authFieldBg(glass: Rgb, p: AuthPalette = AUTH_PALETTE): Rgb {
  return mixRgb(glass, hexToRgb(normalizeHex(p.cardTone, "#ffffff")), p.fieldAlpha);
}

/** 提示条底：自身色叠加在卡片玻璃之上 */
export function authMsgBg(glass: Rgb, bg: string, p: AuthPalette = AUTH_PALETTE): Rgb {
  return mixRgb(glass, hexToRgb(normalizeHex(bg, "#000000")), p.messageAlpha);
}

/** 遮罩背景（两层：四角暗角 + 上浅下深的竖向压暗），卡片区取最坏情况 */
export function authScrimBackground(scrim: typeof AUTH_SCRIM = AUTH_SCRIM): string {
  const c = normalizeHex(scrim.color, "#000000");
  return [
    `radial-gradient(120% 100% at 50% 45%, ${hexToRgba(c, 0)} 0%, ${hexToRgba(c, scrim.vignette)} 100%)`,
    `linear-gradient(180deg, ${hexToRgba(c, scrim.top)} 0%, ${hexToRgba(c, scrim.bottom)} 100%)`,
  ].join(", ");
}

/**
 * 下发到 .auth-page 的 CSS 变量包。
 * 单一事实来源：auth.css 只引用 var(--auth-*)，色值全部由这里算出，
 * 避免「TS 一套、CSS 另一套」的漂移（对比度断言断的就是这一套）。
 */
export function authCssVars(
  p: AuthPalette = AUTH_PALETTE,
  scrim: typeof AUTH_SCRIM = AUTH_SCRIM,
): Record<string, string> {
  // 描边取向按最坏情况（最亮壁纸带）的玻璃等效底色推：始终落在深底一档
  const tone = componentTone(rgbToHex(authGlass(AUTH_WALLPAPER_SAMPLES.brightest, p, scrim.alphaUnderCard)));
  return {
    "--auth-card-bg": hexToRgba(p.cardTone, p.cardAlpha),
    "--auth-card-border": tone.border,
    "--auth-title": p.title,
    "--auth-sub": p.sub,
    "--auth-label": p.label,
    "--auth-link": p.link,
    "--auth-placeholder": p.placeholder,
    "--auth-field-bg": hexToRgba(p.cardTone, p.fieldAlpha),
    /* 自动填充兑底：Chromium 会把输入框底刷成不透明浅黄，必须用不透明色盖回去，
       半透明盖不住（旧写法 rgba(28,36,62,.82) 同一目的） */
    "--auth-field-bg-solid": hexToRgba(p.cardTone, 0.92),
    "--auth-field-border": hexToRgba(p.fieldBorder, p.fieldBorderAlpha),
    "--auth-accent": p.accent,
    "--auth-accent-hover": p.accentHover,
    "--auth-accent-ink": p.accentInk,
    "--auth-danger-text": p.dangerText,
    "--auth-danger-bg": hexToRgba(p.dangerBg, p.messageAlpha),
    "--auth-ok-text": p.okText,
    "--auth-ok-bg": hexToRgba(p.okBg, p.messageAlpha),
    "--auth-scrim": authScrimBackground(scrim),
  };
}
