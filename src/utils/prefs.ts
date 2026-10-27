/**
 * 主题偏好数据层（FEAT-086 重构自 stores/theme.ts）
 *
 * Prefs 结构、默认值与「旧 localStorage → 新结构」的归一化放这里：
 *   - 纯函数、不依赖 DOM / Pinia → node:test 可直接验证兼容性；
 *   - store 只负责读 localStorage、响应式与下发，职责分离（CS1）。
 *
 * FEAT-086 新增字段：
 *   - gradMid：三段渐变中段色标（旧数据缺失 → 由首尾两色插值补出）；
 *   - compAlpha：组件玻璃填充不透明度（旧数据缺失 → 沿用 0.42 默认）；
 *   - material：卡片材质（普通 plain / 液态 liquid），预设一律普通卡片。
 *
 * FEAT-087 新增字段：
 *   - saturation：全局饱和度缩放（1 = 原始色，白字/背景/品牌色/语义色统一乘）
 *
 * Q3 新增字段：
 *   - temperature：色温（-1 冷 ↔ 0 中性 ↔ +1 暖）。默认 0 = 零位移，
 *     所以老安装的 localStorage 归一化后外观逐像素不变。
 *     与 saturation 的区别：**语义色族不接色温** —— 把危险红推暖就变成橙红，
 *     语义会失效；饱和度没有这个问题（红仍是红）。
 */
import { hexToRgb, mixRgb, normalizeHex, rgbToHex } from "./color.ts";

export type ThemeMode = "light" | "dark";
export type BackgroundStyle = "image" | "gradient" | "color";
/** 卡片材质：普通（无模糊/无纹理）/ 液态（低模糊 + 流体高光） */
export type Material = "plain" | "liquid";
/** FEAT-110：仪表墙 widget 材质（只作用于智慧相册「全局概览」等 widget 墙，
 *  与全站卡片材质 Material 互不影响）：玻璃（半透模糊）/ 普通（实心 tint） */
export type WidgetMaterial = "glass" | "plain";

export interface Prefs {
  mode: ThemeMode;
  /** 组件色调：卡片/面板等组件底色 */
  compColor: string;
  /** 组件玻璃填充不透明度（容器基准；面板 = +0.24，弹层 = +0.36） */
  compAlpha: number;
  /** 全局饱和度缩放：1 = 原始色（所有颜色令牌统一乘，含背景/品牌/语义色） */
  saturation: number;
  /** 色温：-1 冷 ↔ 0 中性 ↔ +1 暖（只改色相轴，WCAG 亮度锁定；**不作用于语义色族**） */
  temperature: number;
  /** 玻璃材质 */
  material: Material;
  /* FEAT-117：widgetMaterial 已并入 material（一个材质控制所有），
     不再是 Prefs 的字段。旧 localStorage 里残留的这个键会被 normalizePrefs
     自然丢弃（它按白名单逐字段构建对象），不会报错。 */
  bgStyle: BackgroundStyle;
  bgColor: string;
  gradFrom: string;
  /** 三段渐变中段色标 */
  gradMid: string;
  gradTo: string;
  gradAngle: number;
  bgOpacity: number;
}

export const DEFAULTS: Prefs = {
  mode: "dark",
  /* FEAT-094：默认外观 = 「启动封面」——壁纸用登录页那张日落山峦（covers/login-sunset.jpg），
     组件配色与其配套的落日紫金（与登录/注册/忘记密码三页同一套，见 utils/loginTheme.ts）。
     壁纸本身**不存 localStorage**：theme store 在「背景图模式且用户未自定义图」时回落到
     构建产物里的内置资源，所以 176KB 图片不会占本地存储配额、也不会随偏好丢失。 */
  /* 组件色调 = 玻璃色调：比壁纸更暗一档，保证「背景→容器→内容」三层可读。
     素化 C7：#2b2140（S≈0.34，带明显紫）→ #2a2630（S≈0.14，近中性灰紫）。
     原值把紫带进了卡片底，于是「壁纸的暖粉紫 + 卡片的紫 + 强调金」三套色相并存。
     降饱和后卡片底退成中性承载体，层次全交给明度 —— 壁纸那一个色相不再被稀释。 */
  compColor: "#2a2630",
  compAlpha: 0.64,
  saturation: 1,
  /* Q3 色温默认 0 = 零位移 ⇒ 老用户（localStorage 里没有这个字段）的外观逐像素不变 */
  temperature: 0,
  material: "plain",
  bgStyle: "image",
  /* 图片层下方的底色：紫夜色 —— 壁纸是暖粉紫，底下若仍是墨绿会把画面染脏 */
  bgColor: "#1a1428",
  /* 渐变档的三段色标也换成紫夜色系（用户在弹窗里切到「渐变色」时的默认值）。
     注意：gradMid **必须等于 gradFrom/gradTo 的插值中点**（#3b2b52 + #120e1c 的一半）——
     normalizePrefs 对缺中段的旧数据就是这么推的，prefs.test 锁了「空输入 == DEFAULTS」这条不变量。 */
  gradFrom: "#3b2b52",
  gradMid: "#271d37",
  gradTo: "#120e1c",
  gradAngle: 135,
  /* 壁纸透明度 55%：明显但含蓄（此前 45% 几乎看不出背景图） */
  bgOpacity: 0.55,
};

/**
 * 当前偏好是否就是「默认外观（启动封面）」—— 主题弹窗用它高亮默认卡。
 * 不拿对象深比对，而是逐字段比可调项：mode/saturation 不参与（改了颜色浓淡但结构未变
 * 不算脱离默认，否则用户拖一下饱和度卡片就掉高亮，反而困惑）。
 * hasCustomImage：用户自己选过背景图 → 一律不算默认。
 */
export function isDefaultLook(prefs: Prefs, hasCustomImage: boolean): boolean {
  if (hasCustomImage) return false;
  return (
    prefs.bgStyle === DEFAULTS.bgStyle &&
    prefs.bgColor === DEFAULTS.bgColor &&
    prefs.gradFrom === DEFAULTS.gradFrom &&
    prefs.gradMid === DEFAULTS.gradMid &&
    prefs.gradTo === DEFAULTS.gradTo &&
    prefs.compColor === DEFAULTS.compColor &&
    Math.abs(prefs.compAlpha - DEFAULTS.compAlpha) < 0.001 &&
    Math.abs(prefs.bgOpacity - DEFAULTS.bgOpacity) < 0.001 &&
    prefs.material === DEFAULTS.material
  );
}

const BG_STYLES: BackgroundStyle[] = ["image", "gradient", "color"];
/* 旧版本写入的 frosted / glazed 会在 normalizePrefs 里回落为 plain —— 两种材质收敛为「普通卡片」。 */
const MATERIALS: Material[] = ["plain", "liquid"];

function str(v: unknown, fallback: string): string {
  return typeof v === "string" ? v : fallback;
}

function num(v: unknown, fallback: number, min: number, max: number): number {
  const n = typeof v === "number" ? v : Number(v);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

function oneOf<T extends string>(v: unknown, allowed: T[], fallback: T): T {
  return typeof v === "string" && (allowed as string[]).includes(v) ? (v as T) : fallback;
}

/**
 * 旧 localStorage 偏好 → 当前 Prefs 结构。
 * 缺字段/坏值一律回落默认或从已有字段推导，绝不抛错（坏 JSON 由调 try/catch 兜）。
 */
export function normalizePrefs(raw: unknown): Prefs {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const gradFrom = normalizeHex(str(r.gradFrom, DEFAULTS.gradFrom), DEFAULTS.gradFrom);
  const gradTo = normalizeHex(str(r.gradTo, DEFAULTS.gradTo), DEFAULTS.gradTo);
  // 旧版本只有首尾两色：中段用插值中点补出，视觉上退化为两段渐变，无缝兼容
  const gradMid = normalizeHex(
    str(r.gradMid, ""),
    rgbToHex(mixRgb(hexToRgb(gradFrom), hexToRgb(gradTo), 0.5)),
  );
  return {
    mode: oneOf(r.mode, ["light", "dark"], DEFAULTS.mode),
    compColor: normalizeHex(str(r.compColor, DEFAULTS.compColor), DEFAULTS.compColor),
    compAlpha: num(r.compAlpha, DEFAULTS.compAlpha, 0, 0.9),
    saturation: num(r.saturation, DEFAULTS.saturation, 0.4, 1.5),
    temperature: num(r.temperature, DEFAULTS.temperature, -1, 1),
    material: oneOf(r.material, MATERIALS, DEFAULTS.material),
    bgStyle: oneOf(r.bgStyle, BG_STYLES, DEFAULTS.bgStyle),
    bgColor: normalizeHex(str(r.bgColor, DEFAULTS.bgColor), DEFAULTS.bgColor),
    gradFrom,
    gradMid,
    gradTo,
    gradAngle: num(r.gradAngle, DEFAULTS.gradAngle, 0, 360),
    bgOpacity: num(r.bgOpacity, DEFAULTS.bgOpacity, 0.05, 1),
  };
}
