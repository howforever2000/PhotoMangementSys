/**
 * 创意工坊工具/算子清单 · 前端适配层（FEAT-168）
 *
 * 唯一真相源是 `python/ai_studio/op_catalog.json`：Vite 在构建期把它内联进 bundle，
 * Python 服务没启动、模型没下载时左栏照样能渲染。本文件只做三件事：
 *   ① 把 JSON 收成带类型的视图（ToolDef / OpSpec / ParamSpec）
 *   ② 按展示口径分区（pattern: partition-shared-list-by-area —— 分区不改后端）
 *   ③ 暴露纯函数供组件与守卫测试使用，**不承载任何编排副作用**
 *
 * 新增工具只改那一份 JSON：这里、Rust 白名单、Python REGISTRY、spec hiddenimports
 * 都由它派生或由守卫测试兜住，不再手抄。
 */
import raw from "../../python/ai_studio/op_catalog.json" with { type: "json" };

export type Backend = "local" | "cloud";
export type ToolSurface = "primary" | "single" | "planned";
export type Runtime = "python" | "rust" | "none";
export type ParamKind = "enum" | "range" | "string" | "bool";
/** 单步工具的展示类型（FEAT-169）。意义见 op_catalog.json 的 groups 声明。 */
export type ToolGroup = "tone" | "portrait" | "quality" | "creative" | "utility" | (string & {});

export interface ParamSpec {
  type: ParamKind;
  label: string;
  default: string | number | boolean;
  min?: number;
  max?: number;
  step?: number;
  values?: Array<string | number>;
  /** 契约保留但界面不渲染（FEAT-220：参考图调色的旧参数只留兼容，不再暴露滑条）。 */
  hidden?: boolean;
}

export interface OpSpec {
  id: string;
  label: string;
  desc: string;
  backend: Backend;
  stage: number;
  module: string;
  params: Record<string, ParamSpec>;
  requires: Record<string, unknown>;
  requiresBefore: string[];
  internal?: boolean;
  /** 链条候选的展示分组（FEAT-206：无工具映射的纯算子也能声明归属，如自然饱和度→tone） */
  group?: string;
}

export interface ToolDef {
  id: string;
  label: string;
  desc: string;
  icon: string;
  runtime: Runtime;
  backend: Backend;
  module?: string;
  op?: string;
  costModel: string;
  surface: ToolSurface;
  /** 单步工具的展示分组；primary surface 固定为 "primary"（不进左栏分组） */
  group: ToolGroup;
  order: number;
  ready: boolean;
  providerPolicy: "none" | "vision+edit" | "edit" | "upscale|edit";
  params?: Record<string, ParamSpec>;
  note?: string;
  steps?: string[];
}

/** 分组元数据（来自 op_catalog.json 的 groups 声明，顺序即左栏顺序） */
export interface GroupDef {
  id: string;
  label: string;
  desc: string;
}

interface RawCatalog {
  version: number;
  groups: GroupDef[];
  ops: RawOp[];
  tools: RawTool[];
}
interface RawOp {
  id: string; label: string; desc: string; backend: Backend; stage: number;
  module: string; params: Record<string, ParamSpec>; requires: Record<string, unknown>;
  requiresBefore: string[]; internal?: boolean; group?: string;
}
interface RawTool {
  id: string; label: string; desc: string; icon: string; runtime: Runtime;
  backend: Backend; module?: string; op?: string; costModel: string;
  surface: ToolSurface; group: ToolGroup; order: number; ready: boolean;
  providerPolicy: ToolDef["providerPolicy"];
  params?: Record<string, ParamSpec>; note?: string; steps?: string[];
}

const data = raw as unknown as RawCatalog;

/** 通过声明式断言收窄 JSON 字面量，避免把整个文件手抄成 TS 常量（会重新漂移）。 */
function assertCatalog(v: RawCatalog): asserts v is RawCatalog {
  if (!Array.isArray(v.ops) || !Array.isArray(v.tools) || v.tools.length === 0) {
    throw new Error("op_catalog.json 结构非法：缺 ops/tools");
  }
  if (!Array.isArray(v.groups) || v.groups.length === 0) {
    throw new Error("op_catalog.json 结构非法：缺 groups（单步工具分组口径）");
  }
  const ids = v.tools.map((t) => t.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("op_catalog.json 工具 id 重复");
  }
  const groupIds = new Set(v.groups.map((g) => g.id));
  const opIds = new Set(v.ops.map((o) => o.id));
  for (const t of v.tools) {
    if (t.op && !opIds.has(t.op)) throw new Error(`工具 ${t.id} 引用了不存在的算子 ${t.op}`);
    if (t.surface !== "primary" && !groupIds.has(t.group)) {
      throw new Error(`工具 ${t.id} 的 group「${t.group}」未在 groups 中声明`);
    }
    if (t.runtime === "python" && !t.module?.startsWith("ai_studio.workflows.")) {
      throw new Error(`工具 ${t.id} 缺合法 workflows module`);
    }
  }
}
assertCatalog(data);

export const OPS: readonly OpSpec[] = data.ops as OpSpec[];
export const TOOLS: readonly ToolDef[] = data.tools as ToolDef[];
export const GROUPS: readonly GroupDef[] = data.groups as GroupDef[];

/** Python 工作流工具：Rust `ai_run_job` 认这些 id。 */
export type PythonToolId = string;
export const PYTHON_TOOL_IDS: readonly string[] = TOOLS.filter(
  (t) => t.runtime === "python",
).map((t) => t.id);

/** 工坊左栏条目：与 Python 工具 id 同域（不再有 local_reshape ↔ face_reshape 双 id）。 */
export type WorkshopToolId = string;
export const WORKSHOP_TOOL_IDS: readonly string[] = TOOLS.map((t) => t.id);

export function isPythonTool(id: string): boolean {
  return PYTHON_TOOL_IDS.includes(id);
}

/** AI 修图工作台「本地引擎」选项：纯本机的 Python 工具，按 order 排。 */
export function localToolDefs(): ToolDef[] {
  return TOOLS.filter((t) => t.surface === "single" && t.backend === "local").sort(
    (a, b) => a.order - b.order,
  );
}

/** ③ 单步工具：按**类型**分组展示（FEAT-169）。规划中项一律沉到「处理」组末尾。

为什么按类型而不是按本机/云端：运行位置是用户不关心的技术口径（同一件事
「本机」和「模型」各出现一次时，用户还得先理解后者才能选）。按「影调与滤镜 /
人像 / 画质 / 创作 / 处理」分，用户想的是「我要改什么」。
分组只是展示口径，不改变任何工具归属或后端能力。
*/
export function singleToolsByGroup(): Array<GroupDef & { items: ToolDef[] }> {
  const byOrder = (a: ToolDef, b: ToolDef) => a.order - b.order;
  const single = TOOLS.filter((t) => t.surface === "single");
  const planned = TOOLS.filter((t) => t.surface === "planned").sort(byOrder);
  const out: Array<GroupDef & { items: ToolDef[] }> = GROUPS.map((g) => ({
    ...g,
    items: single.filter((t) => t.group === g.id).sort(byOrder),
  }));
  if (planned.length) {
    // 规划中项挂到「处理」组末尾：它们本就是通用能力，且必须排在最后一屏。
    // 若清单里没有 utility 组，就单独补一个，保证规划项永远不会被静默丢掉。
    const utility = out.find((g) => g.id === "utility");
    if (utility) utility.items.push(...planned);
    else out.push({ id: "utility", label: "处理", desc: "", items: planned });
  }
  return out.filter((g) => g.items.length > 0);
}

/** 某工具所属分组的显示名（面包屑 / 标题副文案用）。 */
export function groupLabelOf(id: string): string {
  const g = toolById(id)?.group;
  return GROUPS.find((x) => x.id === g)?.label ?? "";
}

/** ① 一键出片的主入口工具。 */
export function primaryTool(): ToolDef {
  const t = TOOLS.find((x) => x.surface === "primary");
  if (!t) throw new Error("op_catalog.json 缺少 primary 工具");
  return t;
}

export function toolById(id: string): ToolDef | undefined {
  return TOOLS.find((t) => t.id === id);
}

export function opById(id: string): OpSpec | undefined {
  return OPS.find((o) => o.id === id);
}

/** 参数默认值：SchemaForm / useToolJob 共用，取代各处手写 default。 */
export function defaultParams(spec?: { params?: Record<string, ParamSpec> }): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, p] of Object.entries(spec?.params ?? {})) out[key] = p.default;
  return out;
}

/** 中文名：取代 useAiBilling 里手抄的 AI_TOOL_LABELS（死导出）。 */
export function toolLabel(id: string): string {
  return toolById(id)?.label ?? id;
}

/** 计费模型：本机一律 0 元，云端按模型能力位（保留既有计费口径）。 */
export function costModelOf(id: string): "local-free" | "cloud" {
  return toolById(id)?.costModel === "cloud" ? "cloud" : "local-free";
}
