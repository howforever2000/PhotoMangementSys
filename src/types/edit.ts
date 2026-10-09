/**
 * 非破坏图像编辑引擎的前端类型（与 `src-tauri/src/edit/recipe.rs` 一一对应）
 * ===================================================================
 * ## 为什么单独一个文件
 *
 * 配方结构在 Rust（算法侧）与 TypeScript（UI 侧）各有一份定义。
 * 两份定义必然有漂移风险，因此约定：
 *
 * - **字段名、层级、可选性**必须逐字对应；
 * - Rust 侧 `#[serde(default)]` 允许缺字段，所以前端**不得**假设
 *   `invoke` 一定带回完整对象（老版本后端 / 手工构造的对象会缺键）；
 * - 数值一律 `number`，不做单位换算（单位换算只在 `AdjustSlider` 里做，
 *   依据是后端 schema 的 `format` 字段）。
 *
 * 新增参数时三处同步：Rust 结构体 + Rust `schema::basic_params()` + 此文件。
 */

/** 一条曲线上的控制点（x/y 均为 0..1 归一化，按 x 严格递增）。 */
export interface CurvePoint {
  x: number;
  y: number;
}

/** 一条色调曲线。`points` 少于 2 个时后端按恒等处理。 */
export interface ToneCurve {
  points: CurvePoint[];
}

/** 「基本」面板（FEAT-099）。 */
export interface BasicAdjustments {
  /** 曝光，单位 EV，-5..5 */
  exposure: number;
  /** 对比度，-1..1（支点 18% 中灰） */
  contrast: number;
  /** 高光，-1..1 */
  highlights: number;
  /** 阴影，-1..1 */
  shadows: number;
  /** 白色，-1..1 */
  whites: number;
  /** 黑色，-1..1 */
  blacks: number;
  /** 色温，K，1500..50000 */
  temperature: number;
  /** 色调，-100..100（负绿正品红） */
  tint: number;
}

/** 「颜色」面板。 */
export interface ColorAdjustments {
  /** 自然饱和度，-1..1 */
  vibrance: number;
  /** 饱和度，-1..1 */
  saturation: number;
}

/** 「细节」面板（锐化）。 */
export interface SharpenAdjustments {
  /** 强度，0..1（0 = 关闭） */
  amount: number;
  /** 半径，0..1（映射 σ 0.5..3.0 @短边 1000px） */
  radius: number;
  /** 细节，0..1 */
  detail: number;
  /** 蒙版，0..1 */
  masking: number;
}

/** 裁剪与拉直。坐标归一化到「旋转后画面」的 0..1。 */
export interface CropSpec {
  x: number;
  y: number;
  /** <= 0 视为未裁剪 */
  w: number;
  h: number;
  /** 拉直角度（度），-45..45 */
  angle: number;
  /** 是否处于裁剪生效状态 */
  enabled: boolean;
}

/** 一张图的全部编辑状态。 */
export interface AdjustmentRecipe {
  version: number;
  basic: BasicAdjustments;
  color: ColorAdjustments;
  /** RGB 曲线（保色相） */
  curveRgb: ToneCurve;
  /** 亮度曲线 */
  curveLuma: ToneCurve;
  sharpen: SharpenAdjustments;
  crop: CropSpec;
}

/** 空配方：所有字段为中性值。与 Rust 的 `AdjustmentRecipe::default()` 逐项对应。 */
export function emptyRecipe(): AdjustmentRecipe {
  return {
    version: 1,
    basic: {
      exposure: 0,
      contrast: 0,
      highlights: 0,
      shadows: 0,
      whites: 0,
      blacks: 0,
      temperature: 6500,
      tint: 0,
    },
    color: { vibrance: 0, saturation: 0 },
    curveRgb: { points: [] },
    curveLuma: { points: [] },
    sharpen: { amount: 0, radius: 0.5, detail: 0.25, masking: 0.25 },
    crop: { x: 0, y: 0, w: 1, h: 1, angle: 0, enabled: false },
  };
}

/**
 * 把任意（可能来自旧版本后端 / 用户粘贴的）对象收敛成完整配方。
 *
 * **前端也必须做这一步**，不能只依赖后端的 `sanitize`：后端只保证数值
 * 落在合法区间，不保证**对象结构**完整（缺 `crop` 的旧配方就会让 UI 崩在
 * `recipe.crop.enabled` 上）。这里逐字段补默认值。
 */
export function normalizeRecipe(input: unknown): AdjustmentRecipe {
  const base = emptyRecipe();
  if (!input || typeof input !== "object") return base;
  const o = input as Record<string, unknown>;

  const num = (v: unknown, fallback: number): number =>
    typeof v === "number" && Number.isFinite(v) ? v : fallback;
  const obj = (v: unknown): Record<string, unknown> =>
    v && typeof v === "object" ? (v as Record<string, unknown>) : {};

  const b = obj(o.basic);
  const c = obj(o.color);
  const s = obj(o.sharpen);
  const cr = obj(o.crop);

  const curve = (v: unknown): ToneCurve => {
    const src = obj(v);
    const pts = Array.isArray(src.points) ? src.points : [];
    return {
      points: pts
        .filter((p): p is Record<string, unknown> => !!p && typeof p === "object")
        .map((p) => ({ x: num(p.x, 0), y: num(p.y, 0) }))
        .filter((p) => p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1)
        .sort((a, z) => a.x - z.x),
    };
  };

  return {
    version: num(o.version, base.version),
    basic: {
      exposure: num(b.exposure, base.basic.exposure),
      contrast: num(b.contrast, base.basic.contrast),
      highlights: num(b.highlights, base.basic.highlights),
      shadows: num(b.shadows, base.basic.shadows),
      whites: num(b.whites, base.basic.whites),
      blacks: num(b.blacks, base.basic.blacks),
      temperature: num(b.temperature, base.basic.temperature),
      tint: num(b.tint, base.basic.tint),
    },
    color: {
      vibrance: num(c.vibrance, base.color.vibrance),
      saturation: num(c.saturation, base.color.saturation),
    },
    curveRgb: curve(o.curveRgb ?? o.curve_rgb),
    curveLuma: curve(o.curveLuma ?? o.curve_luma),
    sharpen: {
      amount: num(s.amount, base.sharpen.amount),
      radius: num(s.radius, base.sharpen.radius),
      detail: num(s.detail, base.sharpen.detail),
      masking: num(s.masking, base.sharpen.masking),
    },
    crop: {
      x: num(cr.x, 0),
      y: num(cr.y, 0),
      w: num(cr.w, 1),
      h: num(cr.h, 1),
      angle: num(cr.angle, 0),
      enabled: cr.enabled === true,
    },
  };
}

/** 配方是否等于空配方（用于「是否已修改」提示）。 */
export function isPristineRecipe(r: AdjustmentRecipe): boolean {
  return JSON.stringify(r) === JSON.stringify(emptyRecipe());
}

/** 打开图片的返回值。 */
export interface EditOpenResult {
  sessionId: number;
  width: number;
  height: number;
  previewWidth: number;
  previewHeight: number;
  subsampled: boolean;
  recipe: AdjustmentRecipe;
  hasRecipe: boolean;
  decodeMs: number;
}

/** 渲染元信息。 */
export interface EditRenderMeta {
  width: number;
  height: number;
  elapsedMs: number;
  /** 是否因有更新的请求而被后端丢弃 */
  stale: boolean;
}

/** 直方图。`rgb` 是平铺的 768 项，索引 = 值*3 + 通道（0=R,1=G,2=B）。 */
export interface EditHistogram {
  rgb: number[];
  luma: number[];
  clippedLow: number;
  clippedHigh: number;
  width: number;
  height: number;
}

/** 参数控件类型。 */
export type EditControlKind = "slider" | "sliderPositive" | "select";

/** 数值显示格式。 */
export type EditValueFormat = "plain" | "ev" | "kelvin" | "percent" | "percent100";

/** 单个滑块的定义（来自后端 schema）。 */
export interface EditParamSpec {
  key: string;
  label: string;
  kind: EditControlKind;
  min: number;
  max: number;
  neutral: number;
  step: number;
  format: EditValueFormat;
  fineStep: number;
  help: string;
}

/** 曲线控件定义。 */
export interface EditCurveSpec {
  key: "curveRgb" | "curveLuma";
  label: string;
  help: string;
}

/** 面板分组。 */
export interface EditGroupSpec {
  key: string;
  label: string;
  order: number;
  params: EditParamSpec[];
  curves: EditCurveSpec[];
  collapsed: boolean;
}

/** 完整参数表。 */
export interface EditSchema {
  version: number;
  groups: EditGroupSpec[];
  previewLongEdge: number;
  draftLongEdge: number;
  formats: string[];
  controls: string[];
}

/**
 * 按点号路径读写配方字段。
 *
 * 只支持一层点号（`basic.exposure`），因为后端 schema 就只有这一种深度。
 * 写深一层的通用路径解析器在这里是过度设计 —— 而且它会把「路径写错」
 * 这类错误从编译期推迟到运行期。
 */
export function readParam(r: AdjustmentRecipe, key: string): number {
  const [group, field] = key.split(".");
  const bag = (r as unknown as Record<string, Record<string, unknown>>)[group];
  const v = bag?.[field];
  return typeof v === "number" ? v : 0;
}

export function writeParam(r: AdjustmentRecipe, key: string, value: number): void {
  const [group, field] = key.split(".");
  const bag = (r as unknown as Record<string, Record<string, unknown>>)[group];
  if (!bag || typeof bag !== "object" || !field) return;
  // 必须是**已存在**的键：直接 `bag[field] = v` 会把拼错的键名
  // 悄悄塞进对象，之后 JSON 序列化时被当成有效参数发给后端。
  if (!(field in bag)) return;
  bag[field] = value;
}

/** 曲线的深拷贝（撤销栈需要快照，不能共享引用）。 */
export function cloneRecipe(r: AdjustmentRecipe): AdjustmentRecipe {
  return {
    ...r,
    basic: { ...r.basic },
    color: { ...r.color },
    curveRgb: { points: r.curveRgb.points.map((p) => ({ ...p })) },
    curveLuma: { points: r.curveLuma.points.map((p) => ({ ...p })) },
    sharpen: { ...r.sharpen },
    crop: { ...r.crop },
  };
}