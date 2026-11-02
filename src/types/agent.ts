/**
 * 图像创作 Agent 的前端类型（docs/v5.md §3、§5）。
 *
 * 命名与 Rust 侧 `agent::job::AgentSnapshot` 逐字对应（serde `rename_all = "camelCase"`）。
 * 改动这里必须同步改 Rust 结构体 —— 这条契约是跨进程的，不是内部的。
 */

/** 终态 = done / error / cancelled / budget / handoff；其余都算"还在跑" */
export type AgentStatus =
  | "starting"
  | "running"
  | "done"
  | "error"
  | "cancelled"
  | "budget"
  | "handoff";

export const AGENT_TERMINAL: readonly AgentStatus[] = [
  "done",
  "error",
  "cancelled",
  "budget",
  "handoff",
];

export function isTerminal(status: AgentStatus | undefined): boolean {
  return !!status && AGENT_TERMINAL.includes(status);
}

/** 一步轨迹：每步的缩略图 / 模型 / 耗时 / 成本 / 说明 */
export interface AgentStep {
  idx: number;
  tool: string;
  model: string | null;
  latencyMs: number;
  cost: number;
  thumb: string | null;
  note: string | null;
}

/** 前端看到的 job 快照（真相在 Rust，不在 Python） */
export interface AgentSnapshot {
  jobId: number;
  recipeId: string;
  status: AgentStatus;
  /** 本次走的是哪条路径：P0 / P1a / P1b / P1c / P2 / P3 */
  p1Path: string | null;
  steps: AgentStep[];
  costTotal: number;
  callsUsed: number;
  /** 需要二次装配（扩大范围）时的描述；Rust 判定后另开新 job */
  needReplan: Record<string, unknown> | null;
  error: string | null;
  /** Python 侧是否确认过（false = 还在等受理，或已失联） */
  pythonSeen: boolean;
  startedAt: number;
  updatedAt: number;
}

/** 提交请求（字段只增不改；后续接配方/档位时在这里补） */
export interface AgentSubmitRequest {
  recipeId: string;
  /** 附加描述（空 = P0 纯预设路径，零成本零等待） */
  userMsg?: string;
  /** PhotoTray 手选的图（手选永远优先置顶） */
  paths?: string[];
  tier?: "draft" | "standard" | "hq" | "local";
  outputDir?: string | null;
}
