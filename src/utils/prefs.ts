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
 *   - material：玻璃材质（磨砂 frosted / 液态 liquid），预设一律磨砂。
 */
import { hexToRgb, mixRgb, normalizeHex, rgbToHex } from "./color.ts";

export type ThemeMode = "light" | "dark";
export type BackgroundStyle = "image" | "gradient" | "color";
/** 玻璃材质预设：磨砂（高模糊）/ 液态（低模糊 + 高光流体）/ 釉瓷（缎面 + 镜面天光带） */
export type Material = "frosted" | "liquid" | "glazed";

export interface Prefs {
  mode: ThemeMode;
  /** 组件色调：卡片/面板等组件底色 */
  compColor: string;
  /** 组件玻璃填充不透明度（容器基准；面板 = +0.24，弹层 = +0.36） */
  compAlpha: number;
  /** 玻璃材质 */
  material: Material;
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
  /* 组件色调 = 玻璃色调：比页面背景亮一档，保证「背景→容器→内容」三层可读 */
  compColor: "#16443a",
  compAlpha: 0.42,
  material: "frosted",
  bgStyle: "color",
  bgColor: "#0e211b",
  gradFrom: "#12332a",
  gradMid: "#0c211b",
  gradTo: "#050f0c",
  gradAngle: 135,
  bgOpacity: 0.45,
};

const BG_STYLES: BackgroundStyle[] = ["image", "gradient", "color"];
const MATERIALS: Material[] = ["frosted", "liquid", "glazed"];

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
    compAlpha: num(r.compAlpha, DEFAULTS.compAlpha, 0.15, 0.9),
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
