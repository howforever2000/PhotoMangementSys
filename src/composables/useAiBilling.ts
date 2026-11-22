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
import type { AiBillingRule, AiProviderKind, AiSettings, AiTool } from "../types/ai";
import { costModelOf, toolLabel } from "../utils/toolCatalog.ts";
import { originOfTool, type OriginInfo } from "../utils/modelOrigin.ts";
import { trace } from "../utils/trace.ts";

const STORAGE_KEY = "aiStudio:billing:v2";
const LEGACY_STORAGE_KEY = "aiStudio:billing:v1";

export const AI_MODELS: AiProviderKind[] = ["vision", "edit", "upscale"];

/**
 * 工具中文名：由 `op_catalog.json` 派生（FEAT-168）。
 * 原来的手写 Record 是死导出 + 加工具必漏改，已删除。用 Proxy 保留「按 key 取值」
 * 的调用方式而不冻结一份快照，新增工具自动有名字。
 */
export const AI_TOOL_LABELS: Record<string, string> = new Proxy({} as Record<string, string>, {
  get: (_t, key: string) => (typeof key === "string" ? toolLabel(key) : undefined),
  ownKeys: () => [],
  getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }),
});

/** 模型能力位中文名（计费填写用） */
export const AI_MODEL_LABELS: Record<AiProviderKind, string> = {
  vision: "图像理解",
  edit: "图像编辑",
  upscale: "图像超分",
};

/**
 * 工具 → 所用模型（决定费用预估取哪个模型的单价）。
 * 本机工具一律不计费（local-free）；云端工具按既有口径：
 * 智能修图/手账走编辑（规划的视觉调用在工具内单独估算）。
 * 画质增强已切本机 Real-ESRGAN（local-free）；云端超分算子（链上步 / 模型理解
 * 上采样）的费用走 smart_edit 的 stats 口径，不在这里登记。
 */
const CLOUD_TOOL_MODEL: Record<string, AiProviderKind> = {
  smart_edit: "edit",
  journal: "edit",
  beauty: "edit",
  style_transfer: "edit",
};

export const TOOL_MODEL: Record<string, AiProviderKind> = new Proxy(
  {} as Record<string, AiProviderKind>,
  {
    get: (_t, key: string) =>
      typeof key === "string" && costModelOf(key) === "cloud"
        ? CLOUD_TOOL_MODEL[key] ?? "edit"
        : "edit",
    ownKeys: () => [],
    getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true }),
  },
);

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

/**
 * 某工具的每张单价：本机工具恒为 0，不展示费用。
 *
 * 判据与来源标注同源（`utils/modelOrigin`），不在两处各判一次：
 *   - 走供应商且地址非回环（`cloud-api`）→ 按登记单价估算；
 *   - 纯算法 / 自带模型 / 走回环地址的 API（`api-local`）→ 不计费。
 *
 * 2026-11-22 定调：供应商按**地址**分成「API 模型 · 本地」与「API 模型 · 云端」，
 * 本地那档免费（请求只到本机），只有云端那档才展示金额。
 *
 * `settings` 缺省时退回旧口径（只按工具判），保证没传设置的调用点行为不变。
 */
function unitPrice(tool: AiTool, settings?: AiSettings | null): number {
  if (costModelOf(tool) !== "cloud") return 0;
  if (settings) {
    const info = originOfTool(tool, settings);
    if (!info.billable) return 0;
  }
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
  /** 来源判定（FEAT-199）：决定文案说「按量计费」还是「本机免费」 */
  origin: OriginInfo;
}

/**
 * 估算一次作业的费用：张数 × 该工具所用模型的每张单价。
 *
 * `settings` 只用于判「这个工具算不算出网计费」（走供应商且地址非回环才算），
 * 不参与计价本身 —— 计费规则仍由用户在「计费」里登记（FEAT-157 的结构不变）。
 */
const estimate = trace(
  "aiBilling.estimate",
  function estimate(
    tool: AiTool,
    count: number,
    settings?: AiSettings | null,
  ): AiCostEstimate {
    const model = TOOL_MODEL[tool];
    const origin = originOfTool(tool, settings);
    const unit = unitPrice(tool, settings);
    const n = Math.max(0, Math.floor(Number(count) || 0));
    return {
      tool,
      model,
      count: n,
      unit,
      total: unit * n,
      currency: rule.value.currency || "¥",
      configured: unit > 0,
      origin,
    };
  },
);

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
