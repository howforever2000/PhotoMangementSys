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

/**
 * 一条算子步骤（与 `src-tauri/src/edit/recipe.rs` 的 `OpStep` 对应）。
 *
 * 语义是「记录在图像上的处理」：重开这张照片、重新导出，得到的都是
 * 同一串步骤的结果。因此步骤进 sidecar，而不是只活在内存里。
 */
export interface OpStep {
  /** 算子 id（对应 `/api/ops` 的 `id`，如 `blur-gaussian`） */
  op: string;
  /** 算子参数，原样透传给 `/api/apply` */
  params: Record<string, number | string | boolean>;
  /** 关掉保留步骤与参数，可临时跳过而不删 */
  enabled: boolean;
}

/**
 * 局部蒙版参数（与 `src-tauri/src/edit/recipe.rs` 的 `MaskSpec` 对应）。
 *
 * 蒙版**像素不在配方里**：灰度位图动辄几 MB，塞进 JSON 会让 sidecar
 * 变成不可用的庞然大物。配方只存参数与「有没有蒙版」这一个事实，
 * 像素存在同目录的 `<文件名>.pmmask.png`。
 */
export interface MaskSpec {
  /** 是否启用局部调整。关掉时整图套用。 */
  enabled: boolean;
  /** 羽化半径，单位是蒙版自身的像素，缩放时同比例放大 */
  featherPx: number;
  /** 整体强度 0..1（在羽化之后缩放权重） */
  strength: number;
  /** 反相：选中区与未选中区互换 */
  invert: boolean;
  /** 磁盘上是否存在蒙版文件 */
  hasImage: boolean;
}

/** 蒙版操作的返回信息。 */
export interface MaskInfo {
  /** 蒙版自身宽高（通常等于预览分辨率） */
  width: number;
  height: number;
  /** 选中区域占比 0..1 */
  coverage: number;
  /** 落盘路径 */
  path: string;
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
  /** 局部蒙版参数（像素不在配方里，见 MaskSpec 注释） */
  mask: MaskSpec;
  /**
   * 算子步骤（按顺序作用在调色结果之上）。
   *
   * 进 sidecar，因此重开照片 / 重新导出都会复现同一串处理 ——
   * 这就是「每一步记录在图像上」的落地。
   */
  ops: OpStep[];
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
    mask: { enabled: false, featherPx: 0, strength: 1, invert: false, hasImage: false },
    ops: [],
  };
}

/** 读数值，非有限数（含 NaN / Infinity）一律回落。 */
const num = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;

/** 读对象，非对象（含 null / 数组 / 字符串）一律回落成空对象。 */
const obj = (v: unknown): Record<string, unknown> =>
  v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {};

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
    mask: normalizeMask(o.mask ?? o.maskSpec, base.mask),
    ops: normalizeOps(o.ops),
  };
}

/**
 * 收敛算子步骤数组。
 *
 * **不校验 `op` 是否存在** —— 算子注册表在 Python 侧，前端拿不到权威列表，
 * 猜一个 id 就会把「服务暂时不可用」误判成「配方损坏」。无效 id 交给
 * `/api/apply` 去拒绝，那里有明确的错误文案与支持列表。
 *
 * 只过滤结构上不成立的条目（不是对象、缺 op 字段）。
 */
function normalizeOps(input: unknown): OpStep[] {
  if (!Array.isArray(input)) return [];
  return input
    .filter((s): s is Record<string, unknown> => !!s && typeof s === "object")
    .filter((s) => typeof s.op === "string" && s.op.length > 0)
    .map((s) => ({
      op: s.op as string,
      // params 必须是普通对象：null / 数组 / 数字透传给后端会被 json.loads 拒
      params: (obj(s.params) as Record<string, number | string | boolean>) ?? {},
      // 缺 enabled 视为启用：老配方没有这个字段
      enabled: s.enabled !== false,
    }));
}

/**
 * 收敛蒙版字段。
 *
 * `hasImage` 与 `enabled` 的关系必须在这里也维护一次：没有蒙版像素时
 * 强制关掉 `enabled`，否则 UI 会显示「局部调整已开」而渲染按整图走。
 * 后端 `sanitize` 也会做同样的事，前端做一遍是为了让**当前这一帧**
 * 的显示就与实际一致，不必等下一轮往返。
 */
function normalizeMask(input: unknown, base: MaskSpec): MaskSpec {
  const m = obj(input);
  const hasImage = m.hasImage === true;
  return {
    enabled: hasImage && m.enabled === true,
    featherPx: num(m.featherPx ?? m.feather_px, base.featherPx),
    strength: num(m.strength, base.strength),
    invert: m.invert === true,
    hasImage,
  };
}

/** 配方是否等于空配方（用于「是否已修改」提示）。 */
export function isPristineRecipe(r: AdjustmentRecipe): boolean {
  return JSON.stringify(r) === JSON.stringify(emptyRecipe());
}

/** 蒙版的客观事实（是否存在、多大）。 */
export interface EditMaskInfo {
  exists: boolean;
  width: number;
  height: number;
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
  /** 磁盘上是否已有蒙版 */
  mask: EditMaskInfo;
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
    mask: { ...r.mask },
    // 步骤数组：每步含一个 params 对象，必须逐层拷贝，否则撤销栈里的
    // 快照会与当前配方共享同一个 params 引用，改一步就污染所有历史。
    ops: r.ops.map((s) => ({ ...s, params: { ...s.params } })),
  };
}

/**
 * 「零调整」配方：**保留几何、抹掉一切修饰**。
 *
 * 用于对比视图的左格（调整前）。刻意保留 `crop`：左右两格必须同取景，
 * 否则并排看的是两幅不同构图的东西，对比就失去意义。
 *
 * `mask` 也一并关掉 —— 蒙版只是「限制调整范围」，零调整时它无处施加。
 */
export function withoutAdjustments(r: AdjustmentRecipe): AdjustmentRecipe {
  const e = emptyRecipe();
  return {
    ...e,
    crop: { ...r.crop },
    // hasImage 跟着保留，否则后端会认为磁盘上没蒙版而回落到无蒙版渲染 ——
    // 那正好是我们要的（零调整 + 有无蒙版渲染结果相同），但保持字段真实更不易误解。
    mask: { ...e.mask, hasImage: r.mask.hasImage },
  };
}