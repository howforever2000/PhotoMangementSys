/**
 * 工作台预览缓存的纯逻辑（FEAT-177 性能收敛）
 * ===================================================================
 * 与 `composables/useWorkbenchPreview.ts` 的关系：那边负责「发请求、防抖、单飞、
 * 丢弃过期结果」，这里只负责两件可独立测试的纯计算：
 *
 *   1. **缓存键**：同一 (图 + 计划 + 参数) 必须命中同一条；不同参数必须不同。
 *   2. **LRU**：命中要刷新顺序，淘汰最久未用的一条。
 *
 * 为什么抽出来：这两条以前写在 composable 的闭包里，既没有测试覆盖，也很难
 * 在改并发逻辑时判断有没有破坏缓存语义。抽成 Map 操作后可以直接单测。
 */

/** LRU 缓存条数上限（每条是一个 1280px JPEG 的 data URL，3~5MB 量级） */
export const PREVIEW_CACHE_LIMIT = 8;

/**
 * 稳定序列化：把参数对象拍成确定性的字符串。
 *
 * 直接 `JSON.stringify` 有两个问题：
 *   - 对象键顺序随创建顺序变化，同样的参数可能算出不同的键（缓存永远不命中）；
 *   - 参数对象很小，但滑杆 60Hz 时每次都全量序列化两次仍然浪费。
 *
 * 这里按键名排序后拼接，既稳定又比 JSON.stringify 快（无转义、无递归括号）。
 */
export function stableKey(value: unknown): string {
  if (value == null) return "";
  if (typeof value !== "object") return String(value);
  if (Array.isArray(value)) return `[${value.map(stableKey).join(",")}]`;
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj).sort();
  return `{${keys.map((k) => `${k}:${stableKey(obj[k])}`).join(",")}}`;
}

/** 预览缓存键：路径 + 配方 + 长边 + 计划 + 参数覆盖 */
export function previewCacheKey(input: {
  path: string;
  recipeId?: string | null;
  maxEdge?: number;
  plan?: unknown;
  paramsOverride?: unknown;
}): string {
  return [
    input.path,
    input.recipeId ?? "",
    input.maxEdge ?? 0,
    stableKey(input.plan),
    stableKey(input.paramsOverride),
  ].join("|");
}

/**
 * 按 LRU 语义写入：命中时先删后插（刷新为最新），超限时淘汰最久未用的。
 *
 * 直接返回同一个 Map，便于在 composable 里持有单一实例。
 */
export function putLru<K, V>(cache: Map<K, V>, key: K, value: V, limit = PREVIEW_CACHE_LIMIT): Map<K, V> {
  if (cache.has(key)) cache.delete(key);
  while (cache.size >= limit) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
  cache.set(key, value);
  return cache;
}

/** 按 LRU 语义读取：命中时刷新顺序 */
export function getLru<K, V>(cache: Map<K, V>, key: K): V | undefined {
  if (!cache.has(key)) return undefined;
  const value = cache.get(key) as V;
  cache.delete(key);
  cache.set(key, value);
  return value;
}
