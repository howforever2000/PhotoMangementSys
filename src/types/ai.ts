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

export type AiTool = "journal" | "beauty" | "style_transfer" | "upscale";

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
