/** 创意工坊 · AI 创作类型（FEAT-152）
 *
 * 与 Rust `ai_settings.rs` / `ai_studio.rs` 及 Python `ai_studio/schemas.py` 三方一一对应
 * （均 camelCase）。apiKey 后端只回传掩码，保存时留空/掩码表示「沿用旧密钥」。
 */

export type AiProviderKind = "vision" | "edit" | "upscale";

export interface AiProviderView {
  baseUrl: string;
  /** 掩码（前 4 + … + 后 4），不是明文 */
  apiKey: string;
  model: string;
  hasKey: boolean;
}

export interface AiProvidersView {
  vision?: AiProviderView | null;
  edit?: AiProviderView | null;
  upscale?: AiProviderView | null;
}

export interface AiSettings {
  providers: AiProvidersView;
}

export interface AiProviderInput {
  baseUrl: string;
  apiKey: string;
  model: string;
}

export interface AiSettingsInput {
  providers: {
    vision?: AiProviderInput;
    edit?: AiProviderInput;
    upscale?: AiProviderInput;
  };
}

export interface AiTestResult {
  ok: boolean;
  message: string;
  latencyMs: number;
}

export type AiTool =
  | "journal"
  | "beauty"
  | "style_transfer"
  | "upscale"
  | "smart_edit"
  // 本地增强（FEAT-163）：纯本机推理，免费、离线
  | "film_sim"
  | "color_transfer"
  | "restore";

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

export interface AiJobStatus {
  jobId: string;
  status: AiJobStatusKind;
  progress: number;
  stage: string;
  message: string;
  outputs: string[];
  warnings: string[];
  error?: string | null;
  meta?: Record<string, unknown>;
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
