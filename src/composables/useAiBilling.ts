/**
 * AI 创作计费规则（FEAT-152 / FEAT-157 · 前端展示层）
 *
 * 自 FEAT-157 起改为「按模型登记」：价格挂在模型能力位（图像理解 / 图像编辑 /
 * 图像超分）上，而不是按工具。同一模型被多个工具复用时只登记一次，运行区按
 * 「工具 → 所用模型 → 该模型单价 × 张数」自动预估。
 *
 * 仍只做「本地计费规则保存 + 运行前费用预估」：不改后端作业流程、不接支付、
 * 不做真实扣费。单价由用户自行填写（默认 0 = 未配置），金额一律为「预估」，
 * 实际以服务商账单为准。
 *
 * 数据存 localStorage（键 aiStudio:billing:v2）；读取时兼容旧版按工具的
 * aiStudio:billing:v1 并自动迁移，避免升级后丢已填数据。
 */
import { ref } from "vue";
import type { AiBillingRule, AiProviderKind, AiTool } from "../types/ai";
import { trace } from "../utils/trace.ts";

const STORAGE_KEY = "aiStudio:billing:v2";
const LEGACY_STORAGE_KEY = "aiStudio:billing:v1";

export const AI_TOOLS: AiTool[] = ["journal", "beauty", "style_transfer", "upscale"];
export const AI_MODELS: AiProviderKind[] = ["vision", "edit", "upscale"];

/** 工具中文名（运行区提示共用） */
export const AI_TOOL_LABELS: Record<AiTool, string> = {
  journal: "主题手账",
  beauty: "人物美颜",
  style_transfer: "滤镜迁移",
  upscale: "画质增强",
};

/** 模型能力位中文名（计费填写用） */
export const AI_MODEL_LABELS: Record<AiProviderKind, string> = {
  vision: "图像理解",
  edit: "图像编辑",
  upscale: "图像超分",
};

/**
 * 工具 → 所用模型（决定费用预估取哪个模型的单价）。
 * - 手账风格化 / 美颜 / 滤镜迁移 都走「图像编辑」出图；
 * - 画质增强走「图像超分」（未配置时后端回落编辑，这里按超分价展示）；
 * - 主题手账虽还会调「图像理解」写配文，但按约定只计图像编辑出图成本。
 */
export const TOOL_MODEL: Record<AiTool, AiProviderKind> = {
  journal: "edit",
  beauty: "edit",
  style_transfer: "edit",
  upscale: "upscale",
};

function defaultRule(): AiBillingRule {
  return {
    currency: "¥",
    modelPrices: { vision: 0, edit: 0, upscale: 0 },
  };
}

/** 归一化单价：非数字 / 负数 / 0 一律视为未配置（0） */
function toPrice(v: unknown): number {
  const n = Number(v);
  return Number.isFinite(n) && n > 0 ? n : 0;
}

/**
 * 归一化任意来源的规则对象：
 *  - 新版（含 modelPrices）→ 直接按模型读取；
 *  - 旧版（含 unitPrices，按工具）→ 迁移到按模型：edit 取三工具中任一非零价，
 *    upscale 取旧超分价，vision 旧版无 → 0。
 */
function normalize(raw: unknown): AiBillingRule {
  const base = defaultRule();
  if (!raw || typeof raw !== "object") return base;
  const obj = raw as Record<string, unknown>;
  const currency =
    typeof obj.currency === "string" && obj.currency.trim() ? obj.currency.trim() : base.currency;
  const note = typeof obj.note === "string" ? obj.note : undefined;

  if (obj.modelPrices && typeof obj.modelPrices === "object") {
    const mp = obj.modelPrices as Record<string, unknown>;
    return {
      currency,
      modelPrices: {
        vision: toPrice(mp.vision),
        edit: toPrice(mp.edit),
        upscale: toPrice(mp.upscale),
      },
      note,
    };
  }

  // 兼容旧版按工具单价（aiStudio:billing:v1）
  const up = (obj.unitPrices ?? {}) as Record<string, unknown>;
  return {
    currency,
    modelPrices: {
      vision: 0,
      edit: toPrice(up.beauty) || toPrice(up.style_transfer) || toPrice(up.journal),
      upscale: toPrice(up.upscale),
    },
    note,
  };
}

function read(): AiBillingRule {
  try {
    if (typeof localStorage === "undefined") return defaultRule();
    const raw = localStorage.getItem(STORAGE_KEY) ?? localStorage.getItem(LEGACY_STORAGE_KEY);
    return raw ? normalize(JSON.parse(raw)) : defaultRule();
  } catch {
    // 读失败（无存储 / JSON 损坏）→ 回退默认规则，不阻断界面
    return defaultRule();
  }
}

/** 模块级共享状态：所有组件看到同一份规则 */
const rule = ref<AiBillingRule>(read());

/** 保存计费规则（写 localStorage + 同步内存态）。入口参数与结果由 trace 记录。 */
const save = trace("aiBilling.save", function save(next: AiBillingRule): AiBillingRule {
  const normalized = normalize(next);
  rule.value = normalized;
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(normalized));
    }
  } catch {
    // 写失败（隐私模式 / 配额满）静默忽略，内存态仍然生效
  }
  return normalized;
});

/** 某模型的每张单价（0 = 未配置） */
function modelPrice(kind: AiProviderKind): number {
  const v = Number(rule.value.modelPrices?.[kind] ?? 0);
  return Number.isFinite(v) && v > 0 ? v : 0;
}

/** 某工具的每张单价：取其所用模型的单价 */
function unitPrice(tool: AiTool): number {
  return modelPrice(TOOL_MODEL[tool]);
}

export interface AiCostEstimate {
  tool: AiTool;
  /** 该工具所用的模型能力位 */
  model: AiProviderKind;
  count: number;
  unit: number;
  total: number;
  currency: string;
  /** 是否已配置单价（false 时界面提示去设置，不显示金额） */
  configured: boolean;
}

/** 估算一次作业的费用：张数 × 该工具所用模型的每张单价 */
const estimate = trace("aiBilling.estimate", function estimate(tool: AiTool, count: number): AiCostEstimate {
  const model = TOOL_MODEL[tool];
  const unit = modelPrice(model);
  const n = Math.max(0, Math.floor(Number(count) || 0));
  return {
    tool,
    model,
    count: n,
    unit,
    total: unit * n,
    currency: rule.value.currency || "¥",
    configured: unit > 0,
  };
});

/** 格式化金额：¥0.6 / ¥12.00（去掉多余尾零） */
function formatMoney(amount: number, currency?: string): string {
  const cur = currency || rule.value.currency || "¥";
  const safe = Number.isFinite(amount) ? amount : 0;
  const fixed = safe >= 1 ? safe.toFixed(2) : safe.toFixed(3);
  const trimmed = fixed.replace(/(\.\d*?)0+$/, "$1").replace(/\.$/, "");
  return `${cur}${trimmed}`;
}

/**
 * 判断错误文本是否属于「额度 / 权限不足」类，供结果面板给出充值/开通引导。
 * 覆盖后端归一后的中文提示与原始 403 关键字。
 */
export function isQuotaError(text?: string | null): boolean {
  const s = (text || "").toLowerCase();
  if (!s) return false;
  return (
    s.includes("insufficient_quota") ||
    s.includes("accessdenied") ||
    s.includes("unpurchased") ||
    s.includes("access to model denied") ||
    s.includes("额度") ||
    s.includes("未开通") ||
    s.includes("无使用权限")
  );
}

export function useAiBilling() {
  return { rule, save, modelPrice, unitPrice, estimate, formatMoney };
}
