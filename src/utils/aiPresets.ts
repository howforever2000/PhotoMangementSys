/** AI 创作预设（FEAT-152）
 *
 * 只放「选项标签」，与 Python `ai_studio/presets.py` 的 key 一一对应。
 * 真正的提示词与排版参数在后端（Python），前端只负责把 key 传给后端。
 */

export interface AiOption {
  value: string;
  label: string;
  hint?: string;
}

/** 主题手账：版式模板 */
export const JOURNAL_TEMPLATES: AiOption[] = [
  { value: "blank", label: "留白", hint: "白底大留白，安静克制" },
  { value: "magazine", label: "杂志", hint: "上半满版大图 + 文段" },
  { value: "film", label: "胶片", hint: "深色底 + 细边框" },
];

/** 主题手账：图像风格 */
export const JOURNAL_STYLES: AiOption[] = [
  { value: "sketch", label: "素描" },
  { value: "watercolor", label: "水彩" },
  { value: "ink", label: "水墨" },
  { value: "film", label: "胶片" },
];

/** 人物美颜：风格 */
export const BEAUTY_STYLES: AiOption[] = [
  { value: "natural", label: "自然" },
  { value: "crisp", label: "清透" },
  { value: "film", label: "胶片" },
];

/** 人物美颜：级别 */
export const BEAUTY_LEVELS: AiOption[] = [
  { value: "light", label: "轻" },
  { value: "medium", label: "中" },
  { value: "strong", label: "强" },
];

/** 滤镜迁移模式 */
export const STYLE_MODES: AiOption[] = [
  { value: "transfer", label: "参考图迁移", hint: "输入内容图 + 参考图" },
  { value: "autograde", label: "自主调色", hint: "仅凭提示词调色" },
];

/** 超分倍数 */
export const UPSCALE_SCALES: AiOption[] = [
  { value: "2", label: "2×" },
  { value: "4", label: "4×" },
];
