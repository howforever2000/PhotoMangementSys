/**
 * 配方（Recipe）类型 —— 与 `python/agent/recipes/catalog.json` 逐字对应。
 *
 * 为什么前端也要有一份结构：下拉、参数控件、高级区都要按配方类型切换，
 * 靠 `any` 会让"这个配方到底有没有版式"变成运行期猜谜。
 *
 * 数据来源是 **Rust 的 `agent_recipes` 命令**（`include_str!` 编进二进制）
 * —— 侧车没起来时也能渲染下拉，且不存在第二份真相。
 */

export type RecipeStatus = "ready" | "planned" | "needs_key";
export type RecipeEngine = "local" | "remote";

export interface SlotSpec {
  type: "enum" | "text";
  values?: string[];
  default?: string;
  maxLen?: number;
  hint?: string;
}

export interface PromptTemplateSpec {
  id: string;
  version: number;
  engine: RecipeEngine;
  anchor: string;
  slots: Record<string, SlotSpec>;
  skeleton: string;
  negative?: string;
  maxChars?: number;
}

export interface RunSpec {
  deliverable: "single_image" | "card_images" | "card_pdf" | "bundle";
  scope: { mode: string };
  style?: { preset: string; intensity: number; engine: RecipeEngine };
  retouch?: { faces: boolean; strength: "natural" | "mild" | "strong" };
  subject?: { mode: "contain" | "extract"; background: string };
  layout?: { template: string; title?: string; subtitle?: string };
  export?: {
    formats: string[];
    maxEdge?: number;
    targetKb?: number;
    crop?: string;
    pageSize?: string;
    maxPages?: number;
    overwrite?: boolean;
  };
  budget?: { maxCost: number; maxCalls: number };
}

export interface Recipe {
  id: string;
  group: string;
  icon: string;
  label: string;
  desc?: string;
  status: RecipeStatus;
  /** local_rule_mapped | remote_prompt | no_prompt */
  engine_behavior: string;
  p1_path: string;
  runSpec: RunSpec;
  /** 可以是内联对象，也可以是 catalog.templates 里的 id */
  promptTemplate?: string | PromptTemplateSpec;
}

export interface RecipeCatalog {
  version: number;
  templates: Record<string, PromptTemplateSpec>;
  recipes: Recipe[];
}

/** 解析配方的提示词模板（catalog 里用 id 引用，避免同一模板抄多份）。 */
export function resolveTemplate(cat: RecipeCatalog | null, r: Recipe): PromptTemplateSpec | undefined {
  const t = r.promptTemplate;
  if (!t) return undefined;
  if (typeof t === "object") return t;
  return cat?.templates?.[t];
}

/** 是否必须配置模型服务才能跑（决定「开始」是否灰掉 + 文案）。 */
export function needsProvider(r: Recipe): boolean {
  if (r.runSpec.style?.engine === "remote") return true;
  if (r.runSpec.subject?.mode === "extract") return true;
  return false;
}

/** 分组小标（下拉里的分隔标题）。 */
export function groupLabel(group: string): string {
  const map: Record<string, string> = {
    "A.style": "风格",
    "B.retouch": "人像",
    "C.card": "卡片 / 手账",
    "D.batch": "批量",
    "P.process": "处理",
  };
  return map[group] ?? group;
}

/** 状态的用户可见说明（planned / needs_key 不做假入口）。 */
export function statusHint(r: Recipe): string {
  if (r.status === "planned") return "规划中，暂不可用";
  if (r.status === "needs_key") return "需先配置模型服务";
  return "";
}

/** 高级区里判断该显示「目标提示词」还是「执行参数预览」。 */
export function showsPrompt(r: Recipe): boolean {
  const engine = r.runSpec.style?.engine;
  if (engine === "remote") return true;
  // 不消费提示词的配方（retouch / card / compress）也不显示提示词
  return false;
}
