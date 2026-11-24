/**
 * 工作台「创意组合」步骤链 —— 纯计算层（FEAT-177 / FEAT-197）
 * ===================================================================
 * 职责只有一件事：**链条 ⇄ 执行计划** 的转换，以及围绕它的纯函数
 * （默认值、依赖排序、参数视图、摘要、配方导入）。
 *
 * 为什么单独一个文件：
 *   - 组件只负责画，store 只负责存；转换逻辑必须能被单测直接钉住
 *     （顺序错一步，后端 `pipeline.validate_plan` 会直接拒单）。
 *   - 链条的合法顺序由 `op_catalog.json` 的 `stage` / `requiresBefore` 决定，
 *     前端不在界面上给排序控件（本机算子是**全序**，人工排序没有合法自由度）。
 *
 * ## FEAT-197：每一步有自己的 uid
 *
 * 用户要「同一组件多次插入」（例如连做两次胶片模拟看叠加效果），所以 `op` 不再是
 * 唯一键 —— 每一步有稳定 `uid`，增删改一律按 uid 定位。连带影响：
 *   - `:key` 用 uid（用 op 会触发 Vue 重复 key）；
 *   - 单步微调的基线 / 「不处理」标记也按 uid 对齐；
 *   - 排序仍按 stage 稳定排序，同 stage 的多次插入保持用户插入顺序。
 *
 * 另外放开了算子来源：云端模型（美颜 / 滤镜迁移 / 云端超分）与本地组件都能进链条。
 * 预览只跑本机步（`preview.py` 会摘掉云端步并给提示），提交时云端步照常执行。
 *
 * 不发请求、不碰 store、不 import 任何组件。
 */
import recipesJson from "../../python/ai_studio/recipes.json" with { type: "json" };
import {
  OPS,
  TOOLS,
  defaultParams,
  opById,
  type OpSpec,
  type ParamSpec,
} from "./toolCatalog.ts";
import { AUTOGRADE_STYLES, CT_METHODS, UPSCALE_SCALES } from "./aiPresets.ts";
import type { RunPlan } from "../types/ai";

/** 链条里的一步：稳定 uid + 算子 id + 是否启用 + 原始参数（与 catalog 同值域） */
export interface WbChainStep {
  /** 稳定 id：增删改与 UI key 都按它定位（允许同一算子出现多次） */
  uid: string;
  op: string;
  enabled: boolean;
  params: Record<string, number | string | boolean>;
}

/** 创意组合的步骤上限（与后端 `pipeline.MAX_STEPS` 同一口径） */
export const MAX_CHAIN_STEPS = 10;

/** uid 计数器：模块级自增，保证同一次会话内不重复（测试可预测） */
let uidSeq = 0;

function nextUid(op: string): string {
  uidSeq += 1;
  return `${op}#${uidSeq}`;
}

/** 参数在界面上的形态：range 统一折成 0~100 的整数滑杆 */
export interface ChainParamView {
  key: string;
  label: string;
  kind: "range" | "select" | "reference";
  /** range：0~100 */
  min: number;
  max: number;
  slider: number;
  /** select 的可选项与当前值（字符串化，便于 v-model） */
  options: { value: string; label: string }[];
  value: string;
}

/**
 * 可进链条的原子组件。
 * 判据：catalog 里非 internal、且**有图像产物**（ocr 的产物是 txt，没有可比对的图）。
 * 云端与本地一视同仁 —— 预览层会摘掉云端步并给提示，出片时照常执行。
 */
export function chainableOps(): OpSpec[] {
  return OPS.filter((o) => !o.internal && o.id !== "ocr");
}

/** 该算子是不是云端（决定「本机免费 / 按模型计费」文案与预览能力） */
export function isCloudOp(op: string): boolean {
  return opById(op)?.backend === "cloud";
}

/** 链条里是否存在云端步（费用文案、预览提示共用的判据） */
export function chainHasCloud(steps: WbChainStep[]): boolean {
  return steps.some((s) => s.enabled && isCloudOp(s.op));
}

export function opSpec(op: string): OpSpec | undefined {
  return opById(op);
}

/**
 * 算子所属的展示分组。分组声明在**工具**上（`op_catalog.json` 的 tools[].group），
 * 算子本身没有 group；链条按「影调与滤镜 / 人像 / 画质 / 创作」分组时走这里查。
 */
export function opGroup(op: string): string {
  return TOOLS.find((t) => t.op === op)?.group ?? opById(op)?.group ?? "utility";
}

export function opStage(op: string): number {
  return opById(op)?.stage ?? 0;
}

/**
 * 算子在界面上的名字。
 * 优先取「映射到这个算子的工具名」——用户认识的是工坊里那个入口（智能调色 / 胶片模拟 /
 * 本地美颜 / 画质修复），不是算子内部名（自适应调色 / 分区磨皮 / 本机超分）。
 */
export function opLabel(op: string): string {
  const tool = TOOLS.find((t) => t.op === op);
  return tool?.label ?? opById(op)?.label ?? op;
}

/** 单步默认值：直接取 catalog 的 default，不手抄 */
export function defaultStep(op: string): WbChainStep {
  const spec = opById(op);
  return {
    uid: nextUid(op),
    op,
    enabled: true,
    params: defaultParams(spec) as WbChainStep["params"],
  };
}

/** 按依赖 stage 升序（稳定排序，同 stage 保持原顺序 —— 同算子多次插入靠这个保序） */
export function sortChain(steps: WbChainStep[]): WbChainStep[] {
  return [...steps].sort((a, b) => opStage(a.op) - opStage(b.op));
}

/** 该算子还能再加几步（受总上限约束） */
export function canAddOp(steps: WbChainStep[]): boolean {
  return steps.length < MAX_CHAIN_STEPS;
}

/**
 * 加入一步：**允许同一算子多次插入**（各步 uid 不同）。
 * 插到依赖位（按 stage 稳定排序），所以用户不用管先后。
 */
export function insertOp(steps: WbChainStep[], op: string): WbChainStep[] {
  if (!canAddOp(steps)) return steps;
  return sortChain([...steps, defaultStep(op)]);
}

/** 按 uid 删除（不是按 op：同名算子只删被点的那一步） */
export function removeOp(steps: WbChainStep[], uid: string): WbChainStep[] {
  return steps.filter((s) => s.uid !== uid);
}

export function setEnabled(steps: WbChainStep[], uid: string, enabled: boolean): WbChainStep[] {
  return steps.map((s) => (s.uid === uid ? { ...s, enabled } : s));
}

/** 写一个参数（值域与 catalog 一致，不在此处换算滑杆） */
export function setParam(
  steps: WbChainStep[],
  uid: string,
  key: string,
  value: number | string | boolean,
): WbChainStep[] {
  return steps.map((s) => (s.uid === uid ? { ...s, params: { ...s.params, [key]: value } } : s));
}

/**
 * 当前顺序是否满足 requiresBefore（正常情况下永远成立，用于防御与测试）。
 * 与后端 `pipeline.validate_plan` 同一口径：按「前置算子的首次出现位置」判断，
 * 这样同名算子重复出现时判定依然正确。
 */
export function orderIssues(steps: WbChainStep[]): string[] {
  const firstPos = new Map<string, number>();
  steps.forEach((s, i) => {
    if (!firstPos.has(s.op)) firstPos.set(s.op, i);
  });
  const out: string[] = [];
  steps.forEach((s, i) => {
    for (const dep of opById(s.op)?.requiresBefore ?? []) {
      const j = firstPos.get(dep);
      if (j !== undefined && j > i) {
        out.push(`${opLabel(s.op)} 必须排在 ${opLabel(dep)} 之后`);
      }
    }
  });
  return out;
}

/**
 * 链条 → 受约束计划。
 * 只包含**启用**的步；`intent` 是给后端 critic 的一句话说明（不是提示词）。
 */
export function chainPlan(steps: WbChainStep[]): RunPlan {
  return {
    intent: chainSummary(steps),
    steps: sortChain(steps.filter((s) => s.enabled)).map((s) => ({
      op: s.op,
      params: { ...s.params },
    })),
  };
}

/** 计划里是否有可以提交的步 */
export function planRunnable(steps: WbChainStep[]): boolean {
  return steps.some((s) => s.enabled);
}

/* ---------------- 参数视图（滑杆统一 0~100） ---------------- */

const RANGE_MAX_RAW = 1;

/**
 * 内置胶片 LUT 的 id → 中文名。
 * 单一真相源：步骤链与单步微调共用这一份；id 与 `python/ai_studio/config.LUTS` 对齐。
 * `local_film.lut` 在 `op_catalog.json` 里只有 id 枚举、没有中文标签，标签只在前端。
 */
const FILM_LUTS: { value: string; label: string }[] = [
  { value: "provia", label: "标准彩负" },
  { value: "velvia", label: "艳丽" },
  { value: "astia", label: "柔和" },
  { value: "classic_chrome", label: "纪事" },
  { value: "classic_neg", label: "经典负片" },
  { value: "eterna", label: "电影" },
  { value: "nostalgic_neg", label: "怀旧" },
];

/** 枚举标签：复用 aiPresets 的既有表，不另抄一份 */
function enumOptions(op: string, key: string): { value: string; label: string }[] {
  if (op === "local_film" && key === "lut") {
    return FILM_LUTS.map((o) => ({ value: o.value, label: o.label }));
  }
  if (op === "auto_grade" && key === "style") {
    return AUTOGRADE_STYLES.map((o) => ({ value: o.value, label: o.label }));
  }
  if ((op === "local_upscale" || op === "upscale") && key === "scale") {
    return UPSCALE_SCALES.map((o) => ({ value: String(o.value), label: o.label }));
  }
  if (op === "color_transfer" && key === "method") {
    return CT_METHODS.map((o) => ({ value: o.value, label: o.label }));
  }
  if (op === "beauty" && key === "style") {
    return [
      { value: "natural", label: "自然" },
      { value: "crisp", label: "清晰" },
      { value: "film", label: "胶片" },
    ];
  }
  if (op === "beauty" && key === "level") {
    return [
      { value: "light", label: "轻" },
      { value: "medium", label: "中" },
      { value: "strong", label: "强" },
    ];
  }
  if (op === "style_transfer" && key === "mode") {
    return [
      { value: "autograde", label: "一句话调色" },
      { value: "transfer", label: "按参考图迁移" },
    ];
  }
  return [];
}

/**
 * raw → 滑杆（0~100）。
 *
 * FEAT-206：开始尊重 `spec.min` —— 旧实现写死 `n / max * 100`，隐含「min 恒为 0」，
 * 所有既有参数（min:0）结果不变；但独立自然饱和度是**双向值域**（-0.25~+0.25），
 * 中性值 0 必须落在滑条**中间 50**（用户口径：不是压在最左的 0），否则
 * 「往右拉才加饱和、往左没东西」且默认位置看起来像「已经减到底」。
 */
function rawToSlider(spec: ParamSpec, raw: unknown): number {
  const n = typeof raw === "number" ? raw : Number(raw);
  if (!Number.isFinite(n)) return 100;
  const min = spec.min ?? 0;
  const max = spec.max ?? RANGE_MAX_RAW;
  const span = max - min || 1;
  return Math.max(0, Math.min(100, Math.round(((n - min) / span) * 100)));
}

function sliderToRaw(spec: ParamSpec, slider: number): number {
  const min = spec.min ?? 0;
  const max = spec.max ?? RANGE_MAX_RAW;
  const span = max - min || 1;
  const v = min + (Math.max(0, Math.min(100, slider)) / 100) * span;
  return Math.round(v * 1000) / 1000;
}

/** 某一步在界面上要渲染的参数列表（顺序即 catalog 声明顺序；`hidden` 的契约参数不渲染） */
export function paramViews(step: WbChainStep): ChainParamView[] {
  const spec = opById(step.op);
  if (!spec) return [];
  return Object.entries(spec.params).flatMap(([key, p]) => {
    if (p.hidden) return [];
    if (p.type === "enum") {
      const opts = enumOptions(step.op, key);
      const raw = String(step.params[key] ?? p.default);
      return {
        key,
        label: p.label,
        kind: "select" as const,
        min: 0,
        max: 100,
        slider: 0,
        options: opts,
        value: raw,
      };
    }
    if (p.type === "range") {
      return {
        key,
        label: p.label,
        kind: "range" as const,
        min: 0,
        max: 100,
        slider: rawToSlider(p, step.params[key] ?? p.default),
        options: [],
        value: "",
      };
    }
    // 参考图路径：界面上渲染成「选择参考图」按钮，不是只读文本
    if (p.type === "string" && key === "reference_path") {
      return {
        key,
        label: p.label,
        kind: "reference" as const,
        min: 0,
        max: 100,
        slider: 0,
        options: [],
        value: String(step.params[key] ?? p.default),
      };
    }
    // 其余 bool / string 参数按只读文本展示（云端算子的说明性字段）
    return {
      key,
      label: p.label,
      kind: "select" as const,
      min: 0,
      max: 100,
      slider: 0,
      options: [],
      value: String(step.params[key] ?? p.default),
    };
  });
}

/** 滑杆值 → 写回参数（0~100 → catalog 值域） */
export function applySlider(step: WbChainStep, key: string, slider: number): WbChainStep["params"] {
  const spec = opById(step.op)?.params[key];
  if (!spec) return step.params;
  return { ...step.params, [key]: sliderToRaw(spec, slider) };
}

/** 参数值 → 滑杆位置（用于初值） */
export function sliderOf(step: WbChainStep, key: string): number {
  const spec = opById(step.op)?.params[key];
  if (!spec) return 0;
  return rawToSlider(spec, step.params[key] ?? spec.default);
}

/**
 * 下拉选出来的字符串 → catalog 里的真实枚举值（`local_upscale.scale` 是数字 2/4，
 * 直接传 "2" 会被后端当字符串，夹取时静默回落默认值）。
 */
export function coerceEnum(op: string, key: string, raw: string): number | string {
  const spec = opById(op)?.params[key];
  const hit = (spec?.values ?? []).find((v) => String(v) === raw);
  return hit ?? raw;
}

/** 枚举参数的显示名 */
export function enumText(step: WbChainStep, key: string): string {
  const spec = opById(step.op)?.params[key];
  if (!spec) return "";
  const raw = String(step.params[key] ?? spec.default);
  return enumOptions(step.op, key).find((o) => o.value === raw)?.label ?? raw;
}

/* ---------------- 文案 ---------------- */

/**
 * 步骤摘要：界面上每一步右侧那行小字。
 * 只取前两个 range 参数，枚举单独处理（换滤镜比滑杆更值得先说）。
 * `hidden` 的枚举仍要显示（方法名是有效信息），`hidden` 的滑条/开关不占名额。
 */
export function stepSummary(step: WbChainStep): string {
  const parts: string[] = [];
  const spec = opById(step.op);
  if (!spec) return "";
  for (const [key, p] of Object.entries(spec.params)) {
    if (p.type === "enum") {
      const t = enumText(step, key);
      if (t) parts.push(t);
      continue;
    }
    if (p.hidden) continue;
    if (p.type === "range" && parts.length < 2) {
      parts.push(`${p.label} ${sliderOf(step, key)}%`);
    }
  }
  return parts.join(" · ");
}

/**
 * 整条链的一句话（用作 plan.intent 与界面提示）。
 * 同名算子多次出现时各自列一遍 —— 用户要看到「跑了两遍」这件事。
 */
export function chainSummary(steps: WbChainStep[]): string {
  const list = sortChain(steps.filter((s) => s.enabled));
  return list.length ? list.map((s) => opLabel(s.op)).join(" → ") : "自定义组合";
}

/* ---------------- 从配方起步 ---------------- */

interface RecipeLite {
  id: string;
  label: string;
  steps: Array<{ op: string; params?: Record<string, number | string | boolean | boolean> }>;
}

export const RECIPES: RecipeLite[] = (recipesJson as unknown as { recipes: RecipeLite[] }).recipes;

export function recipeById(id: string): RecipeLite | undefined {
  return RECIPES.find((r) => r.id === id);
}

/**
 * 配方 → 链条：配方本来就是「别人配好的步骤链」，所以可以直接接着改。
 * 参数用配方里的值（不是 catalog 默认值），否则「从配方起步」就等于什么都没带过来。
 */
export function chainFromRecipe(id: string): WbChainStep[] {
  const r = recipeById(id);
  if (!r) return [];
  return sortChain(
    r.steps.map((s) => ({
      uid: nextUid(s.op),
      op: s.op,
      enabled: true,
      params: { ...(defaultStep(s.op).params as Record<string, number | string | boolean>), ...(s.params ?? {}) },
    })),
  );
}

/* ---------------- 单步微调：基线 / 差量覆盖 / 复位 ---------------- */

/** 深拷贝一份链条：微调基线必须与用户正在编辑的那份互不共享 params 对象。 */
export function cloneSteps(steps: WbChainStep[]): WbChainStep[] {
  return steps.map((s) => ({ uid: s.uid, op: s.op, enabled: s.enabled, params: { ...s.params } }));
}

/**
 * 只把**用户改过**的参数抽成后端 `paramsOverride` 形状 `{op:{key:value}}`。
 *
 * 为什么不整条计划重发：配方里可能含后端会夹取/补默认的参数，整发等于替后端做决定；
 * 只发改动项则「没动过 = 完全走配方原值」，与后端 `apply_overrides` 的语义一致。
 * 基线里没有的键也算改动（未来 catalog 新增参数不会丢）。
 *
 * 按 uid 对齐基线（同名算子各算各的），但输出仍按 op 汇总成后端口径；
 * 同名算子多次出现时后写覆盖先写 —— 配方本身不含重复算子，这是已知边界。
 */
export function paramsOverride(
  steps: WbChainStep[],
  base: WbChainStep[],
): Record<string, Record<string, number | string | boolean>> {
  const baseMap = new Map(base.map((s) => [s.uid, s.params]));
  const out: Record<string, Record<string, number | string | boolean>> = {};
  for (const s of steps) {
    const before = baseMap.get(s.uid);
    for (const [key, value] of Object.entries(s.params)) {
      if (before && before[key] === value) continue;
      (out[s.op] ??= {})[key] = value;
    }
  }
  return out;
}

/**
 * 复位：传 `uid` 只复位那一步，否则全部复位。参数与启用状态都回到基线。
 * 复位不是「删掉这一步」，而是回到进入微调时冻结的配方默认值。
 */
export function restoreTune(steps: WbChainStep[], base: WbChainStep[], uid?: string): WbChainStep[] {
  const baseMap = new Map(base.map((s) => [s.uid, { enabled: s.enabled, params: s.params }]));
  return steps.map((s) => {
    if (uid && s.uid !== uid) return s;
    const before = baseMap.get(s.uid);
    return before ? { ...s, enabled: before.enabled, params: { ...before.params } } : s;
  });
}

/* ---------------- 单参数「不处理」 ---------------- */

/** 扁平键：`uid.key` → 是否不处理（按步定位，同名算子互不干扰） */
export type TuneOff = Record<string, boolean>;

export const tuneOffKey = (uid: string, key: string): string => `${uid}.${key}`;

/**
 * 「不处理」→ 真正下发到计划里的步骤。
 *
 * 语义（与界面一致）：
 *   - **滑杆（range）**：不设单独开关，拉到中性值就是不做 —— 既有可调 range 全部
 *     以 0 为中性（调色力度 / 胶片强度 / 颗粒 / 暗角 / 瘦脸 / 大眼 / 磨皮）；
 *     FEAT-206 起自然饱和度是**双向值域**（-0.25~+0.25），中性 0 落在滑条中间 50。
 *     0 只是一个普通参数值，因此数值参数不会让步骤消失；
 *   - **下拉（enum）**：LUT / 风格没有中性值，选「不处理」→ 整步不参与；
 *   - bool/string 当前算子族没有可调项，保持原值。
 *
 * 只返回启用中的步骤；排序交给 `tunePlan`。
 */
export function effectiveTuneSteps(steps: WbChainStep[], off: TuneOff): WbChainStep[] {
  const out: WbChainStep[] = [];
  for (const s of steps) {
    if (!s.enabled) continue;
    const spec = opById(s.op);
    const skipStep = Object.entries(spec?.params ?? {}).some(
      ([key, p]) => p.type === "enum" && Boolean(off[tuneOffKey(s.uid, key)]),
    );
    if (!skipStep) out.push({ ...s, params: { ...s.params } });
  }
  return out;
}

/**
 * 微调步骤 + 不处理标记 → 配方执行计划（与后端 `options.plan` 同形）。
 * 只要存在不处理标记，页面就改发这条计划；全都不处理时 steps 为空，由入口层拦下。
 */
export function tunePlan(steps: WbChainStep[], off: TuneOff, intent = ""): RunPlan {
  return {
    intent,
    steps: sortChain(effectiveTuneSteps(steps, off)).map((s) => ({
      op: s.op,
      params: { ...s.params },
    })),
  };
}
