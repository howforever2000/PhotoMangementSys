/**
 * 创意工坊「单步工具参数记忆」—— 持久层（读 / 写 / 校验 / 版本）
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么单独一个文件
 * ──────────────────────────────────────────────────────────────────────────
 * 工坊 13 个工具面板的参数原本都是组件内 `ref(默认值)`：`Workshop.vue` 用
 * `<component :is>` 切换工具（无 keep-alive），切走即卸载，回来就回到默认值。
 * 「记住上次调好的滑条」是一组内聚的读写+校验职责，不该在 6 个面板里各写一遍
 * （CS1：持久层不承载编排与 UI，面板只负责绑定）。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么按工具 id 分桶
 * ──────────────────────────────────────────────────────────────────────────
 * `strength` / `scale` / `look` 这些键在不同工具里语义与量纲都不同
 * （有的 0~100 百分数，有的 0~1 比例）。扁平存一张表必然互相串，所以按
 * `tool.id`（与 `op_catalog.json` 的 tools[].id 同域，见 utils/toolCatalog.ts）
 * 分桶，一个工具一份。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么要有「记哪些字段」的白名单
 * ──────────────────────────────────────────────────────────────────────────
 * 需求口径：只记**用户真调过的量**。肤色（原肤色/白皙档）、饱和度这类
 * 「中性值 = 不处理」或纯风格档不记 —— 记了反而会把「这次想从头调」的用户
 * 锁在上一次的档位上。白名单是本需求的**唯一真相点**：以后增减字段只改这里，
 * 面板与存储层都不用动。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么恢复时必须校验 + 夹取
 * ──────────────────────────────────────────────────────────────────────────
 * localStorage 里的东西可能是：手改过的、上个版本滑条范围写的、枚举常量
 * 已经改名留下的旧档位。越界值直接交给后端会被拒，甚至让整批作业失败。
 * 所以恢复时按**默认值的类型**校验，数值再按调用方给的当前控件范围夹取，
 * 枚举按调用方给的允许值过滤；任何一项不合格就退回默认值（不半信半疑地用）。
 *
 * 存法：单个键 `pm-tool-params`，值是 `{ v, tools: { <toolId>: { ... } } }`。
 * 工具数量少、单份 JSON 便于整体迁移与清空；版本不匹配就整份丢弃（宁可回默认，
 * 也不要把半旧字段灌进界面）。要升版本时在这里加迁移分支，不要直接改 v。
 */

/** localStorage 键：一份 JSON 存全部工具 */
export const TOOL_PARAMS_KEY = "pm-tool-params";
/** 结构版本：不匹配即整份丢弃（升降级都安全，代价只是回到默认值） */
export const TOOL_PARAMS_VERSION = 1;

/**
 * 每个工具要记住哪些参数 —— **唯一真相点**
 *
 * 口径：滑条强度类 + 明确的形变档（瘦脸/大眼）+ 胶片/调色风格档；
 * 不记：肤色、饱和度这类中性或风格档，以及 prompt / 输出目录 / 素材路径。
 */
export const REMEMBERED_FIELDS: Record<string, readonly string[]> = {
  /** 自动调色：只记力度（风格档不记） */
  auto_grade: ["strength"],
  /** 胶片模拟：两条强度线 + 颗粒/暗角 + 胶片档与调色档（用户明确要记风格档） */
  film_sim: ["lut", "strength", "look", "grade", "gradeStrength", "grain", "vignette"],
  /** 参考图调色：只记强度（保护肤色开关按「肤色不记」的口径排除） */
  color_transfer: ["strength"],
  /** 人像形变：瘦脸 / 大眼 / 眼睛提亮 —— 用户点名要记 */
  face_reshape: ["slim", "eyes", "eyeBright"],
  /** 本地美颜：档位 + 保留皮肤质感（肤色排除） */
  local_beauty: ["level", "texture"],
  /** 智能修图：渲染步数 + 上采样倍率（prompt / 风格 / 反思不记） */
  smart_edit: ["renderSteps", "upsampleScale"],
};

/** 恢复参数时的额外约束：数值夹取范围、枚举允许值（都由调用方按当前控件给） */
export interface RememberOptions {
  /** 字段 → [min, max]，数值超出即夹到边界 */
  clamp?: Record<string, readonly [number, number]>;
  /** 字段 → 允许值集合，不在集合内即退回默认值 */
  allowed?: Record<string, readonly unknown[]>;
}

/** 落盘结构 */
export interface ToolParamsBlob {
  v: number;
  tools: Record<string, Record<string, unknown>>;
}

/** 只依赖三个方法，便于单测注入假存储 */
export type ParamsStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** 该工具是否记这个字段（面板/调用方判断用，避免各处再抄一遍白名单） */
export function isRememberedField(toolId: string, key: string): boolean {
  return (REMEMBERED_FIELDS[toolId] ?? []).includes(key);
}

/**
 * 参数是否仍然全是默认值（只比白名单字段）
 *
 * 用途：只「访问过」没调过的工具不该在存储里留一条记录 —— 记的全是默认值等于没记，
 * 却会让「恢复默认」看起来没生效（记录还在），也让存储里堆满无意义的空条目。
 */
export function differsFromDefaults(
  toolId: string,
  values: Record<string, unknown>,
  defaults: Record<string, unknown>,
): boolean {
  for (const key of REMEMBERED_FIELDS[toolId] ?? []) {
    if (key in values && values[key] !== defaults[key]) return true;
  }
  return false;
}

/**
 * 把一份来路不明的参数收敛成「可安全使用的一份」：
 * 只保留白名单内、类型正确、落在范围内、枚举合法的字段，其余一律回默认值。
 */
export function normalizeToolParams(
  toolId: string,
  raw: unknown,
  defaults: Record<string, unknown>,
  opts: RememberOptions = {},
): Record<string, unknown> {
  const out: Record<string, unknown> = { ...defaults };
  if (!raw || typeof raw !== "object") return out;
  const rec = raw as Record<string, unknown>;
  for (const key of REMEMBERED_FIELDS[toolId] ?? []) {
    // 面板没有的字段不认（schema 改过名 / 白名单写错都能自愈）
    if (!(key in defaults) || !(key in rec)) continue;
    const v = coerceOne(rec[key], defaults[key], opts.clamp?.[key], opts.allowed?.[key]);
    if (v !== undefined) out[key] = v;
  }
  return out;
}

/** 单字段校验：不合格返回 undefined（调用方保留默认值） */
function coerceOne(
  v: unknown,
  def: unknown,
  clampRange?: readonly [number, number],
  allowed?: readonly unknown[],
): unknown {
  if (typeof def === "number") {
    if (typeof v !== "number" || !Number.isFinite(v)) return undefined;
    if (!clampRange) return v;
    return Math.min(clampRange[1], Math.max(clampRange[0], v));
  }
  if (typeof def === "boolean") {
    return typeof v === "boolean" ? v : undefined;
  }
  if (typeof def === "string") {
    if (typeof v !== "string" || !v) return undefined;
    if (allowed && !allowed.includes(v)) return undefined;
    return v;
  }
  return undefined;
}

/** 解析存储；显式传入（含 null）时以传入为准，便于单测与「无 localStorage 环境」 */
function resolveStorage(storage?: ParamsStorage | null): ParamsStorage | null {
  if (storage !== undefined) return storage;
  return typeof localStorage === "undefined" ? null : localStorage;
}

/** 读整份 blob：坏 JSON / 版本不符 / 结构不对 → 空表（绝不半读） */
export function readBlob(storage?: ParamsStorage | null): ToolParamsBlob {
  const s = resolveStorage(storage);
  const empty: ToolParamsBlob = { v: TOOL_PARAMS_VERSION, tools: {} };
  if (!s) return empty;
  try {
    const parsed = JSON.parse(s.getItem(TOOL_PARAMS_KEY) ?? "null") as unknown;
    if (!parsed || typeof parsed !== "object") return empty;
    const rec = parsed as Record<string, unknown>;
    if (rec.v !== TOOL_PARAMS_VERSION) return empty;
    const tools = rec.tools;
    if (!tools || typeof tools !== "object") return empty;
    return { v: TOOL_PARAMS_VERSION, tools: tools as Record<string, Record<string, unknown>> };
  } catch {
    return empty;
  }
}

/** 写整份 blob：配额满/隐私模式之类写不进去时静默放弃（记忆是增强，不该挡住业务） */
function writeBlob(blob: ToolParamsBlob, storage?: ParamsStorage | null): void {
  const s = resolveStorage(storage);
  if (!s) return;
  try {
    s.setItem(TOOL_PARAMS_KEY, JSON.stringify(blob));
  } catch {
    /* 配额不足：只丢记忆，不影响本次调参 */
  }
}

/** 读某工具的参数（已归一化，可直接和默认值合并） */
export function loadToolParams(
  toolId: string,
  defaults: Record<string, unknown>,
  opts: RememberOptions = {},
  storage?: ParamsStorage | null,
): Record<string, unknown> {
  const blob = readBlob(storage);
  return normalizeToolParams(toolId, blob.tools[toolId], defaults, opts);
}

/** 写某工具的参数：只存白名单内的字段，不覆盖其它工具 */
export function saveToolParams(
  toolId: string,
  values: Record<string, unknown>,
  storage?: ParamsStorage | null,
): void {
  const fields = REMEMBERED_FIELDS[toolId];
  if (!fields?.length) return;
  const keep: Record<string, unknown> = {};
  for (const key of fields) {
    if (key in values) keep[key] = values[key];
  }
  if (!Object.keys(keep).length) return;
  const blob = readBlob(storage);
  blob.tools[toolId] = keep;
  writeBlob(blob, storage);
}

/** 清掉某工具的记忆（「恢复默认」用）：其余工具不受影响；一个不剩就把键整个删掉 */
export function clearToolParams(toolId: string, storage?: ParamsStorage | null): void {
  const blob = readBlob(storage);
  if (!(toolId in blob.tools)) return;
  delete blob.tools[toolId];
  if (!Object.keys(blob.tools).length) {
    // 空表没有信息量，留着只会让「恢复默认」看起来像没生效（存储里还有一条键）
    try {
      resolveStorage(storage)?.removeItem(TOOL_PARAMS_KEY);
    } catch {
      /* 删不掉就留着空表，行为不受影响 */
    }
    return;
  }
  writeBlob(blob, storage);
}
