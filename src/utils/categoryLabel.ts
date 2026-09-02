/**
 * AI 分类类目中文化工具。
 *
 * 后端（python/vcr/taxonomy.py）在输出前会把内部 key 折叠到 9 大组：
 *   portrait / street / animal / food / flower / landscape / cityscape / night_scene / document
 *
 * 但仍有少量中间 key 透出（如 `landscape_nature` / `architecture` / `plant_flower` / `indoor`），
 * —— 这些来自 ImageNet 通道或 Places365 scene 通道，不便在后端做二次折叠，
 * 在前端统一处理即可。
 *
 * 使用：
 *   import { categoryLabel, categoryTone } from "@/utils/categoryLabel";
 *   <span class="chip">{{ categoryLabel(hit.category) }}</span>
 */

/** 9 大组 + 中间 key 的中文标签映射。 */
const LABELS: Record<string, string> = {
  // 9 大组（taxonomy 输出）
  portrait: "人物",
  street: "扫街",
  animal: "动物",
  food: "食物",
  flower: "花朵",
  landscape: "自然风景",
  cityscape: "城市风光",
  night_scene: "夜景",
  document: "文档",

  // 中间 key（ImageNet / Places365 通道直接输出）
  landscape_nature: "自然风景",
  architecture: "建筑",
  plant_flower: "植物花卉",
  plant: "植物",
  text: "文档",
  sports: "运动",
  vehicle: "车辆",
  indoor: "室内",
  other: "其他",
};

/**
 * 把任意 category key 翻成中文。
 * 未知 key 原样返回，便于排查（如 "scene/foo/bar"）。
 *
 * @param key 后端返回的类目 key
 */
export function categoryLabel(key: string | null | undefined): string {
  if (!key) return "";
  const k = String(key).toLowerCase();
  return LABELS[k] ?? key;
}

/**
 * 类目对应的色调（用于 chip 背景）。
 * 浅色模式：浅彩底；深色模式由调用方基于 css 变量覆盖。
 */
const TONES: Record<string, { bg: string; fg: string; border: string }> = {
  portrait:      { bg: "#fde7ef", fg: "#a8326a", border: "#f4c1d6" },
  street:        { bg: "#f4f0ff", fg: "#5e4ad6", border: "#d8cbf7" },
  animal:        { bg: "#fff4e0", fg: "#a76f0b", border: "#f4dca6" },
  food:          { bg: "#fff0e6", fg: "#c2501f", border: "#f7d3b3" },
  flower:        { bg: "#fff0f5", fg: "#c8347a", border: "#f5c4d8" },
  landscape:     { bg: "#e8f3e6", fg: "#3a7d3c", border: "#bcdbb8" },
  landscape_nature: { bg: "#e8f3e6", fg: "#3a7d3c", border: "#bcdbb8" },
  cityscape:     { bg: "#e6eef9", fg: "#2c5aa0", border: "#b6cce8" },
  architecture:  { bg: "#e6eef9", fg: "#2c5aa0", border: "#b6cce8" },
  night_scene:   { bg: "#2a2f44", fg: "#cfd6f5", border: "#4a5170" },
  document:      { bg: "#eef1f5", fg: "#475569", border: "#cad0db" },
  text:          { bg: "#eef1f5", fg: "#475569", border: "#cad0db" },
  plant:         { bg: "#ecf6e9", fg: "#5c8b3a", border: "#cae0b6" },
  plant_flower:  { bg: "#ecf6e9", fg: "#5c8b3a", border: "#cae0b6" },
  sports:        { bg: "#e6f6fb", fg: "#1f7a96", border: "#b6e0ec" },
  vehicle:       { bg: "#f3f4f6", fg: "#475569", border: "#cdd3da" },
  indoor:        { bg: "#f5eef9", fg: "#7a3eb1", border: "#dcc7eb" },
  other:         { bg: "#f3f4f6", fg: "#64748b", border: "#cdd3da" },
};

/**
 * 取类目的"色卡"。深色模式下整体走 CSS 变量覆盖，这里只给浅色模式默认值。
 */
export function categoryTone(key: string | null | undefined): { bg: string; fg: string; border: string } {
  if (!key) return TONES.other;
  return TONES[String(key).toLowerCase()] ?? TONES.other;
}

/** 是否已知 key（用于排查后端是否漏发新类目） */
export function isKnownCategory(key: string | null | undefined): boolean {
  if (!key) return false;
  return String(key).toLowerCase() in LABELS;
}