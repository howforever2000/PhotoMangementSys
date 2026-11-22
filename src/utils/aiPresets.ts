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
export interface AiStyleOption extends AiOption {
  /** 分组 key，与 Python `presets.JOURNAL_STYLES[*].group` 一一对应（仅影响呈现，不影响处理） */
  group: string;
}

/** 主题手账：风格分组（顺序即呈现顺序） */
export const JOURNAL_STYLE_GROUPS: AiOption[] = [
  { value: "drawing", label: "绘画" },
  { value: "cartoon", label: "卡通形象" },
];

export const JOURNAL_STYLES: AiStyleOption[] = [
  { value: "sketch", label: "素描", group: "drawing" },
  { value: "watercolor", label: "水彩", group: "drawing" },
  { value: "ink", label: "水墨", group: "drawing" },
  { value: "film", label: "胶片", group: "drawing" },
  { value: "anime_summer", label: "动画夏日", group: "cartoon", hint: "日式 2D 手绘动画：扁平色块 + 夏日光影" },
  { value: "guoman_3d", label: "国漫 3D", group: "cartoon", hint: "三维动画电影渲染：立体材质 + 电影级打光" },
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

/** 参考图调色（本地）：算法 */
export const CT_METHODS: AiOption[] = [
  { value: "reinhard", label: "自然", hint: "按均值/方差迁移，柔和自然（推荐）" },
  { value: "histogram", label: "强匹配", hint: "逐通道直方图匹配，风格更强烈" },
];

/** 画质修复（本地）：放大倍数（模型固定 4× 推理，2× 会再缩到目标） */
export const RESTORE_SCALES: AiOption[] = [
  { value: "2", label: "2×" },
  { value: "4", label: "4×" },
];

/** 智能调色（本地）：风格。key 与 Python `autograde.STYLES` 一一对应 */
export const AUTOGRADE_STYLES: AiOption[] = [
  { value: "natural", label: "自然", hint: "收一点对比与饱和，尽量不改变画面气质" },
  { value: "vivid", label: "鲜明", hint: "对比与自然饱和度都更强，适合风景" },
  { value: "warm", label: "暖调", hint: "偏暖，适合人像与室内" },
  { value: "cool", label: "冷调", hint: "偏冷，适合雪景、阴天与工业风" },
];

/** 本地美颜（本地）：力度档。数值即磨皮强度，与 Python 侧 0~1 对应 */
export const LOCAL_BEAUTY_LEVELS: AiOption[] = [
  { value: "0.3", label: "轻", hint: "只压明显噪点，保留皮肤纹理" },
  { value: "0.6", label: "中", hint: "日常人像的推荐档" },
  { value: "0.9", label: "强", hint: "明显平滑，适合低分辨率或手机前置" },
];

/** 肤色档位（FEAT-208/209）：映射到 Python `skintone.SkinParams` 的四个配方级量。
 *  数值必须与 `op_catalog.json` 里 skin_tone 参数的 min/max 一致（守卫测试兜底）。
 *  后端会逐脸按发长收敛：长发用这里的档位；其余只还原原肤色 + 轻提亮（男性档）。 */
export interface SkinPreset {
  value: string;
  label: string;
  hint: string;
  params: { hue_shift: number; chroma: number; brighten: number; blush: number };
}

export const SKIN_PRESETS: SkinPreset[] = [
  {
    value: "off",
    label: "关闭",
    hint: "不做肤色调整，只磨皮",
    params: { hue_shift: 0, chroma: 0, brighten: 0, blush: 0 },
  },
  {
    value: "natural",
    label: "自然",
    hint: "先把滤镜带偏的肤色拉回原图，再提亮与苹果肌嫩红（推荐；男性按发长自动收敛）",
    params: { hue_shift: 3, chroma: 1.5, brighten: 2.2, blush: 1.8 },
  },
  {
    value: "fair",
    label: "白皙",
    hint: "更亮更净，血色收敛，适合想突出通透感的照片",
    params: { hue_shift: 3, chroma: 1.0, brighten: 3.0, blush: 1.0 },
  },
  {
    value: "rosy",
    label: "红润",
    hint: "血色更明显，适合气色差或室内灯光偏黄的照片",
    params: { hue_shift: 3, chroma: 2.0, brighten: 2.2, blush: 2.6 },
  },
];

/** 人像形变（本地）：瘦脸强度档。上限 0.8 —— 再高会明显失真（实测）。
 *  「自然」是 FEAT-210 起的默认档（0.15 = 配方默认值；配方链路里男脸按 2/3 折算成 0.10）。 */
export const RESHAPE_SLIM_LEVELS: AiOption[] = [
  { value: "0.15", label: "自然", hint: "只收一点点下颌线，几乎看不出修过（默认）" },
  { value: "0.3", label: "轻", hint: "下颌线略微收窄，五官不动" },
  { value: "0.5", label: "标准", hint: "脸型明显收窄，五官不动" },
  { value: "0.8", label: "强", hint: "脸型收窄明显；再高会失真，已到上限" },
];

/** 人像形变（本地）：大眼强度档。上限 0.5。「自然」是 FEAT-210 起的默认档（0.20） */
export const RESHAPE_EYE_LEVELS: AiOption[] = [
  { value: "0", label: "关", hint: "只瘦脸，不动眼睛" },
  { value: "0.2", label: "自然", hint: "眼睛略微放大，眼周自然（默认）" },
  { value: "0.3", label: "标准", hint: "眼睛放大明显，适合近景" },
  { value: "0.5", label: "强", hint: "眼睛放大明显；再高会失真，已到上限" },
];
