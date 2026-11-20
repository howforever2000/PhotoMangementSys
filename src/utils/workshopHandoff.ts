/**
 * 「把照片送进创意工坊」的携带协议（FEAT-187）
 *
 * 背景：工坊素材托盘本来就吃「一组照片」（`stores/workshopUi.tray`），但入口侧
 * 过去只有原图预览的单张按钮（FEAT-150）。相册 / 人物 / 分类 / 地点四个预览界面
 * 进入多选态后都没有对应选项，用户只能一张张往里送。
 *
 * 纪律：URL 里怎么表达「一组路径」只能有一份实现 —— 入口侧拼、工坊侧拆，两边
 * 各写一次 `split("|")` 迟早漂移（FEAT-168 的 `local_reshape` 双 id 就是这么漂的）。
 * 所以拼与拆都放本文件：纯函数、无副作用、可单测。
 *
 * 协议：`?photos=a|b|c`。用 `|` 而不是 `,` / `;` —— Windows 路径本身可以带逗号
 * 与分号，`|` 是文件名里的非法字符，永远不会出现在路径中（`Workshop.vue` 原注释
 * 的约定，此处固化为实现）。
 */

/** 多条路径在 URL 里的分隔符 */
export const WORKSHOP_PATH_SEP = "|";

/**
 * 规整路径列表：去首尾空白、丢空串、去重且**保序**
 *
 * 保序很重要：托盘顺序就是用户在网格里看到/勾选的顺序，乱序会让「第一批」变成
 * 随机一张。去重走 Set（插入序），不是 sort。
 */
export function dedupePhotoPaths(paths: readonly string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of paths) {
    if (typeof raw !== "string") continue;
    const p = raw.trim();
    if (!p || seen.has(p)) continue;
    seen.add(p);
    out.push(p);
  }
  return out;
}

/** 拼成 `?photos=` 的参数值；空输入返回空串（调用方据此判断「没东西可送」） */
export function joinPhotoPaths(paths: readonly string[]): string {
  return dedupePhotoPaths(paths).join(WORKSHOP_PATH_SEP);
}

/**
 * 拆开 `?photos=`（同时兼容单张用的 `?photo=`）
 *
 * 入参取 `unknown`：vue-router 的 query 值是 `string | string[] | null`，
 * 非字符串一律按空处理，避免把 `undefined` 拼成字符串 "undefined"。
 */
export function splitPhotoPaths(raw: unknown): string[] {
  if (typeof raw !== "string") return [];
  return dedupePhotoPaths(raw.split(WORKSHOP_PATH_SEP));
}

/** 工坊路由目标（`name` + `query`），与 router 定义同源，避免手抄路径串 */
export interface WorkshopRouteTarget {
  name: "workshop";
  query: Record<string, string>;
}

/**
 * 生成「带照片进工坊」的路由目标；一组都为空时返回 null
 *
 * @param paths 目标照片（可为空数组，返回 null 让调用方直接放弃）
 * @param tool  可选工位 id；不传则由工坊自己落到默认工位（①一键出片）
 */
export function workshopRouteTarget(
  paths: readonly string[],
  tool?: string,
): WorkshopRouteTarget | null {
  const photos = joinPhotoPaths(paths);
  if (!photos) return null;
  const query: Record<string, string> = { photos };
  // 工位 id 必须来自 catalog（`isWorkshopTool`），这里不做二次校验：
  // 校验属编排层职责，纯函数只负责拼装
  if (tool) query.tool = tool;
  return { name: "workshop", query };
}

/* ---------------- 工坊 → 工作台的跳转协议 ---------------- */

/** 工作台路由目标（`name` + `query`），与 router 定义同源 */
export interface WorkbenchRouteTarget {
  name: "ai-workbench";
  query: Record<string, string>;
}

export interface WorkbenchJumpOptions {
  /** 落地引擎；当前只支持「创意组合」（chain） */
  engine?: "chain";
  /** 预置的原子算子 id（engine=chain 时生效；合法性由工作台侧校验） */
  op?: string;
}

/**
 * 生成「带照片进工作台」的路由目标。
 *
 * 与 `workshopRouteTarget` 同一纪律：URL 里怎么表达一组路径只有这一份实现。
 * 照片为空时**允许**只带 engine/op（从空托盘的单步工具跳过去也成立）；
 * 三样都没有才返回 null。
 */
export function workbenchRouteTarget(
  paths: readonly string[],
  opts: WorkbenchJumpOptions = {},
): WorkbenchRouteTarget | null {
  const query: Record<string, string> = {};
  const photos = joinPhotoPaths(paths);
  if (photos) query.photos = photos;
  if (opts.engine) query.engine = opts.engine;
  if (opts.op) query.op = opts.op;
  if (!photos && !opts.engine && !opts.op) return null;
  return { name: "ai-workbench", query };
}
