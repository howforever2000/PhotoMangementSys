/**
 * 自定义预设（FEAT-118）—— 用户把自己的调色结果存成一套外观
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 为什么单独放 localStorage 而不是塞进 Prefs
 * ───────────────────────────────────────────────────────────────────────────
 * Prefs 是「当前长什么样」——每个字段都是单一真值，normalizePrefs 逐字段白名单归一化。
 * 自定义预设是「可保存的外观集合」，是**列表**而不是状态：它没有默认值、
 * 不参与 isDefaultLook、删光了也不该影响当前外观。塞进 Prefs 会让
 * normalizePrefs 的"白名单逐字段"模型破功，还会让 prefs.test 的
 * 「空输入 == DEFAULTS」不变量失效。所以走独立键，和取色器的最近色同一个先例。
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * 三条硬约束
 * ───────────────────────────────────────────────────────────────────────────
 * 1. **上限 6 个**。上限不是为了省存储（一条才 ~120 字节），是为了让这个列表
 *    始终能被一眼看完 —— 存到 30 个就退化成另一个"选不出来"的问题，
 *    和预设从 10 砍到 8 是同一个理由。满了显式告诉用户"先删一个"，
 *    而不是静默丢掉最早的（静默丢失比拒绝更让人恼火）。
 * 2. **只存外观，不存身份**。id 是生成的时间戳 + 序号，不参与任何比对；
 *    重名允许（用户可能想存"深绿 v1 / v2"），用 id 区分。
 * 3. **读取必须能容错**。localStorage 里的东西可能是上个版本写的、被手改过、
 *    或被别的标签页写坏 —— 任何一条不合法就整条丢弃，绝不让坏数据把
 *    主题弹窗搞崩（这是"用户外观丢失"最难受的一种）。
 */
import { normalizeHex } from "./color.ts";
import type { Prefs, ThemeMode } from "./prefs.ts";

/** 一条自定义预设能存的外观字段（刻意比 Preset 少：不存 accent/washes，
 *  那是"某个内置预设想表达的彩色"，用户自定义时没有这个输入界面） */
export interface CustomPreset {
  id: string;
  name: string;
  mode: ThemeMode;
  /** 三段渐变 [起, 中, 止] */
  colors: [string, string, string];
  angle: number;
  compColor: string;
  compAlpha: number;
  /** 保存时的壁纸透明度（预设卡上要显示 "62%" 这类读数） */
  createdAt: number;
}

export const MAX_CUSTOM_PRESETS = 6;
export const CUSTOM_PRESETS_KEY = "pm-theme.customPresets";
export const CUSTOM_PRESET_NAME_MAX = 12;

/** 从当前偏好里摘出"可被预设表达"的那部分 */
export function customPresetFromPrefs(
  p: Pick<Prefs, "mode" | "gradFrom" | "gradMid" | "gradTo" | "gradAngle" | "compColor" | "compAlpha">,
  name: string,
  now = Date.now(),
): CustomPreset {
  return {
    id: `cp-${now}-${Math.random().toString(36).slice(2, 6)}`,
    name: normalizeName(name),
    mode: p.mode,
    colors: [normalizeHex(p.gradFrom), normalizeHex(p.gradMid), normalizeHex(p.gradTo)],
    angle: clampAngle(p.gradAngle),
    compColor: normalizeHex(p.compColor),
    compAlpha: clampAlpha(p.compAlpha),
    createdAt: now,
  };
}

export function normalizeName(raw: string): string {
  const t = (raw ?? "").replace(/\s+/g, " ").trim().slice(0, CUSTOM_PRESET_NAME_MAX);
  return t || "未命名预设";
}

function clampAngle(v: unknown): number {
  const n = typeof v === "number" && Number.isFinite(v) ? v : 165;
  return Math.round(Math.min(360, Math.max(0, n)));
}

function clampAlpha(v: unknown): number {
  const n = typeof v === "number" && Number.isFinite(v) ? v : 0.62;
  return Math.round(Math.min(1, Math.max(0, n)) * 100) / 100;
}

/** 单条容错归一化；不合法返回 null（整条丢弃） */
export function normalizeCustomPreset(raw: unknown): CustomPreset | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const colors = Array.isArray(r.colors) ? r.colors : null;
  if (!colors || colors.length !== 3) return null;
  const hexes: string[] = [];
  for (const c of colors) {
    if (typeof c !== "string") return null;
    const hex = normalizeHex(c, "");
    if (!hex) return null;
    hexes.push(hex);
  }
  const id = typeof r.id === "string" && r.id.trim() ? r.id.trim() : "";
  if (!id) return null;
  return {
    id,
    name: normalizeName(typeof r.name === "string" ? r.name : ""),
    mode: r.mode === "light" ? "light" : "dark",
    colors: [hexes[0], hexes[1], hexes[2]],
    angle: clampAngle(r.angle),
    compColor: normalizeHex(typeof r.compColor === "string" ? r.compColor : "#2a2630"),
    compAlpha: clampAlpha(r.compAlpha),
    createdAt: typeof r.createdAt === "number" && Number.isFinite(r.createdAt) ? r.createdAt : 0,
  };
}

/** 读取全部（容错 + 去重 id + 截断到上限） */
export function readCustomPresets(
  getItem: (k: string) => string | null = (k) =>
    typeof localStorage === "undefined" ? null : localStorage.getItem(k),
): CustomPreset[] {
  try {
    const raw = JSON.parse(getItem(CUSTOM_PRESETS_KEY) ?? "[]");
    if (!Array.isArray(raw)) return [];
    const out: CustomPreset[] = [];
    const seen = new Set<string>();
    for (const item of raw) {
      const p = normalizeCustomPreset(item);
      if (!p || seen.has(p.id)) continue;
      seen.add(p.id);
      out.push(p);
      if (out.length >= MAX_CUSTOM_PRESETS) break;
    }
    return out;
  } catch {
    return [];
  }
}

/** 追加一条；已满返回 { ok:false } 而不是丢旧的（静默丢失比拒绝更恼火） */
export function addCustomPreset(
  list: CustomPreset[],
  preset: CustomPreset,
): { ok: boolean; list: CustomPreset[] } {
  if (list.length >= MAX_CUSTOM_PRESETS) return { ok: false, list };
  return { ok: true, list: [...list, preset] };
}

export function removeCustomPreset(list: CustomPreset[], id: string): CustomPreset[] {
  return list.filter((p) => p.id !== id);
}
