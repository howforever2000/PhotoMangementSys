/**
 * 模型来源判定（FEAT-199）
 * ===================================================================
 * 回答用户的一句话问题：「这一步是本地模型，还是要调 API？」
 *
 * ## 为什么不能只看 tool.backend
 *
 * `op_catalog.json` 的 `backend` 描述的是**执行方式**（本机算子 / 调模型），
 * 不是**模型跑在哪**。两个方向都会判错：
 *   - 本机侧：智能调色 / 胶片模拟 / 参考图调色是纯算法 + LUT，**不跑任何模型**，
 *     与要下 340MB 模型的画质修复不是一回事；
 *   - 云端侧：FEAT-196 起「供应商库」允许把一个后端工具绑到本机
 *     Ollama / LM Studio / 本地图生图网关（baseUrl 是 127.0.0.1），这时照片其实
 *     没出本机、也不花钱，标「云端 API」是错的。
 *
 * ## 判据全部派生，不新增手写名单
 *   - 执行方式 ← catalog 的 `backend` 与 `requires.models`（`utils/toolCatalog`）
 *   - 模型位置 ← 设置里绑定供应商的 `baseUrl` 主机名（`utils/urlHost`）
 *   - 云端算子用哪个能力位 ← catalog 的 `requires.providers`（不是另抄一张表）
 *
 * 纯函数：不发请求、不读 store、不 import 组件。设置对象由调用方传入。
 */
import { opById, toolById } from "./toolCatalog.ts";
import { recipeById } from "./workbenchPlan.ts";
import { isLoopbackHost, hostOf } from "./urlHost.ts";
import type { AiProviderEntryView, AiProviderKind, AiSettings } from "../types/ai";

/** 四档来源：本机纯算法 / 本机模型（含绑到本机的本地服务）/ 云端 API / 还没配 */
export type ModelOrigin = "local-algo" | "local-model" | "cloud-api" | "unconfigured";

export interface OriginInfo {
  origin: ModelOrigin;
  /** 默认短标签（左栏角标、步骤标签用） */
  label: string;
  /** 更短的形态（横向工具条、极小空间） */
  short: string;
  /** 悬停说明：一句话讲清「照片出不出本机、花不花钱」 */
  detail: string;
  /** 是否可能产生费用 */
  billable: boolean;
  /** 照片是否会离开本机 */
  leavesDevice: boolean;
  /** 云端工具但绑到了本机地址（Ollama / LM Studio / 本地网关） */
  viaLocalApi: boolean;
}

const LABELS: Record<ModelOrigin, { label: string; short: string }> = {
  "local-algo": { label: "本机算法", short: "本机" },
  "local-model": { label: "本机模型", short: "本机" },
  "cloud-api": { label: "云端 API", short: "云端" },
  unconfigured: { label: "未配置模型", short: "未配置" },
};

function make(origin: ModelOrigin, extra: Partial<OriginInfo> = {}): OriginInfo {
  return {
    origin,
    label: LABELS[origin].label,
    short: LABELS[origin].short,
    detail: "",
    billable: origin === "cloud-api",
    leavesDevice: origin === "cloud-api",
    viaLocalApi: false,
    ...extra,
  };
}

/* ---------------- 供应商解析 ---------------- */

/** 一个能力位解析出来的「供应商 + 模型名」 */
export interface ResolvedProvider {
  kind: AiProviderKind;
  provider: AiProviderEntryView;
  model: string;
}

/**
 * 能力位 → 供应商。找不到返回 null。
 *
 * `upscale` 缺省时回落 `edit` —— 与设置界面那句「未配置时回落图像编辑」同一口径
 * （`AiSettingsDialog.vue` 的能力卡说明）。不回落会让「画质增强」在只配了编辑模型
 * 时被误标「未配置」，而实际上后端能跑。
 */
export function resolveBinding(
  kind: AiProviderKind,
  settings?: AiSettings | null,
): ResolvedProvider | null {
  const providers = settings?.providers ?? [];
  const find = (k: AiProviderKind): ResolvedProvider | null => {
    const b = settings?.[k];
    if (!b?.providerId) return null;
    const provider = providers.find((p) => p.id === b.providerId);
    if (!provider || !provider.baseUrl.trim()) return null;
    return { kind: k, provider, model: b.model };
  };
  const hit = find(kind);
  if (hit) return hit;
  return kind === "upscale" ? find("edit") : null;
}

/**
 * 云端工具要用的能力位列表。
 * 来源是 catalog 的 `providerPolicy`（`vision+edit` / `edit` / `upscale|edit`），
 * 不另抄一张「工具 → 能力位」表。
 */
export function kindsOfTool(toolId: string): AiProviderKind[] {
  const policy = toolById(toolId)?.providerPolicy ?? "none";
  const out: AiProviderKind[] = [];
  for (const part of policy.split("+")) {
    // `upscale|edit` 是「优选超分、回落编辑」的写法，这里按顺序全部列出
    for (const k of part.split("|")) {
      if (k === "vision" || k === "edit" || k === "upscale") out.push(k);
    }
  }
  return [...new Set(out)];
}

/** 云端算子要用的能力位：直接读 catalog 的 requires.providers */
export function kindsOfOp(opId: string): AiProviderKind[] {
  const raw = (opById(opId)?.requires?.providers ?? []) as unknown[];
  return raw.filter((k): k is AiProviderKind => k === "vision" || k === "edit" || k === "upscale");
}

/* ---------------- 单条判定 ---------------- */

/** 本机工具：有 `requires.models` 才是「跑模型」，否则是纯算法 / LUT */
function localOriginOf(models: unknown): ModelOrigin {
  return Array.isArray(models) && models.length > 0 ? "local-model" : "local-algo";
}

function localDetail(origin: ModelOrigin, models: unknown): string {
  const list = Array.isArray(models) ? models.join(" / ") : "";
  return origin === "local-model"
    ? `本机推理（${list}），免费、离线，照片不出本机`
    : "本机纯算法 / 查表，不跑模型，免费离线，照片不出本机";
}

/**
 * 云端工具的来源：按能力位逐个解析供应商。
 *
 * - 全都解析不出来 → 未配置模型（照常允许点，运行时报后端原有报错）；
 * - 全部落在回环地址 → 本机模型（文案点明「本机 API」，因为链路确实走 HTTP，
 *   只是不出本机）；
 * - 任一非回环 → 云端 API（照片会出去，且按调用计费）；混合时在说明里点明。
 * - `settings` 未载入（undefined）→ 按云端显示但不诬告「未配置」。
 */
function cloudOrigin(kinds: AiProviderKind[], settings?: AiSettings | null): OriginInfo {
  const resolved = kinds.map((k) => resolveBinding(k, settings)).filter(Boolean) as ResolvedProvider[];
  const allResolved = resolved.length === kinds.length && kinds.length > 0;

  if (!allResolved) {
    if (!settings) {
      return make("cloud-api", {
        detail: "模型来自你配置的云端服务；配置尚未载入，打开「⚙ 模型设置」可确认",
      });
    }
    return make("unconfigured", {
      detail: "还没绑定供应商（在「⚙ 模型设置」里绑定后可运行）；现在运行会报未配置",
      billable: false,
      leavesDevice: false,
    });
  }

  const hosts = resolved.map((r) => hostOf(r.provider.baseUrl));
  const allLocal = resolved.every((r) => isLoopbackHost(hostOf(r.provider.baseUrl)));
  if (allLocal) {
    const names = [...new Set(resolved.map((r) => r.provider.name.trim() || hostOf(r.provider.baseUrl)))]
      .join(" / ");
    return make("local-model", {
      viaLocalApi: true,
      detail: `走你本机运行的模型服务（${names}），免费、照片不出本机`,
    });
  }

  const remote = resolved
    .filter((r) => !isLoopbackHost(hostOf(r.provider.baseUrl)))
    .map((r) => r.provider.name.trim() || hostOf(r.provider.baseUrl));
  const mixed = resolved.length > remote.length;
  return make("cloud-api", {
    detail:
      `照片会发到你配置的服务商（${[...new Set(remote)].join(" / ")}），按调用计费` +
      (mixed ? `；另有部分能力位走本机（${hosts.join(" / ")}）` : ""),
  });
}

/* ---------------- 对外入口 ---------------- */

/** 某个工具的来源（左栏角标 / 标题副文案 / 面板来源行共用） */
export function originOfTool(toolId: string, settings?: AiSettings | null): OriginInfo {
  const tool = toolById(toolId);
  if (!tool) return make("unconfigured", { detail: "清单里没有这个工具", billable: false });
  if (tool.backend === "local") {
    const models = tool.op ? opById(tool.op)?.requires?.models : undefined;
    const origin = localOriginOf(models);
    return make(origin, { detail: localDetail(origin, models) });
  }
  return cloudOrigin(kindsOfTool(toolId), settings);
}

/** 某个原子算子的来源（工作台步骤链每一步） */
export function originOfOp(opId: string, settings?: AiSettings | null): OriginInfo {
  const op = opById(opId);
  if (!op) return make("unconfigured", { detail: "清单里没有这个组件", billable: false });
  if (op.backend === "local") {
    const origin = localOriginOf(op.requires?.models);
    return make(origin, { detail: localDetail(origin, op.requires?.models) });
  }
  return cloudOrigin(kindsOfOp(opId), settings);
}

/**
 * ①一键出片的配方：来源 = 配方实际算子集合的来源。
 * 「人像」配方含形变 + 磨皮（要本地模型），「自然」只有智能调色（纯算法）——
 * 用工具级 `backend` 判会把这两条标成同一个结果。
 */
export function originOfRecipe(recipeId: string, settings?: AiSettings | null): OriginInfo {
  const r = recipeById(recipeId);
  if (!r) return originOfTool("recipe", settings);
  return aggregateOrigins(r.steps.map((s) => originOfOp(s.op, settings)));
}

/** 自己组合的步骤链：只看启用中的步 */
export function originOfChain(
  steps: ReadonlyArray<{ op: string; enabled: boolean }>,
  settings?: AiSettings | null,
): OriginInfo {
  const live = steps.filter((s) => s.enabled);
  if (!live.length) return make("local-algo", { detail: "还没有启用任何步骤" });
  return aggregateOrigins(live.map((s) => originOfOp(s.op, settings)));
}

/**
 * 「一键出片」入口的来源：它背后是**一组配方**，不是单条链路。
 *
 * 不能按工具级判：四条配方里「自然 / 通透 / 胶片」是纯算法，「人像」要本地
 * 形变与磨皮模型 —— 用工具的 `backend` 判会把这两类标成同一个结果。
 */
export function originOfRecipeSet(ids: readonly string[], settings?: AiSettings | null): OriginInfo {
  const per = ids.map((id) => originOfRecipe(id, settings));
  if (!per.length) return originOfTool("recipe", settings);
  const agg = aggregateOrigins(per, "条配方");
  if (agg.origin === "local-model" && per.some((p) => p.origin === "local-algo")) {
    return make("local-model", {
      detail:
        "配方全部在本机跑，免费、照片不出本机；其中「人像」要本地模型（形变 / 磨皮），其余为纯算法",
    });
  }
  return agg;
}

/**
 * 一组来源合成一个（链条 / 配方 / 批量）。
 *
 * 优先级按「用户最需要知道的那件事」排：未配置 > 出网计费 > 本机模型 > 本机算法。
 * 未配置排最前是因为它会让整条链跑不起来，其它三档都只是花费与隐私差异。
 *
 * `unit` 只是文案用词：链条说「步」，配方集合说「条配方」。
 */
export function aggregateOrigins(
  list: ReadonlyArray<OriginInfo>,
  unit = "步",
): OriginInfo {
  if (!list.length) return make("local-algo", { detail: "还没有步骤" });
  if (list.length === 1) return list[0];

  const has = (o: ModelOrigin) => list.some((x) => x.origin === o);
  const counts = (o: ModelOrigin) => list.filter((x) => x.origin === o).length;

  if (has("unconfigured")) {
    return make("unconfigured", {
      detail: `有 ${counts("unconfigured")} ${unit}还没绑定供应商，跑不起来`,
      billable: false,
      leavesDevice: false,
    });
  }
  if (has("cloud-api")) {
    return make("cloud-api", {
      detail: `${counts("cloud-api")} ${unit}走云端 API（照片会离开本机，按调用计费）；其余 ${counts(
        "local-model",
      )} ${unit}本机模型、${counts("local-algo")} ${unit}本机算法`,
    });
  }
  if (has("local-model")) {
    const api = list.some((x) => x.viaLocalApi);
    return make("local-model", {
      viaLocalApi: api,
      detail: api
        ? `本机全部免费：${counts("local-model")} ${unit}调用本机模型服务，${counts(
            "local-algo",
          )} ${unit}纯算法，照片不出本机`
        : `全本机免费：${counts("local-model")} ${unit}跑本地模型，${counts(
            "local-algo",
          )} ${unit}纯算法，照片不出本机`,
    });
  }
  return make("local-algo", {
    detail: `全本机免费：${list.length} ${unit}都是纯算法 / 查表，不跑模型，照片不出本机`,
  });
}
