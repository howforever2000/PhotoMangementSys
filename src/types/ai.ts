/** 创意工坊 · AI 创作类型（FEAT-152 → FEAT-196 供应商库）
 *
 * 与 Rust `ai_settings.rs` / `ai_studio.rs` 及 Python `ai_studio/schemas.py` 三方一一对应
 * （均 camelCase）。apiKey 后端只回传掩码，保存时留空/掩码表示「沿用旧密钥」。
 *
 * ## FEAT-196 的形状变化
 *
 * 旧形状是三个能力位各自内联 `baseUrl + apiKey + model` —— 同一家服务商被三个
 * 能力位复用时要把地址和密钥填三遍。新形状拆成两层：
 *   - `providers`：供应商库（服务商 + 凭证），只存一份，可被多个能力位共用；
 *   - `vision/edit/upscale`：能力位 → `{providerId, model}` 的绑定。
 * 模型名留在绑定上而不是供应商上：同一家服务商在「图像理解」和「图像编辑」
 * 用不同模型是常态。
 */

/** 能力位（不是供应商）：决定某个工具该用哪一组模型 */
export type AiProviderKind = "vision" | "edit" | "upscale";

/** 供应商库条目（读回时 apiKey 是掩码） */
export interface AiProviderEntryView {
  /** 稳定 id（`p1`/`p2`…）；新建但未保存的条目为空串 */
  id: string;
  /** 展示名；空时后端回落 baseUrl 的 host */
  name: string;
  baseUrl: string;
  /** 掩码（前 4 + … + 后 4），不是明文 */
  apiKey: string;
  hasKey: boolean;
}

/** 能力位绑定：指向某个供应商 + 该能力位要用的模型名 */
export interface AiBindingView {
  /** 供应商 id；空 = 该能力位未配置 */
  providerId: string;
  model: string;
}

export interface AiSettings {
  providers: AiProviderEntryView[];
  vision?: AiBindingView | null;
  edit?: AiBindingView | null;
  upscale?: AiBindingView | null;
}

/** 供应商库条目（写入）：apiKey 留空或掩码 = 沿用已保存密钥 */
export interface AiProviderEntryInput {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
}

export interface AiBindingInput {
  providerId: string;
  model: string;
}

export interface AiSettingsInput {
  providers: AiProviderEntryInput[];
  vision?: AiBindingInput | null;
  edit?: AiBindingInput | null;
  upscale?: AiBindingInput | null;
}

export interface AiTestResult {
  ok: boolean;
  message: string;
  latencyMs: number;
}

/**
 * AI 作业工具 id：由 `op_catalog.json`（经 `utils/toolCatalog.ts`）派生。
 * 不再手抄联合类型 —— 新增工具只改那一份 JSON，这里自动跟上。
 */
export type AiTool = string;

/** 计费规则（FEAT-152 / FEAT-157 前端本地保存，仅用于「费用预估」展示，不参与后端结算） */
export interface AiBillingRule {
  /** 币种符号，如 ¥ / $ */
  currency: string;
  /** 各模型（按能力位：图像理解 / 图像编辑 / 图像超分）「每张生成」单价；0 表示未配置 */
  modelPrices: Record<AiProviderKind, number>;
  /** 可选备注（例如所用模型、结算口径） */
  note?: string;
}

export type AiJobStatusKind = "queued" | "running" | "done" | "failed" | "cancelled";

/** 结构化步骤事件（FEAT-168）：后端 jobs.py 的 steps[]，替代自由文本 stage。 */
export interface RunStep {
  index: number;
  total: number;
  op: string;
  label: string;
  status: "running" | "done" | "skipped" | "failed";
  ms: number;
  issues?: string[];
}

/** 大模型/配方的受约束计划，与 Python `pipeline.validate_plan` 输出同形。 */
export interface RunPlan {
  intent: string;
  steps: Array<{ op: string; params: Record<string, unknown>; optional?: boolean }>;
}

/** 复核结果：verdict 三态，unknown 不冒充通过（旧版 fail-open 已修）。 */
export interface Verification {
  verdict: "pass" | "fail" | "unknown";
  score?: number;
  dims?: Record<string, number>;
  issues: string[];
}

export interface AiJobMeta {
  plans?: RunPlan[];
  verifications?: Verification[];
  steps?: RunStep[];
  recipeId?: string;
  stats?: Record<string, number>;
  [key: string]: unknown;
}

export interface AiJobStatus {
  jobId: string;
  status: AiJobStatusKind;
  progress: number;
  stage: string;
  message: string;
  outputs: string[];
  warnings: string[];
  error?: string | null;
  /** 结构化步骤进度；旧工作流不传时为空数组（后端 jobs.py 保证） */
  steps?: RunStep[];
  meta?: AiJobMeta;
}

export interface AiRunRequest {
  tool: AiTool;
  paths: string[];
  prompt: string;
  outputDir: string;
  options?: Record<string, unknown>;
}

/* ---------------- 本地增强（FEAT-163）资源状态 ---------------- */

export interface LocalModelEntry {
  id: string;
  name: string;
  file: string;
  /** 期望大小（MB），仅用于展示 */
  sizeMb: number;
  /** 下载地址（GitHub release） */
  url: string;
  /** 是否核心模型（缺失即无法使用对应能力） */
  required: boolean;
  path: string;
  exists: boolean;
  sizeBytes: number;
}

export interface LocalLutEntry {
  id: string;
  name: string;
  file: string;
  exists: boolean;
}

export interface LocalModelsStatus {
  models: LocalModelEntry[];
  luts: LocalLutEntry[];
  modelsDir: string;
  lutsDir: string;
}
