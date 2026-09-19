<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useThemeStore } from "../../stores/theme";

/**
 * 创意工坊 · 传统图像处理（FEAT-066）
 * ===================================
 * 宿主组件：只负责与算法无关的壳子 —— 图片载入、画布与缩放、蒙版编辑、
 * 结果展示与保存。具体算子与参数表单由后端 `GET /api/ops` 返回的 schema 驱动，
 * 新增算子只改后端注册表，前端不需要加分支。
 *
 * 处理：独立 Python 微服务（python-studio，Rust studio_ensure 负责拉起），
 *       前端 fetch 直连 127.0.0.1，`POST /api/apply` 上传 op + params + 原图 + 可选蒙版。
 * 蒙版约定：独立离屏 canvas，与原图同分辨率；**画布内透明=未选、白=选中**
 *       （BUG-2026-0918-006：不透明黑底会让 overlay 的 source-in 红色叠加
 *       铺满全图——黑也是 alpha=255）。导出给后端时铺黑合成灰度 PNG。
 * 蒙版是可选的：不画蒙版 = 整图处理（模糊类默认整图）。
 *
 * 布局约定（BUG-2026-0918-008）：两张画布包在 `.ip-frame` 内，由文档流中的 display
 *       画布撑开高度。canvas 若直接绝对定位在 `.ip-stage` 上（脱离文档流），舞台高度
 *       只剩 min-height，高图会向下溢出、盖住参数行与底栏按钮。
 */
const theme = useThemeStore();

/**
 * FEAT-065：可选的初始图片（由灯箱「✏️ 编辑」经 /workshop?photo=<path> 带入）。
 * 传了就自动载入，不传则保持原有「先选择一张图片」流程。
 */
const props = defineProps<{ initialPath?: string }>();

/* ---------------- 算子注册表（后端拉取） ---------------- */
type OpParam = {
  name: string;
  label: string;
  type: "number" | "bool" | "enum";
  min?: number;
  max?: number;
  step?: number;
  default: number | string | boolean;
  choices?: { value: string; label: string }[];
};
type OpDef = { id: string; label: string; supportsMask: boolean; params: OpParam[] };

/** 灯箱 ✏️ 直达的默认算子（FEAT-065 行为不变） */
const DEFAULT_OP = "region-equalize";

const ops = ref<OpDef[]>([]);
const opId = ref(DEFAULT_OP);
/**
 * 算子列表三态（BUG-2026-0918-009）：显式的 loading / ready / error，失败必须给可点的重试。
 * 绝不允许「加载中…」永久停留——旧实现因为后端 ensure 永不返回，用户看到的就是
 * 永远加载中，既没错误也没重试入口，无从判断是慢还是坏。
 */
const opsState = ref<"loading" | "ready" | "error">("loading");
const opsError = ref("");
/** 算子列表超时：服务冷启动约 1~2s（python + cv2），留足余量后必须回到失败态 */
const OPS_TIMEOUT_MS = 12_000;
/** 当前算子的参数值（切换算子时按 schema 重置为默认值） */
const values = ref<Record<string, number | string | boolean>>({});

const currentOp = computed(() => ops.value.find((o) => o.id === opId.value) ?? null);
/** 模糊组：注册表里 id 以 blur- 开头的算子（新增模糊算子自动归入该组） */
const blurOps = computed(() => ops.value.filter((o) => o.id.startsWith("blur-")));
const eqOp = computed(() => ops.value.find((o) => o.id === DEFAULT_OP) ?? null);
/** 顶层分组：均衡化 / 模糊 */
const group = computed<"eq" | "blur">(() => (opId.value === DEFAULT_OP ? "eq" : "blur"));

/* ---------------- 状态 ---------------- */
type Tool = "rect" | "brush" | "eraser";
const tool = ref<Tool>("rect");
const brushSize = ref(24);

const srcPath = ref("");
const statusMsg = ref("先选择一张图片");
const processing = ref(false);
const hasResult = ref(false);
/** 蒙版上是否有选中区域（无蒙版 = 整图处理，不再阻止「应用」） */
const hasMask = ref(false);

/**
 * FEAT-068 修图前后对比（仿 Lightroom）：
 *  - edit ：编辑态（蒙版可画）
 *  - side ：左右并排对比（Lightroom 的 Y/Y 快捷键，再按 Y 回编辑态）
 *  - split：单画布分割对比，分割线可拖动（左原图 / 右结果）
 * 「原图」指当前底图 baseEl（叠加链下的最近基底），「结果」为最近一次应用输出。
 */
type ViewMode = "edit" | "side" | "split";
const viewMode = ref<ViewMode>("edit");
/** 分割线位置（0~1，占画布宽度比例） */
const splitX = ref(0.5);
let splitDragging = false;

function setView(m: ViewMode) {
  if (m !== "edit" && !hasResult.value) return;
  if (viewMode.value === m) return;
  viewMode.value = m;
}

/** 视图切换后画布重挂载（side 模式换分支），等 DOM 稳定后重算尺寸并重绘 */
watch(viewMode, async () => {
  await nextTick();
  if (srcPath.value) {
    fitDisplay();
    redraw();
  }
});

/** Y 键：Lightroom 式左右对比开关（有结果时可用；输入控件聚焦时不劫持） */
function onCompareKey(e: KeyboardEvent) {
  if (e.key !== "y" && e.key !== "Y") return;
  if (!hasResult.value) return;
  const t = e.target as HTMLElement | null;
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
  e.preventDefault();
  setView(viewMode.value === "side" ? "edit" : "side");
}
window.addEventListener("keydown", onCompareKey);

/* ---------------- 分割线拖动 ---------------- */
const splitFrameEl = ref<HTMLElement | null>(null);

function onSplitDown(e: PointerEvent) {
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  splitDragging = true;
}
function onSplitMove(e: PointerEvent) {
  if (!splitDragging) return;
  const frame = splitFrameEl.value;
  if (!frame) return;
  const r = frame.getBoundingClientRect();
  splitX.value = Math.min(0.98, Math.max(0.02, (e.clientX - r.left) / r.width));
  redraw();
}
function onSplitUp(e: PointerEvent) {
  if (!splitDragging) return;
  splitDragging = false;
  (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
}

/**
 * FEAT-067 叠加处理：处理结果可「转正」为新底图，继续叠加下一个算子。
 * - baseEl   ：当前底图（初始为原图；promote 后为最近一次结果）
 * - baseBlob ：当前底图字节（null = 原图文件，apply 时从 srcPath 取；非 null 直接上传，
 *              避免把中间结果落盘）
 * - steps    ：已叠加的算子链（状态栏展示）
 * 后端 /api/apply 天然接受任意图片字节，叠加纯前端实现，算子无需感知。
 */
const baseEl = new Image();
let baseBlob: Blob | null = null;
const steps = ref<string[]>([]);

const resultEl = new Image();
let resultBlob: Blob | null = null;
let objectUrls: string[] = [];
/** 微服务基址（studio_ensure 成功后缓存，后续请求直接复用） */
let base = "";

/** 蒙版离屏画布（原图分辨率，白=选中） */
const maskCanvas = document.createElement("canvas");
const maskCtx = maskCanvas.getContext("2d")!;

const displayCanvas = ref<HTMLCanvasElement | null>(null);
const overlayCanvas = ref<HTMLCanvasElement | null>(null);
/** FEAT-068：左右对比模式的「结果」画布（原图复用 displayCanvas） */
const sideResultCanvas = ref<HTMLCanvasElement | null>(null);
/** 舞台容器（fitDisplay 量可用宽度用：画布被 .ip-frame 包着，不能拿画布父元素量） */
const stageEl = ref<HTMLElement | null>(null);

/** 显示缩放：画布尺寸 / 原图尺寸 */
const scale = ref(1);
/** 拖拽框选的实时预览（显示坐标） */
const dragRect = ref<{ x: number; y: number; w: number; h: number } | null>(null);

const canApply = computed(() => !!srcPath.value && !processing.value && !!currentOp.value);
const panelStyle = computed(() => theme.cardStyle);

/* ---------------- 算子与参数 ---------------- */
async function ensureBase(): Promise<string> {
  if (!base) base = await invoke<string>("studio_ensure");
  return base;
}

/** 给 promise 套超时：后端命令无法取消，卡住时也必须回到失败态而不是永远「加载中」 */
function withTimeout<T>(p: Promise<T>, ms: number, msg: string): Promise<T> {
  let timer = 0;
  const timeout = new Promise<never>((_, reject) => {
    timer = window.setTimeout(() => reject(new Error(msg)), ms);
  });
  return Promise.race([p.finally(() => window.clearTimeout(timer)), timeout]);
}

/** 把异常翻译成用户能看懂的话（不把堆栈/英文错误原样抛出） */
function describeOpsError(e: unknown): string {
  const secs = OPS_TIMEOUT_MS / 1000;
  if (e instanceof DOMException && e.name === "AbortError") return `本机处理服务 ${secs}s 内无响应`;
  const m = String((e as Error)?.message ?? e);
  if (m.includes("超时")) return `本机处理服务 ${secs}s 内未就绪（首次启动较慢，或服务异常）`;
  if (/fetch|network|load failed|ECONNREFUSED|Failed/i.test(m)) return "无法连接本机处理服务";
  return m;
}

/** 拉取算子注册表（schema 驱动参数表单） */
async function loadOps() {
  opsState.value = "loading";
  opsError.value = "";
  const ac = new AbortController();
  const abortTimer = window.setTimeout(() => ac.abort(), OPS_TIMEOUT_MS);
  try {
    // 服务冷启动 + 注册表请求都套超时；超时/失败一律落到 error 态给重试
    const svc = await withTimeout(ensureBase(), OPS_TIMEOUT_MS, "服务启动超时");
    const resp = await fetch(`${svc}/api/ops`, { signal: ac.signal });
    if (!resp.ok) throw new Error(`服务返回 ${resp.status}`);
    const list = ((await resp.json()) as { ops?: OpDef[] }).ops ?? [];
    if (!list.length) throw new Error("服务未返回任何算子");
    ops.value = list;
    applyDefaults();
    opsState.value = "ready";
  } catch (e) {
    // 失败即丢弃缓存基址：重试时重新 ensure（服务可能已被回收或换了端口）
    base = "";
    opsError.value = describeOpsError(e);
    opsState.value = "error";
    statusMsg.value = "算子列表加载失败，可点「🔄 重试」";
  } finally {
    window.clearTimeout(abortTimer);
  }
}

/** 按 schema 默认值初始化当前算子的参数 */
function applyDefaults() {
  const o = currentOp.value;
  const init: Record<string, number | string | boolean> = {};
  if (o) for (const p of o.params) init[p.name] = p.default;
  values.value = init;
}

function selectOp(id: string) {
  opId.value = id;
  applyDefaults();
}

/** 参数显示：0~1 的参数按百分比显示（强度），其余直接显示数值 */
function displayValue(p: OpParam): string {
  const v = values.value[p.name];
  if (p.type === "number" && (p.step ?? 1) < 1) return `${Math.round(Number(v) * 100)}%`;
  return String(v);
}

/* ---------------- 载入图片 ---------------- */
async function chooseImage() {
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "选择要编辑的图片",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp"] }],
  });
  if (typeof picked !== "string") return;
  await loadImage(picked);
}

function loadImage(path: string): Promise<void> {
  return new Promise((resolve) => {
    baseEl.onload = () => {
      srcPath.value = path;
      // 叠加链复位：底图回到原图
      baseBlob = null;
      steps.value = [];
      maskCanvas.width = baseEl.naturalWidth;
      maskCanvas.height = baseEl.naturalHeight;
      // 赋值 width/height 已把画布重置为全透明（= 未选中），无需再铺底色
      hasMask.value = false;
      hasResult.value = false;
      viewMode.value = "edit";
      resultBlob = null;
      fitDisplay();
      redraw();
      statusMsg.value = `已载入 ${baseEl.naturalWidth}×${baseEl.naturalHeight}，框选或涂抹要处理的区域（不画=整图）`;
      resolve();
    };
    baseEl.onerror = () => {
      statusMsg.value = "图片加载失败";
      resolve();
    };
    baseEl.src = convertFileSrc(path);
  });
}

/** 显示画布适配：最长边贴合容器（保持比例）。以当前底图（可能是叠加结果）为准 */
function fitDisplay() {
  const disp = displayCanvas.value;
  if (!disp) return;
  const overlay = overlayCanvas.value;
  const side = sideResultCanvas.value;
  // 量舞台而不是画布父元素：画布父元素是 .ip-frame（尺寸由画布自己撑开），
  // 拿它当容器会形成「画布多宽容器就多宽」的闭环，窗口放大时画布缩不回去。
  const cw = stageEl.value?.clientWidth ?? 0;
  // 帧未稳定/容器被隐藏时宽度可能为 0，兜底避免算出 1×1 画布
  // FEAT-068：左右对比时两画布平分舞台宽度（中间留 16px 间隙）
  const availW = cw > 0 ? cw : 800;
  const twoUp = viewMode.value === "side" && hasResult.value;
  const maxW = twoUp ? (availW - 16) / 2 : availW;
  const maxH = Math.max(320, window.innerHeight * 0.58);
  const s = Math.min(1, maxW / baseEl.naturalWidth, maxH / baseEl.naturalHeight);
  scale.value = s;
  const w = Math.max(1, Math.round(baseEl.naturalWidth * s));
  const h = Math.max(1, Math.round(baseEl.naturalHeight * s));
  disp.width = w;
  disp.height = h;
  if (overlay) {
    overlay.width = w;
    overlay.height = h;
  }
  if (side) {
    side.width = w;
    side.height = h;
  }
}

function redraw() {
  const disp = displayCanvas.value;
  if (!disp || !baseEl.naturalWidth) return;
  const ctx = disp.getContext("2d")!;
  ctx.clearRect(0, 0, disp.width, disp.height);
  const overlay = overlayCanvas.value;
  const octx = overlay?.getContext("2d") ?? null;

  /* ---- FEAT-068 左右对比：左=当前底图，右=最近结果 ---- */
  if (viewMode.value === "side" && hasResult.value) {
    ctx.drawImage(baseEl, 0, 0, disp.width, disp.height);
    const rc = sideResultCanvas.value;
    if (rc) {
      const rctx = rc.getContext("2d")!;
      rctx.clearRect(0, 0, rc.width, rc.height);
      rctx.drawImage(resultEl, 0, 0, rc.width, rc.height);
    }
    return;
  }

  /* ---- FEAT-068 分割对比：底图整幅，结果裁剪到分割线左侧 ---- */
  if (viewMode.value === "split" && hasResult.value) {
    ctx.drawImage(baseEl, 0, 0, disp.width, disp.height);
    const sx = Math.round(disp.width * splitX.value);
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, 0, sx, disp.height);
    ctx.clip();
    ctx.drawImage(resultEl, 0, 0, disp.width, disp.height);
    ctx.restore();
    // 分割线画在 overlay：深色描边 + 白线，亮/暗底都可见
    if (overlay && octx) {
      octx.clearRect(0, 0, overlay.width, overlay.height);
      octx.strokeStyle = "rgba(0,0,0,.55)";
      octx.lineWidth = 4;
      octx.beginPath();
      octx.moveTo(sx, 0);
      octx.lineTo(sx, overlay.height);
      octx.stroke();
      octx.strokeStyle = "#fff";
      octx.lineWidth = 2;
      octx.beginPath();
      octx.moveTo(sx, 0);
      octx.lineTo(sx, overlay.height);
      octx.stroke();
    }
    return;
  }

  /* ---- 编辑态（原逻辑）：底图 + 蒙版红色叠加 + 框选预览 ---- */
  if (!overlay || !octx) return;
  ctx.drawImage(baseEl, 0, 0, disp.width, disp.height);

  // 蒙版红色半透明叠加
  // BUG-2026-0918-010：编辑态才画叠加（结果/对比视图上盖红色会让人以为处理失败）。
  // clearRect 已在上方执行，不会残留上一次的红色像素。
  octx.clearRect(0, 0, overlay.width, overlay.height);
  octx.save();
  octx.drawImage(maskCanvas, 0, 0, overlay.width, overlay.height);
  octx.globalCompositeOperation = "source-in";
  octx.fillStyle = "rgba(255,64,64,.5)";
  octx.fillRect(0, 0, overlay.width, overlay.height);
  octx.restore();
  // 框选拖拽预览
  if (dragRect.value) {
    const { x, y, w, h } = dragRect.value;
    octx.strokeStyle = "#ffd54a";
    octx.lineWidth = 1.5;
    octx.setLineDash([5, 4]);
    octx.strokeRect(x, y, w, h);
  }
}

/**
 * 开始编辑蒙版前调用：对比视图（side/split）下叠加不可见，此时若直接改蒙版，
 * 用户会「静默改掉而看不见」。故一律先切回编辑态，让他看得见自己在改什么。
 */
function beginEdit() {
  if (viewMode.value !== "edit") {
    viewMode.value = "edit";
    redraw();
  }
}

/* ---------------- 指针交互（显示坐标 → 原图坐标） ---------------- */
function posOf(e: PointerEvent) {
  const overlay = overlayCanvas.value!;
  const r = overlay.getBoundingClientRect();
  return {
    dx: e.clientX - r.left,
    dy: e.clientY - r.top,
    nx: (e.clientX - r.left) / scale.value,
    ny: (e.clientY - r.top) / scale.value,
  };
}

let painting = false;
let dragStart: { dx: number; dy: number; nx: number; ny: number } | null = null;

function onPointerDown(e: PointerEvent) {
  if (!srcPath.value) return;
  const overlay = overlayCanvas.value!;
  overlay.setPointerCapture(e.pointerId);
  // 结果态下叠加不可见，动手前先切回原图态（BUG-2026-0918-010）
  beginEdit();
  const p = posOf(e);
  painting = true;
  if (tool.value === "rect") {
    dragStart = p;
    dragRect.value = null;
  } else {
    stroke(p.nx, p.ny);
    redraw();
  }
}

function onPointerMove(e: PointerEvent) {
  if (!painting || !srcPath.value) return;
  const p = posOf(e);
  if (tool.value === "rect" && dragStart) {
    dragRect.value = {
      x: Math.min(dragStart.dx, p.dx),
      y: Math.min(dragStart.dy, p.dy),
      w: Math.abs(p.dx - dragStart.dx),
      h: Math.abs(p.dy - dragStart.dy),
    };
    redraw();
  } else if (tool.value !== "rect") {
    stroke(p.nx, p.ny);
    redraw();
  }
}

function onPointerUp(e: PointerEvent) {
  if (!painting) return;
  painting = false;
  const overlay = overlayCanvas.value!;
  overlay.releasePointerCapture(e.pointerId);
  if (tool.value === "rect" && dragStart) {
    const p = posOf(e);
    const x = Math.min(dragStart.nx, p.nx);
    const y = Math.min(dragStart.ny, p.ny);
    const w = Math.abs(p.nx - dragStart.nx);
    const h = Math.abs(p.ny - dragStart.ny);
    dragRect.value = null;
    if (w > 2 && h > 2) {
      maskCtx.fillStyle = "#fff";
      maskCtx.fillRect(x, y, w, h);
      hasMask.value = true;
      statusMsg.value = "已加入框选区域";
    }
    dragStart = null;
  }
  redraw();
}

/** 画笔/橡皮落墨（原图坐标，半径随缩放换算）。橡皮 = 擦回透明（未选中） */
function stroke(nx: number, ny: number) {
  const r = brushSize.value / 2 / scale.value;
  maskCtx.globalCompositeOperation = tool.value === "eraser" ? "destination-out" : "source-over";
  maskCtx.fillStyle = "#fff";
  maskCtx.beginPath();
  maskCtx.arc(nx, ny, Math.max(1, r), 0, Math.PI * 2);
  maskCtx.fill();
  hasMask.value = true;
}

function clearMask() {
  beginEdit();
  maskCtx.clearRect(0, 0, maskCanvas.width, maskCanvas.height);
  hasMask.value = false;
  redraw();
  statusMsg.value = "已清空蒙版（将按整图处理）";
}

function selectAll() {
  beginEdit();
  maskCtx.fillStyle = "#fff";
  maskCtx.fillRect(0, 0, maskCanvas.width, maskCanvas.height);
  hasMask.value = true;
  redraw();
  statusMsg.value = "已全选（整图处理）";
}

/* ---------------- 调用微服务 ---------------- */
function canvasBlob(canvas: HTMLCanvasElement, type: string): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("canvas 导出失败"))), type),
  );
}

async function apply() {
  const op = currentOp.value;
  if (!srcPath.value || processing.value || !op) return;
  processing.value = true;
  statusMsg.value = "正在处理…";
  try {
    // 1) 确保微服务就绪（Rust 侧探活/收养/启动）
    const svc = await ensureBase();

    // 2) 底图字节：有叠加链时直接上传当前底图（最近一次结果），否则从原图文件取
    //    （asset 协议 fetch 拿原始文件，不经 canvas 重编码）
    const imgBlob = baseBlob ?? (await (await fetch(convertFileSrc(srcPath.value))).blob());

    const form = new FormData();
    form.append("op", op.id);
    form.append("params", JSON.stringify(values.value));
    form.append("image", imgBlob, "image.jpg");

    // 3) 蒙版可选：只在画过蒙版时导出（画布内是「透明=未选、白=选中」，
    //    铺黑合成后端约定的灰度蒙版 PNG；不传 mask 即整图处理）
    if (hasMask.value) {
      const exportCanvas = document.createElement("canvas");
      exportCanvas.width = maskCanvas.width;
      exportCanvas.height = maskCanvas.height;
      const ectx = exportCanvas.getContext("2d")!;
      ectx.fillStyle = "#000";
      ectx.fillRect(0, 0, exportCanvas.width, exportCanvas.height);
      ectx.drawImage(maskCanvas, 0, 0);
      form.append("mask", await canvasBlob(exportCanvas, "image/png"), "mask.png");
    }

    const resp = await fetch(`${svc}/api/apply`, { method: "POST", body: form });
    if (!resp.ok) {
      const detail = await resp.text().catch(() => "");
      throw new Error(`服务返回 ${resp.status}${detail ? `: ${detail.slice(0, 200)}` : ""}`);
    }
    resultBlob = await resp.blob();

    // 4) 展示结果：自动进入分割对比（编辑态时）；已在对比视图则保持
    const url = URL.createObjectURL(resultBlob);
    objectUrls.push(url);
    resultEl.onload = () => {
      hasResult.value = true;
      if (viewMode.value === "edit") viewMode.value = "split";
      else {
        fitDisplay();
        redraw();
      }
      statusMsg.value = `处理完成（${op.label}）——可保存、按 Y 左右对比或拖动分割线对比，或「以结果继续」叠加下一个算子`;
    };
    resultEl.src = url;
    steps.value.push(hasMask.value ? `${op.label}(选区)` : op.label);
  } catch (e) {
    statusMsg.value = `处理失败：${String(e)}`;
  } finally {
    processing.value = false;
  }
}

/**
 * FEAT-067：把最近一次结果「转正」为新底图，清空蒙版后继续叠加下一个算子。
 * 结果与底图同分辨率（算子不改尺寸），蒙版画布直接按结果尺寸重置。
 */
function promoteResult() {
  if (!resultBlob) return;
  const url = URL.createObjectURL(resultBlob);
  objectUrls.push(url);
  baseBlob = resultBlob;
  baseEl.onload = () => {
    maskCanvas.width = baseEl.naturalWidth;
    maskCanvas.height = baseEl.naturalHeight;
    hasMask.value = false;
    hasResult.value = false;
    viewMode.value = "edit";
    resultBlob = null;
    fitDisplay();
    redraw();
    statusMsg.value = `已以结果为底图（${steps.value.length} 步）——重选蒙版后可继续叠加`;
  };
  baseEl.src = url;
}

/** 丢弃叠加链，回到原图重新开始（已保存的结果不受影响） */
function revertToOriginal() {
  if (!srcPath.value) return;
  baseBlob = null;
  steps.value = [];
  baseEl.onload = () => {
    maskCanvas.width = baseEl.naturalWidth;
    maskCanvas.height = baseEl.naturalHeight;
    hasMask.value = false;
    hasResult.value = false;
    viewMode.value = "edit";
    resultBlob = null;
    fitDisplay();
    redraw();
    statusMsg.value = "已回到原图";
  };
  baseEl.src = convertFileSrc(srcPath.value);
}

async function saveResult() {
  if (!resultBlob) return;
  const picked = await saveFileDialog({
    title: "保存处理结果",
    defaultPath: `${opId.value}.jpg`,
    filters: [{ name: "JPEG 图片", extensions: ["jpg"] }],
  });
  if (typeof picked !== "string") return;
  const buf = new Uint8Array(await resultBlob.arrayBuffer());
  await invoke("studio_save_result", { path: picked, data: Array.from(buf) });
  statusMsg.value = `已保存：${picked}`;
}

/* ---------------- 生命周期 ---------------- */
function onResize() {
  if (srcPath.value) {
    fitDisplay();
    redraw();
  }
}
window.addEventListener("resize", onResize);

onMounted(async () => {
  await loadOps();
  // FEAT-065：从灯箱带图进来时自动载入（打开即用）。
  // nextTick 等画布真正挂载 —— loadImage 的 onload 里会调 fitDisplay()，
  // 它依赖画布父容器的宽度。
  if (props.initialPath) {
    await nextTick();
    await loadImage(props.initialPath);
  }
});

onBeforeUnmount(() => {
  window.removeEventListener("resize", onResize);
  window.removeEventListener("keydown", onCompareKey);
  objectUrls.forEach((u) => URL.revokeObjectURL(u));
  objectUrls = [];
});
</script>

<template>
  <section class="ip" :style="panelStyle">
    <header class="ip-head">
      <button class="ip-btn" type="button" @click="chooseImage">📂 选择图片</button>
      <div class="ip-tools">
        <button
          v-for="t in [
            { id: 'rect', label: '▭ 框选' },
            { id: 'brush', label: '🖌 画蒙版' },
            { id: 'eraser', label: '🧽 橡皮' },
          ]"
          :key="t.id"
          type="button"
          class="ip-tool"
          :class="{ on: tool === t.id }"
          @click="tool = t.id as Tool"
        >
          {{ t.label }}
        </button>
        <label v-if="tool !== 'rect'" class="ip-brush">
          笔刷
          <input v-model.number="brushSize" type="range" min="4" max="80" step="2" />
          <b>{{ brushSize }}</b>
        </label>
        <button class="ip-btn" type="button" @click="selectAll">全选</button>
        <button class="ip-btn" type="button" @click="clearMask">清空蒙版</button>
      </div>
    </header>

    <!-- 算子选择：均衡化 / 模糊（模糊下再选具体方式） -->
    <div class="ip-ops">
      <div class="ip-seg">
        <button v-if="eqOp" type="button" :class="{ on: group === 'eq' }" @click="selectOp(eqOp.id)">
          {{ eqOp.label }}
        </button>
        <button
          type="button"
          :class="{ on: group === 'blur' }"
          :disabled="!blurOps.length"
          @click="blurOps.length && selectOp(blurOps[0].id)"
        >
          模糊
        </button>
      </div>
      <div v-if="group === 'blur'" class="ip-seg ip-sub">
        <button
          v-for="o in blurOps"
          :key="o.id"
          type="button"
          :class="{ on: opId === o.id }"
          @click="selectOp(o.id)"
        >
          {{ o.label }}
        </button>
      </div>
    </div>

    <div ref="stageEl" class="ip-stage" :class="{ 'ip-stage-side': viewMode === 'side' && hasResult }">
      <!-- FEAT-068 左右对比：左右两帧各画一张（Lightroom Y 视图） -->
      <template v-if="viewMode === 'side' && hasResult && srcPath">
        <div class="ip-frame ip-frame-half">
          <canvas ref="displayCanvas" class="ip-canvas"></canvas>
          <span class="ip-tag">原图</span>
        </div>
        <div class="ip-frame ip-frame-half">
          <canvas ref="sideResultCanvas" class="ip-canvas"></canvas>
          <span class="ip-tag ip-tag-right">结果</span>
        </div>
      </template>
      <!-- BUG-2026-0918-008：画布必须包在 .ip-frame 里、由 display 画布在文档流中撑开高度。
           画布直接当 .ip-stage 的绝对定位子元素时不参与父容器高度计算，舞台高度只剩
           min-height，图比它高就向下溢出、盖住参数行与底栏。 -->
      <div v-else ref="splitFrameEl" class="ip-frame">
        <canvas ref="displayCanvas" class="ip-canvas"></canvas>
        <canvas
          ref="overlayCanvas"
          class="ip-overlay"
          :class="{ 'ip-crosshair': tool === 'rect', 'ip-brush-cursor': tool !== 'rect', 'ip-noevents': viewMode === 'split' }"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="onPointerUp"
          @pointercancel="onPointerUp"
        ></canvas>
        <!-- FEAT-068 分割对比：可拖动的分割线（左原图 / 右结果） -->
        <div
          v-if="viewMode === 'split' && hasResult"
          class="ip-split-divider"
          :style="{ left: splitX * 100 + '%' }"
          title="拖动对比原图与结果"
          @pointerdown="onSplitDown"
          @pointermove="onSplitMove"
          @pointerup="onSplitUp"
          @pointercancel="onSplitUp"
        >
          <span class="ip-tag">原图</span>
          <span class="ip-tag ip-tag-right">结果</span>
        </div>
      </div>
      <div v-if="!srcPath" class="ip-empty">📂 先选择一张图片开始编辑</div>
    </div>

    <!-- 参数面板：由 /api/ops 的 schema 自动渲染；三态显式（加载中 / 失败可重试 / 就绪） -->
    <div v-if="opsState === 'ready' && currentOp" class="ip-params">
      <template v-for="p in currentOp.params" :key="p.name">
        <div v-if="p.type === 'enum'" class="ip-field">
          <span class="ip-field-label">{{ p.label }}</span>
          <div class="ip-seg">
            <button
              v-for="c in p.choices ?? []"
              :key="c.value"
              type="button"
              :class="{ on: values[p.name] === c.value }"
              @click="values[p.name] = c.value"
            >
              {{ c.label }}
            </button>
          </div>
        </div>
        <label v-else-if="p.type === 'number'" class="ip-range">
          <span>{{ p.label }}</span>
          <input v-model.number="values[p.name]" type="range" :min="p.min" :max="p.max" :step="p.step" />
          <b>{{ displayValue(p) }}</b>
        </label>
        <label v-else class="ip-check">
          <input v-model="values[p.name]" type="checkbox" />
          {{ p.label }}
        </label>
      </template>
      <span v-if="!hasMask" class="ip-hint">未画蒙版 → 按整图处理</span>
    </div>
    <div v-else-if="opsState === 'error'" class="ip-ops-error">
      <span class="ip-status err">算子列表加载失败：{{ opsError }}</span>
      <button class="ip-btn" type="button" @click="loadOps">🔄 重试</button>
    </div>
    <div v-else class="ip-hint">算子列表加载中…（首次需启动本机处理服务，约 1~2 秒）</div>

    <footer class="ip-foot">
      <span class="ip-status" :class="{ err: statusMsg.startsWith('处理失败') || statusMsg.includes('失败') }">{{
        statusMsg
      }}</span>
      <div class="ip-actions">
        <!-- FEAT-068：三态视图切换（替代旧「对比结果」复选框） -->
        <div v-if="hasResult" class="ip-seg ip-viewseg">
          <button type="button" :class="{ on: viewMode === 'edit' }" @click="setView('edit')" title="返回编辑蒙版">✏️ 编辑</button>
          <button type="button" :class="{ on: viewMode === 'side' }" @click="setView('side')" title="快捷键 Y">⬒ 左右对比</button>
          <button type="button" :class="{ on: viewMode === 'split' }" @click="setView('split')" title="拖动中间分割线">⬟ 分割对比</button>
        </div>
        <button v-if="hasResult" class="ip-btn" type="button" @click="promoteResult">
          🔗 以结果继续
        </button>
        <button v-if="baseBlob" class="ip-btn" type="button" @click="revertToOriginal">
          ↩️ 回到原图
        </button>
        <button class="ip-btn" type="button" :disabled="!hasResult" @click="saveResult">💾 保存结果</button>
        <button class="ip-btn ip-primary" type="button" :disabled="!canApply" @click="apply">
          {{ processing ? "处理中…" : `✨ 应用${currentOp ? currentOp.label : ""}` }}
        </button>
      </div>
    </footer>
    <div v-if="steps.length" class="ip-chain">叠加链：{{ steps.join(" → ") }}</div>
  </section>
</template>

<style scoped>
.ip {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border-radius: var(--radius-lg, 16px);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.ip-head,
.ip-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.ip-tools {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.ip-btn {
  height: 34px;
  padding: 0 14px;
  font-size: 13px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.12);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 9px;
  cursor: pointer;
  transition: background 0.2s;
}

.ip-btn:hover:not(:disabled) {
  background: rgba(120, 120, 130, 0.22);
}

.ip-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.ip-primary {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-primary:hover:not(:disabled) {
  background: #2f5de0;
}

.ip-tool {
  height: 34px;
  padding: 0 12px;
  font-size: 13px;
  color: var(--color-text-2);
  background: transparent;
  border: 1px solid rgba(120, 120, 130, 0.25);
  border-radius: 9px;
  cursor: pointer;
}

.ip-tool.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-brush {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-brush input[type="range"] {
  width: 110px;
}

.ip-ops {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.ip-stage {
  position: relative;
  display: flex;
  justify-content: center;
  /* 必须 center/flex-start：默认 stretch 会把 .ip-frame 拉伸到 min-height，
     里面的画布依旧溢出（BUG-2026-0918-008 的成因之一） */
  align-items: center;
  /* min-height 只在空态（无图）时起作用；有图时舞台高度由 .ip-frame 撑开 */
  min-height: 320px;
  border-radius: 12px;
  background:
    repeating-conic-gradient(rgba(128, 128, 128, 0.12) 0% 25%, transparent 0% 50%) 50% / 22px 22px;
}

/* 画布框：尺寸由文档流中的 display 画布撑开 → 舞台高度随图变化，不再压缩/溢出 */
.ip-frame {
  position: relative;
  flex: 0 0 auto;
}

/* ---- FEAT-068 对比视图 ---- */
/* 左右对比：舞台两帧并排，中间留 16px 间隙 */
.ip-stage-side {
  gap: 16px;
}

.ip-frame-half {
  position: relative;
  flex: 0 0 auto;
}

/* 对比标签：小徽标贴在画布角上 */
.ip-tag {
  position: absolute;
  top: 8px;
  left: 8px;
  padding: 2px 8px;
  font-size: 11px;
  font-weight: 600;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border-radius: 6px;
  pointer-events: none;
  z-index: 2;
}

.ip-tag-right {
  left: auto;
  right: 8px;
}

/* 分割对比：分割线手柄（线本身画在 overlay 上，这里提供拖拽热区） */
.ip-split-divider {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 14px;
  margin-left: -7px;
  cursor: ew-resize;
  touch-action: none;
  z-index: 3;
}

/* 对比视图下 overlay 不再接收指针事件（防止在看不见的地方偷偷改蒙版） */
.ip-overlay.ip-noevents {
  pointer-events: none;
}

/* 底部视图切换组：与其他按钮对齐 */
.ip-viewseg {
  align-items: center;
}

.ip-canvas,
.ip-overlay {
  border-radius: 8px;
}

.ip-canvas {
  display: block;
}

/* 叠加层贴合 display 画布：宽高由 JS 同步写入的 canvas 属性决定，故只钉左上角，不写 inset:0 */
.ip-overlay {
  position: absolute;
  top: 0;
  left: 0;
}

.ip-overlay.ip-crosshair {
  cursor: crosshair;
}

.ip-overlay.ip-brush-cursor {
  cursor: pointer;
}

.ip-empty {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  color: var(--color-text-2);
}

.ip-params {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
}

.ip-field {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-field-label {
  white-space: nowrap;
}

.ip-seg {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.ip-sub {
  padding-left: 4px;
}

.ip-seg button {
  height: 32px;
  padding: 0 12px;
  font-size: 12.5px;
  color: var(--color-text-2);
  background: rgba(120, 120, 130, 0.1);
  border: 1px solid rgba(120, 120, 130, 0.22);
  border-radius: 9px;
  cursor: pointer;
}

.ip-seg button.on {
  color: #fff;
  background: #3a6cf5;
  border-color: #3a6cf5;
}

.ip-seg button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.ip-range {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12.5px;
  color: var(--color-text-2);
}

.ip-range input[type="range"] {
  width: 140px;
}

.ip-range b {
  min-width: 42px;
  text-align: right;
  color: var(--color-text);
}

.ip-hint {
  font-size: 12px;
  color: var(--color-text-2);
  opacity: 0.8;
}

/* 算子列表加载失败：明确报错 + 重试入口（不许静默停留在「加载中」） */
.ip-ops-error {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
  padding: 8px 12px;
  border: 1px solid rgba(229, 72, 77, 0.35);
  border-radius: 10px;
  background: rgba(229, 72, 77, 0.08);
}

.ip-status {
  font-size: 12.5px;
  color: var(--color-text-2);
  flex: 1;
  min-width: 200px;
}

.ip-status.err {
  color: #e5484d;
}

.ip-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.ip-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--color-text-2);
  cursor: pointer;
}

/* 叠加链展示：当前已应用的算子序列 */
.ip-chain {
  font-size: 12px;
  color: var(--color-text-2);
  opacity: 0.85;
  text-align: right;
}
</style>
