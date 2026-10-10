<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useNotify } from "../../../composables/useNotify";
import { useEditStore } from "../../../stores/edit";
import type { AdjustmentRecipe } from "../../../types/edit";
import { cloneRecipe } from "../../../types/edit";
import CropOverlay from "./CropOverlay.vue";
import EditorPanel from "./EditorPanel.vue";
import HistogramPanel from "./HistogramPanel.vue";
import MaskOverlay from "./MaskOverlay.vue";
import type { Tool as MaskTool } from "./MaskOverlay.vue";
import StudioOpsPanel from "./StudioOpsPanel.vue";
import { useStudioOps } from "../useStudioOps";

/**
 * 创意工坊 · 基准调色（FEAT-099）宿主组件
 * =====================================================================
 * 布局（对齐 Lightroom 的「Develop」模块）：
 *
 * ```text
 * ┌──────────────────────────────────────────────────────────────┐
 * │ 工具条：打开 保存方案 导入 导出 撤销 重做 自动 对比            │
 * ├──────────┬─────────────────────────────┬─────────────────────┤
 * │ 直方图   │        预览画布 + 裁剪层     │  调整面板（schema） │
 * │          │                             │                     │
 * ├──────────┴─────────────────────────────┴─────────────────────┤
 * │ 状态栏：尺寸 · 档位 · 渲染耗时 · 配方状态                     │
 * └──────────────────────────────────────────────────────────────┘
 * ```
 *
 * 「左直方图 + 中画布 + 右面板」这个三栏结构是 Lightroom 的标志性布局：
 * 直方图必须在**视线不移动**的情况下与画布同时可见，否则用户无法边看分布
 * 边调曝光 —— 这是调色里最高频的反馈闭环。
 *
 * ## 宿主只做四件事
 *
 * 1. 布局（上面这个三栏）；
 * 2. 画布适配（把预览图按容器尺寸等比放置，并算出裁剪层需要的 `frame`）；
 * 3. 文件对话框（打开/保存方案/导出）；
 * 4. 快捷键（Ctrl+Z / Ctrl+Shift+Z / Y / 0 / \\）。
 *
 * 参数编辑、渲染调度、撤销栈全在 store 与 EditorPanel 里。
 */
const props = defineProps<{ initialPath?: string }>();
const notify = useNotify();
const store = useEditStore();

/* ---------------- 折叠状态持久化 ---------------- */

/**
 * 面板折叠状态按「图源路径」隔离。
 *
 * 用路径的简单散列而不是路径本身：路径可能很长（含用户目录名），
 * 直接当 localStorage key 既占空间也把私人路径写进存储。
 */
function foldKey(path: string): string {
  let h = 2166136261;
  for (let i = 0; i < path.length; i++) {
    h ^= path.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return `pm-edit-fold:${(h >>> 0).toString(36)}`;
}

const collapsed = ref<Record<string, boolean>>(loadFold());
const foldKeyRef = ref("");

function loadFold(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(foldKey(props.initialPath ?? ""));
    return raw ? (JSON.parse(raw) as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

function saveFold(): void {
  try {
    localStorage.setItem(foldKey(foldKeyRef.value), JSON.stringify(collapsed.value));
  } catch {
    /* 忽略（隐私模式 / 配额满） */
  }
}

watch(collapsed, saveFold, { deep: true });

/* ---------------- 画布适配 ---------------- */

const stageEl = ref<HTMLElement | null>(null);
const stageBox = ref({ w: 0, h: 0 });
/** 预览图在 stage 内的显示矩形（stage 局部坐标） */
const imageFrame = ref({ x: 0, y: 0, w: 0, h: 0 });

const aspectRatio = computed(() => {
  const { w, h } = store.previewSize.w ? store.previewSize : { w: store.originalWidth, h: store.originalHeight };
  return w > 0 && h > 0 ? w / h : 3 / 2;
});

/** 左右对比时两格之间的间隙（像素）。 */
const SIDE_GAP = 16;

/** 是否处于「左右并排」对比。 */
const sideMode = computed(
  () => store.compareMode === "side" && store.hasImage,
);

/** 右格（结果）的显示矩形。左右两格等宽等高，只差一个间隙。 */
const sideRightFrame = computed(() => ({
  x: imageFrame.value.x + imageFrame.value.w + SIDE_GAP,
  y: imageFrame.value.y,
  w: imageFrame.value.w,
  h: imageFrame.value.h,
}));

/** 预览图按「适配容器」放置（contain）。 */
function layout(): void {
  const el = stageEl.value;
  if (!el) return;
  const r = el.getBoundingClientRect();
  stageBox.value = { w: r.width, h: r.height };
  const pad = 16;
  const availW = Math.max(40, r.width - pad * 2);
  const availH = Math.max(40, r.height - pad * 2);
  // 并排时两格平分宽度（各扣一半间隙），单幅时用满
  const paneW = sideMode.value ? Math.max(40, (availW - SIDE_GAP) / 2) : availW;
  let w = paneW;
  let h = w / aspectRatio.value;
  if (h > availH) {
    h = availH;
    w = h * aspectRatio.value;
  }
  // 并排时整体（含间隙）居中，否则只居中单格会整体偏左
  const totalW = sideMode.value ? w * 2 + SIDE_GAP : w;
  imageFrame.value = {
    x: (r.width - totalW) / 2,
    y: (r.height - h) / 2,
    w,
    h,
  };
}

/* ---------------- 局部蒙版 ---------------- */

const maskOverlay = ref<InstanceType<typeof MaskOverlay> | null>(null);

/** 显示缩放：显示尺寸 / 蒙版像素。蒙版层据此把指针坐标换算成蒙版像素。 */
const maskScale = computed(() => {
  const w = store.previewSize.w;
  if (!w || !imageFrame.value.w) return 1;
  return imageFrame.value.w / w || 1;
});

/** 蒙版画完：提交 PNG。失败时 maskError 由 store 写入。 */
async function onMaskChange(): Promise<void> {
  const ov = maskOverlay.value;
  if (!ov) return;
  const blob = await ov.toPngBlob();
  if (!blob) return;
  await store.commitMask(await blob.arrayBuffer());
}

/**
 * 进入蒙版模式时把磁盘上的蒙版画回画布。
 *
 * 必须等下一帧：`<MaskOverlay v-if="store.masking">` 意味着组件在
 * `masking` 变 true 的**同一轮渲染里**才挂载，此刻 `maskOverlay.value`
 * 仍是 null，直接调 `loadFromPng` 什么都不会发生。
 */
/**
 * 进出「左右并排」要重排：单幅时用满舞台宽度，并排时两格各占一半。
 * 不重排的话切过去两格会按旧的整幅宽度画，右格直接顶出容器。
 */
watch(sideMode, () => layout());

watch(
  () => store.masking,
  async (on) => {
    if (!on) return;
    store.cropping = false;
    await nextTick();
    const ov = maskOverlay.value;
    const png = store.maskPng;
    if (ov && png) await ov.loadFromPng(new Blob([png], { type: "image/png" }));
    else ov?.clearAll();
  },
);

/** 蒙版开关：与裁剪互斥（两者都要独占画布手势）。 */
/** 蒙版工具由宿主持有，画布层只读 —— 否则面板与画布的工具会各说各话。 */
const maskTool = ref<MaskTool>("rect");
const maskBrush = ref(48);
/** 「新建蒙版」弹层是否展开（对齐 Lightroom：点按钮 → 先选新建方式）。 */
const maskPickerOpen = ref(false);

const MASK_TOOL_LABEL: Record<MaskTool, string> = {
  rect: "矩形选区",
  brush: "画笔",
  eraser: "橡皮擦",
};

const MASK_TOOL_HINT: Record<MaskTool, string> = {
  rect: "拖出一个矩形区域",
  brush: "按住涂抹，可反复叠加",
  eraser: "擦掉已选中的部分",
};

/**
 * 选一种新建方式 → 关闭弹层并进入绘制模式。
 *
 * 先选工具再进画布（Lightroom 的路径），而不是先进画布再换工具 ——
 * 后者会让用户在还没有蒙版时于画布上乱划一通，才发现有工具用错了。
 */
function pickMaskTool(t: MaskTool): void {
  maskTool.value = t;
  maskPickerOpen.value = false;
  if (!store.masking) {
    store.masking = true;
    store.cropping = false;
  }
}

/**
 * 点「🎯 局部」：开/关「新建蒙版」弹层。
 *
 * 不直接进绘制模式 —— 先让用户选新建方式（Lightroom 的路径）。
 * 已有蒙版时则是「继续编辑选区」，直接回画布。
 */
function toggleMask(): void {
  if (!store.hasImage) return;
  if (store.masking) {
    // 已在绘制 → 收工退出。蒙版像素早就提交了，这里退出的只是手势层。
    store.masking = false;
    maskPickerOpen.value = false;
    return;
  }
  // 未在绘制 → 开/关「新建蒙版」弹层，让用户先选新建方式
  store.cropping = false;
  maskPickerOpen.value = !maskPickerOpen.value;
}

/* ---------------- 算子（Python 微服务） ---------------- */

// 解构成独立 ref：模板里直接用 `studioOps.value` 不如 `studioOps` 直观，
// 且解构后仍是 ref（模板会自动解包），响应式不丢。
const {
  ops: studioOps,
  loading: opsLoading,
  error: opsError,
  loadOps: loadStudioOps,
  apply: applyStudioOp,
} = useStudioOps();

/* ---------------- 算子链编排 ---------------- */

/**
 * 重跑整条算子链：Rust 底图 → op₁ → op₂ → … → 结果。
 *
 * ## 为什么由前端编排而不是后端
 *
 * 算子在 Python 微服务里，而底图在 Rust 进程内 —— 两者只能由前端串起来
 * （前端同时持有两边的句柄）。后端若要代跑，就得再开一条 Rust↔Python 的
 * 通道，把前端已有的 HTTP 调用搬进 Rust，纯属绕路。
 *
 * ## 为什么链结果不进 store
 *
 * 链结果是**烘焙产物**，不是真相源。真相源是 `recipe.ops`。把 blob 塞进
 * store 会带来两个问题：一是撤销/重做时它与配方不同步（配方回到 v1、blob
 * 还是 v2 的结果）；二是它无法序列化，导出时要重跑一遍。保持
 * 「配方 = 真相、blob = 派生」，两边就不会漂。
 */
const opsResultBlob = shallowRef<Blob | null>(null);
const opsRunning = ref(false);
const opsRunError = ref("");
/** 链的版本号：只有配方里 steps 变化才需要重跑 */
let opsChainToken = 0;

/** 链结果的 object URL。由 `opsResultBlob` 派生，随它自动换。 */
const opsResultUrl = ref("");
/** 当前 object URL 对应的 blob（用来判断是否需要重新创建） */
let opsResultUrlFor: Blob | null = null;

/** 链结果 blob 变化时同步 object URL（否则每次都创建一个 URL 且从不回收）。 */
watch(opsResultBlob, (b) => {
  if (opsResultUrlFor === b) return;
  if (opsResultUrl.value) URL.revokeObjectURL(opsResultUrl.value);
  opsResultUrlFor = b;
  opsResultUrl.value = b ? URL.createObjectURL(b) : "";
});

/** 已记录的算子步骤数（状态栏展示用）。 */
const opsStepCount = computed(() => store.recipe.ops.length);

/**
 * 每一步是否只作用于选区。
 *
 * 取自 `params.mask` —— **存在参数里**而不是独立的组件状态，因为它是
 * 这一步处理的一部分，必须随步骤一起进 sidecar、一起撤销。关掉局部时
 * 该步就退化成整图处理，重开照片后行为一致。
 *
 * 缺省 true：有蒙版时优先按选区处理，更符合「局部调整」的预期。
 * 判据用 `!== false`：老配方没有这个字段时按 true 走。
 */
function stepUsesMask(params: Record<string, number | string | boolean>): boolean {
  return params.mask !== false;
}

/** 跑链时的一步（`masked` 决定这次调用传不传蒙版）。 */
interface ActiveStep {
  op: string;
  params: Record<string, number | string | boolean>;
  masked: boolean;
}

/** 当前要跑链的步骤（跳过被禁用的）。 */
function activeSteps(): ActiveStep[] {
  const hasMask = store.hasMask;
  return store.recipe.ops
    .filter((s) => s.enabled && s.op)
    .map((s) => {
      const params = { ...s.params };
      // `mask` 是本项目自加的编排开关，**不是** Python 侧认识的算子参数 ——
      // 必须在发请求前摘掉，否则会混进 params 传给 /api/apply。后端
      // `_coerce_params` 按 schema 整流会忽略未知键，但混进去会让服务日志
      // 与排障失真。
      const masked = stepUsesMask(params);
      delete params.mask;
      return { op: s.op, params, masked: masked && hasMask };
    });
}

/**
 * 跑整条链并更新显示。
 *
 * 步骤为空时**清空链产物**，让画面退回纯调色结果 —— 否则删光步骤后屏幕上
 * 还留着旧算子的效果，而配方里已经没有了。
 */
async function runOpsChain(): Promise<void> {
  const steps = activeSteps();
  if (!store.hasImage || !steps.length) {
    opsResultBlob.value = null;
    return;
  }

  const token = ++opsChainToken;
  opsRunning.value = true;
  opsRunError.value = "";
  try {
    const base = await opsInput();
    if (!base) throw new Error("没有可用的底图，请先打开照片");

    // 蒙版只取一次：整条链共用同一份选区，逐步重取是浪费。
    // 是否**使用**由每一步自己的 `masked` 决定（有的步整图、有的步局部）。
    const mask = steps.some((s) => s.masked) ? await opsMask() : null;

    let img = base;
    for (const s of steps) {
      img = await applyStudioOp(s.op, s.params, img, s.masked ? mask : null);
      // 期间用户可能又改了配方：让新的一次调用接手，避免结果错位
      if (token !== opsChainToken) return;
    }
    opsResultBlob.value = img;
  } catch (e) {
    if (token !== opsChainToken) return;
    opsRunError.value = String((e as Error)?.message ?? e);
    // 失败时退回底图：宁可少显示效果，也不要给一张来源不明的图
    opsResultBlob.value = null;
  } finally {
    if (token === opsChainToken) opsRunning.value = false;
  }
}

/** 配方里的步骤变了 → 重跑链。 */
function onOpsChanged(): void {
  void runOpsChain();
}

/**
 * 底图变了 → 链结果立刻作废。
 *
 * 拖动滑块时每帧都重跑整条链是不可能的（一次算子 100~300ms），所以
 * 这里只作废并显示底图，静止后按静默期重跑。
 */
let opsSettleTimer = 0;
function invalidateOpsChain(): void {
  opsChainToken++;
  opsResultBlob.value = null;
  if (!activeSteps().length) return;
  window.clearTimeout(opsSettleTimer);
  // 静默期略长于 store 的高清重渲（140ms）：等底图稳定再跑链，
  // 否则会基于草稿底图算一次，随后又被作废，白跑一趟
  opsSettleTimer = window.setTimeout(() => void runOpsChain(), 260);
}

onMounted(() => {
  // 预热：进编辑器就拉算子表，让用户点开面板时已经就绪。
  // 失败静默 —— 面板里会显示错误态。
  void loadStudioOps();
});

onBeforeUnmount(() => {
  if (opsResultUrl.value) URL.revokeObjectURL(opsResultUrl.value);
  opsResultUrl.value = "";
  opsResultUrlFor = null;
});

/**
 * 取 Rust 当前渲染出的预览字节作为算子输入。
 *
 * 必须是**已调色**的渲染结果：算子作用在它上面，用户看到的预览
 * 才是「调色 + 算子」的最终效果。若传原图，预览里有算子效果、
 * 导出时调色会把它盖掉 —— 那种不一致比算子慢更让人困惑。
 */
async function opsInput(): Promise<Blob | null> {
  // 预览刚渲染完时 previewBlob 已是最新；渲染中则用当前 blob，
  // 略微滞后一次渲染但语义正确（总比拿原图强）。
  if (store.previewBlob) return store.previewBlob;
  await store.renderNow("preview");
  return store.previewBlob;
}

/** 取当前蒙版 PNG（与 Rust 侧共用同一份，不重新绘制）。 */
async function opsMask(): Promise<Blob | null> {
  const ov = maskOverlay.value;
  const png = store.maskPng;
  if (!ov || !png || !store.hasMask) return null;
  // 画布可能有尚未提交的改动，先刷新一份
  return await ov.toPngBlob();
}

const ro = ref<ResizeObserver | null>(null);
onMounted(() => {
  store.revive();
  void store.ensureSchema();
  layout();
  if (stageEl.value && typeof ResizeObserver !== "undefined") {
    const obs = new ResizeObserver(() => layout());
    obs.observe(stageEl.value);
    ro.value = obs;
  }
  // 配方提交后作废算子链（输入变了）。返回的取消函数在卸载时调用。
  offCommit = store.onCommit(() => invalidateOpsChain());
  if (props.initialPath) void doOpen(props.initialPath);
});

onBeforeUnmount(() => {
  ro.value?.disconnect();
  ro.value = null;
  offCommit?.();
  offCommit = null;
  window.clearTimeout(opsSettleTimer);
  store.dispose();
});

/** 取消「配方提交」订阅（避免宿主卸载后仍回调已销毁的链）。 */
let offCommit: (() => void) | null = null;

/* ---------------- 打开 / 关闭 ---------------- */

async function chooseImage(): Promise<void> {
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "选择要调色的照片",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"] }],
  });
  if (typeof picked !== "string") return;
  await doOpen(picked);
}

async function doOpen(path: string): Promise<void> {
  foldKeyRef.value = path;
  collapsed.value = loadFold();
  const ok = await store.open(path);
  if (!ok) {
    notify.error("打开照片失败", store.renderError);
    return;
  }
  layout();
  notify.success(
    "已打开",
    `${store.originalWidth}×${store.originalHeight} · 解码 ${store.decodeMs}ms${
      store.hasSidecar ? " · 已载入上次的调整方案" : ""
    }`,
  );
}

/* ---------------- 配方持久化 ---------------- */

async function saveRecipe(): Promise<void> {
  if (!store.hasImage) return;
  try {
    const p = await store.persist();
    notify.success("调整方案已保存", p);
  } catch (e) {
    notify.error("保存失败", String((e as Error)?.message ?? e));
  }
}

async function resetAll(): Promise<void> {
  if (!store.hasImage) return;
  const ok = await notify.confirm("重置全部调整？", "当前所有滑块、曲线与裁剪都会回到原图，可撤销。");
  if (!ok) return;
  await store.resetToOriginal();
  notify.info("已重置为原图", "如需一并删除已保存的方案，请在导出时选择「原始」或重新打开照片。");
}

/* ---------------- 导入 / 导出 ---------------- */

async function exportRecipe(): Promise<void> {
  if (!store.hasImage) return;
  try {
    const text = await store.exportText();
    const defaultName = `${baseName()}.pmrecipe.json`;
    const target = await saveFileDialog({
      title: "导出调整方案",
      defaultPath: defaultName,
      filters: [{ name: "调整方案", extensions: ["json"] }],
    });
    if (typeof target !== "string") {
      // 用户取消：仍然把 JSON 复制到剪贴板，便于跨设备粘贴
      await copyText(text);
      notify.info("方案已复制到剪贴板", "可直接粘贴到别的设备导入");
      return;
    }
    await writeTextFile(target, text);
    notify.success("方案已导出", target);
  } catch (e) {
    notify.error("导出失败", String((e as Error)?.message ?? e));
  }
}

async function importRecipe(): Promise<void> {
  if (!store.hasImage) return;
  const picked = await openFileDialog({
    multiple: false,
    directory: false,
    title: "导入调整方案",
    filters: [{ name: "调整方案", extensions: ["json"] }],
  });
  if (typeof picked === "string") {
    try {
      const text = await readTextFile(picked);
      await store.importText(text);
      notify.success("方案已导入", picked);
      return;
    } catch (e) {
      notify.error("导入失败", String((e as Error)?.message ?? e));
      return;
    }
  }
  // 未选文件：走剪贴板
  const text = await readClipboard();
  if (!text) {
    notify.info("未选择文件", "剪贴板里也没有方案文本，已取消");
    return;
  }
  try {
    await store.importText(text);
    notify.success("方案已从剪贴板导入");
  } catch (e) {
    notify.error("导入失败", String((e as Error)?.message ?? e));
  }
}

/* ---------------- 导出图片 ---------------- */

const exportFormat = ref<"jpeg" | "png">("jpeg");
const exportQuality = ref(92);
const exportMaxEdge = ref(0);

async function exportImage(): Promise<void> {
  if (!store.hasImage) return;
  const stem = baseName();
  const ext = exportFormat.value === "png" ? "png" : "jpg";
  const target = await saveFileDialog({
    title: "导出照片",
    defaultPath: `${stem}_edited.${ext}`,
    filters:
      exportFormat.value === "png"
        ? [{ name: "PNG（无损）", extensions: ["png"] }]
        : [{ name: "JPEG", extensions: ["jpg", "jpeg"] }],
  });
  if (typeof target !== "string") return;

  try {
    const [w, h, blob] = await store.queryExportSize(
      exportFormat.value,
      exportQuality.value,
      exportMaxEdge.value,
    );

    // 导出必须重跑算子链，不能直接存预览或上次的链产物：
    // 预览是草稿/降采样档、尺寸也小得多。这里拿全分辨率底图重跑，
    // 保证「所见（链的语义）= 所得（链的重跑）」。
    const steps = activeSteps();
    let out: Blob = blob;
    if (steps.length) {
      opsRunning.value = true;
      try {
        const mask = steps.some((s) => s.masked) ? await opsMask() : null;
        out = blob;
        for (const s of steps) {
          out = await applyStudioOp(s.op, s.params, out, s.masked ? mask : null);
        }
      } finally {
        opsRunning.value = false;
      }
    }

    const buf = await out.arrayBuffer();
    await invoke("edit_write_file", { path: target, data: Array.from(new Uint8Array(buf)) });
    notify.success(
      "导出完成",
      `${w}×${h} · ${(out.size / 1024).toFixed(0)} KB${steps.length ? ` · 含 ${steps.length} 步算子` : ""}`,
    );
  } catch (e) {
    notify.error("导出失败", String((e as Error)?.message ?? e));
  }
}

function baseName(): string {
  const p = store.sourcePath.replace(/\\/g, "/");
  const n = p.slice(p.lastIndexOf("/") + 1);
  const dot = n.lastIndexOf(".");
  return dot > 0 ? n.slice(0, dot) : n;
}

/* ---------------- 自动优化 ---------------- */

async function doAutotune(): Promise<void> {
  if (!store.hasImage) return;
  await store.autotune();
  notify.info("已应用自动优化", "只调整了曝光与极端截断，其余保持中性，可随时微调或撤销。");
}

/* ---------------- 对比 ---------------- */

/**
 * 左「调整前」右「调整后」并排（Lightroom 的 Y 视图）。
 *
 * 左格要的是「零调整」那一帧，渲染它得 1s 上下，所以先切模式、让用户
 * 立刻看到右格，零调整帧回来后再补上 —— 宁可左格晚 1s 出现，
 * 也不要点了按钮整个界面卡住。
 */
function toggleSide(): void {
  if (store.compareMode === "side") {
    store.compareMode = "off";
    return;
  }
  store.compareMode = "side";
  store.cropping = false;
  store.masking = false;
  void store.ensureOriginal().then(() => layout());
}

/** 整幅看调整前 / 回结果。 */
function toggleCompare(): void {
  const to = store.compareMode === "original" ? "off" : "original";
  store.compareMode = to;
  if (to === "original") void store.ensureOriginal().then(() => layout());
}

/** 分屏：需要拖分割线的持续交互，故与 Y 分开。 */
function toggleSplit(): void {
  const to = store.compareMode === "split" ? "off" : "split";
  store.compareMode = to;
  // 右半要露的是「调整前」，那帧得单独渲染
  if (to === "split") void store.ensureOriginal();
}

/* ---------------- 快捷键 ---------------- */

function onKeydown(e: KeyboardEvent): void {
  const t = e.target as HTMLElement | null;
  // 输入控件聚焦时不劫持（滑块用键盘调节，曲线画布可聚焦）
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;

  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    if (e.shiftKey) void store.redo();
    else void store.undo();
    return;
  }
  if (mod && e.key.toLowerCase() === "y") {
    e.preventDefault();
    void store.redo();
    return;
  }
  if (e.key === "y" || e.key === "Y") {
    if (store.compareMode !== "split") {
      e.preventDefault();
      toggleSide();
    }
    return;
  }
  if (e.key === "\\") {
    e.preventDefault();
    toggleSplit();
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));

/* ---------------- 状态栏 ---------------- */

const statusLine = computed(() => {
  if (!store.hasImage) return "选择一张照片开始调色";
  const q = store.quality === "draft" ? "草稿档" : store.subsampled ? "预览(降采样)" : "预览";
  const dirty = store.pristine ? "未调整" : "已调整（可保存方案）";
  return `${store.originalWidth}×${store.originalHeight} · ${q} ${store.previewSize.w}×${store.previewSize.h} · 渲染 ${store.renderMs}ms · ${dirty}`;
});

/* ---------------- 导入导出小工具 ---------------- */

/** Tauri 没有内置剪贴板读 API；用隐藏 textarea + execCommand 兜底。 */
async function readClipboard(): Promise<string> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    notify.info("无法读取剪贴板", "请改用「导入方案 → 选择文件」");
    return "";
  }
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      document.execCommand("copy");
    } finally {
      document.body.removeChild(ta);
    }
  }
}

async function readTextFile(path: string): Promise<string> {
  // 复用 python-studio 的读文件能力？没有对应命令 → 用 fs 插件不可用时
  // 退回到让用户粘贴。方案文件都是 JSON，粘贴完全够用。
  void path;
  throw new Error("该版本暂不支持从文件读取方案，请用剪贴板粘贴（Ctrl+V 后点「导入方案」）");
}

async function writeTextFile(path: string, text: string): Promise<void> {
  const bytes = new TextEncoder().encode(text);
  await invoke("edit_write_file", { path, data: Array.from(bytes) });
}

/* ---------------- 暴露给模板 ---------------- */

const panelCollapsed = computed({
  get: () => collapsed.value,
  set: (v: Record<string, boolean>) => (collapsed.value = v),
});

void CropOverlay;
void cloneRecipe;
void panelCollapsed;
export type { AdjustmentRecipe };
</script>

<template>
  <div class="tone-editor">
    <!-- 工具条 -->
    <header class="te-toolbar">
      <button class="te-btn primary" type="button" :disabled="!store.hasImage" @click="chooseImage">
        📂 打开照片
      </button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="saveRecipe">
        💾 保存方案
      </button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="importRecipe">📥 导入方案</button>
      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="exportRecipe">📤 导出方案</button>

      <span class="te-div"></span>

      <button class="te-btn" type="button" :disabled="!store.canUndo" title="Ctrl+Z" @click="store.undo()">
        ↶ 撤销
      </button>
      <button class="te-btn" type="button" :disabled="!store.canRedo" title="Ctrl+Shift+Z" @click="store.redo()">
        ↷ 重做
      </button>

      <span class="te-div"></span>

      <button class="te-btn" type="button" :disabled="!store.hasImage" @click="doAutotune">✨ 自动优化</button>

      <!-- 点这里开「新建蒙版」弹层（Lightroom 的路径：点局部 → 选新建方式 → 绘制） -->
      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.masking }"
        title="局部调整：框选或涂抹一片区域，调色只作用于选区内"
        @click="toggleMask()"
      >
        🎯 局部
      </button>
      <button class="te-btn" type="button" :disabled="store.pristine" @click="resetAll">↺ 重置</button>

      <span class="te-spacer"></span>

      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.compareMode === 'side' }"
        title="左右并排对比（快捷键 Y）"
        @click="toggleSide()"
      >
        ⬒ 左右对比
      </button>
      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.compareMode === 'original' }"
        title="整幅看调整前的画面"
        @click="toggleCompare"
      >
        {{ store.compareMode === "original" ? "看结果" : "看原图" }}
      </button>
      <button
        class="te-btn"
        type="button"
        :disabled="!store.hasImage"
        :class="{ active: store.compareMode === 'split' }"
        title="拖动分割线对比（快捷键 \\）"
        @click="toggleSplit()"
      >
        分屏对比
      </button>

      <span class="te-div"></span>

      <select v-model="exportFormat" class="te-select" :disabled="!store.hasImage" title="导出格式">
        <option value="jpeg">JPEG</option>
        <option value="png">PNG</option>
      </select>
      <template v-if="exportFormat === 'jpeg'">
        <input
          v-model.number="exportQuality"
          class="te-num"
          type="number"
          min="50"
          max="100"
          step="1"
          title="JPEG 质量"
          :disabled="!store.hasImage"
        />
      </template>
      <select v-model.number="exportMaxEdge" class="te-select" :disabled="!store.hasImage" title="导出长边">
        <option :value="0">原始尺寸</option>
        <option :value="4096">长边 4096</option>
        <option :value="2560">长边 2560</option>
        <option :value="1600">长边 1600</option>
      </select>
      <button class="te-btn accent" type="button" :disabled="!store.hasImage" @click="exportImage">
        导出照片
      </button>
    </header>

    <!-- 三栏主体 -->
    <div class="te-body">
      <!-- 左：直方图 -->
      <aside class="te-left">
        <HistogramPanel
          :histogram="store.histogram"
          :width="store.previewSize.w"
          :height="store.previewSize.h"
        />
        <div class="te-hint">
          <p>拖动滑块实时预览，松开后自动出高清图。</p>
          <p>双击滑块归零 · Shift 精细调节 · 滚轮也可调。</p>
          <p><kbd>Y</kbd> 对比原图 · <kbd>\</kbd> 分屏 · <kbd>Ctrl+Z</kbd> 撤销。</p>
        </div>
      </aside>

      <!-- 中：画布 -->
      <section ref="stageEl" class="te-stage" :class="{ cropping: store.cropping }">
        <div v-if="!store.hasImage" class="te-empty">
          <p class="te-empty-title">还没有打开照片</p>
          <p class="te-empty-sub">点左上「打开照片」，或从灯箱的「✏️ 编辑」直接进来。</p>
          <button class="te-btn primary" type="button" @click="chooseImage">📂 选择照片</button>
        </div>

        <template v-else>
          <!-- 左「调整前」右「调整后」并排。两格沿用 `.te-image-holder`
               （它是 `position:absolute`，算子结果层靠它做叠放基准）。 -->
          <template v-if="sideMode">
            <div
              class="te-image-holder"
              :style="{
                left: `${imageFrame.x}px`,
                top: `${imageFrame.y}px`,
                width: `${imageFrame.w}px`,
                height: `${imageFrame.h}px`,
              }"
            >
              <img
                v-if="store.originalUrl"
                :src="store.originalUrl"
                alt="调整前"
                class="te-image"
                draggable="false"
              />
              <span v-else-if="store.originalBusy" class="te-pane-wait">正在渲染调整前画面…</span>
              <span class="te-pane-tag">调整前</span>
            </div>

            <div
              class="te-image-holder"
              :style="{
                left: `${sideRightFrame.x}px`,
                top: `${sideRightFrame.y}px`,
                width: `${sideRightFrame.w}px`,
                height: `${sideRightFrame.h}px`,
              }"
            >
              <img :src="store.previewUrl" alt="调整后" class="te-image" draggable="false" />
              <img
                v-if="opsResultUrl"
                :src="opsResultUrl"
                alt="算子链结果"
                class="te-image te-ops-layer"
                draggable="false"
              />
              <span class="te-pane-tag right">结果</span>
            </div>
          </template>

          <div
            v-else
            class="te-image-holder"
            :style="{
              left: `${imageFrame.x}px`,
              top: `${imageFrame.y}px`,
              width: `${imageFrame.w}px`,
              height: `${imageFrame.h}px`,
            }"
          >
            <img
              v-show="store.compareMode !== 'original'"
              :src="store.previewUrl"
              alt="调整后预览"
              class="te-image"
              draggable="false"
            />

            <!-- 算子链结果层：盖在 Rust 底图之上（靠 CSS 绝对定位叠住，见
                 `.te-ops-layer`）。它是 `recipe.ops` 的**派生产物**，不参与
                 撤销栈 —— 撤销回退配方后链会重跑，这里自然跟着更新。 -->
            <img
              v-if="opsResultUrl && store.compareMode === 'off'"
              :src="opsResultUrl"
              alt="算子链结果"
              class="te-image te-ops-layer"
              draggable="false"
            />

            <!-- 整幅看「调整前」。这帧要单独渲染（≈1s），故懒加载，
                 切模式时立刻切、帧回来再补，避免界面卡住。 -->
            <img
              v-if="store.compareMode === 'original' && store.originalUrl"
              :src="store.originalUrl"
              alt="调整前"
              class="te-image"
              draggable="false"
            />
            <span
              v-else-if="store.compareMode === 'original' && store.originalBusy"
              class="te-pane-wait"
            >
              正在渲染调整前画面…
            </span>

            <!-- 分屏：右半露出「调整前」（clipPath 裁掉左半） -->
            <img
              v-if="store.compareMode === 'split' && store.originalUrl"
              :src="store.originalUrl"
              alt="调整前"
              class="te-image te-image-split"
              :style="{ clipPath: `inset(0 0 0 ${store.splitX * 100}%)` }"
              draggable="false"
            />

            <!-- 分屏分割线 -->
            <div
              v-if="store.compareMode === 'split'"
              class="te-split"
              :style="{ left: `${store.splitX * 100}%` }"
              @pointerdown="
                (e: PointerEvent) => {
                  const holder = (e.currentTarget as HTMLElement).parentElement as HTMLElement;
                  holder.setPointerCapture(e.pointerId);
                  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                }
              "
              @pointermove="
                (e: PointerEvent) => {
                  const holder = (e.currentTarget as HTMLElement).parentElement as HTMLElement;
                  if (!holder?.hasPointerCapture(e.pointerId)) return;
                  const r = holder.getBoundingClientRect();
                  store.splitX = Math.min(0.98, Math.max(0.02, (e.clientX - r.left) / r.width));
                }
              "
            >
              <span class="te-split-grip">◀▶</span>
            </div>

            <!-- 局部蒙版层（与裁剪互斥：两者都要独占画布手势） -->
            <MaskOverlay
              v-if="store.masking"
              ref="maskOverlay"
              :mask-width="store.previewSize.w"
              :mask-height="store.previewSize.h"
              :scale="maskScale"
              :enabled="store.hasImage"
              :tool="maskTool"
              :brush-size="maskBrush"
              @change="onMaskChange"
            />

            <!-- 裁剪层 -->
            <CropOverlay
              v-if="store.cropping && !store.masking"
              :crop="store.recipe.crop"
              :frame="imageFrame"
              :src-width="store.originalWidth"
              :src-height="store.originalHeight"
              @begin="store.beginInteraction()"
              @update:crop="(c) => { const n = cloneRecipe(store.recipe); n.crop = c; void store.applyRecipe(n, false); }"
              @commit:crop="
                (c) => {
                  const n = cloneRecipe(store.recipe);
                  n.crop = c;
                  void store.applyRecipe(n, true);
                }
              "
            />
          </div>
        </template>

        <!-- 渲染错误：必须显式展示，不能静默 -->
        <div v-if="store.renderError" class="te-error">
          <strong>渲染失败</strong>
          <span>{{ store.renderError }}</span>
          <button class="te-btn" type="button" @click="store.renderNow?.('preview')">重试</button>
        </div>
      </section>

      <!-- 右：调整面板 -->
      <aside class="te-right">
        <!-- 局部调整（Lightroom 的交互路径：点按钮 → 弹层选新建方式 → 回画布绘制） -->
        <section class="te-mask-params">
          <div class="te-mask-head">
            <span>局部调整</span>
            <button
              class="te-mask-new"
              type="button"
              :disabled="!store.hasImage"
              :class="{ active: maskPickerOpen }"
              @click="toggleMask()"
            >
              {{ store.hasMask ? "编辑选区" : "＋ 新建蒙版" }}
            </button>
          </div>

          <!-- 新建方式弹层 -->
          <!-- 换图后 store.masking 会被重置，但弹层是本页局部状态，一并收掉 -->
          <div v-if="maskPickerOpen && store.hasImage" class="te-mask-picker">
            <p class="te-mask-picker-title">
              {{ store.hasMask ? "选择编辑工具" : "选择新建方式" }}
            </p>
            <button
              v-for="t in (['rect', 'brush', 'eraser'] as MaskTool[])"
              :key="t"
              class="te-mask-opt"
              type="button"
              :class="{ active: maskTool === t }"
              @click="pickMaskTool(t)"
            >
              <b>{{ MASK_TOOL_LABEL[t] }}</b>
              <span>{{ MASK_TOOL_HINT[t] }}</span>
            </button>

            <label v-if="maskTool !== 'rect'" class="te-mask-brush">
              <span>笔刷大小</span>
              <input v-model.number="maskBrush" type="range" min="8" max="240" step="4" />
              <b>{{ maskBrush }}px</b>
            </label>

            <p class="te-mask-note">
              局部调整让上方的调色滑块<b>只作用于选区内</b>，选区外完全保持原样。
            </p>
          </div>

          <p v-else-if="store.hasMask" class="te-mask-tip">
            {{ MASK_TOOL_LABEL[maskTool] }} · 选区外的画面不受调色影响
          </p>

          <!-- 蒙版参数：只在真的画了蒙版后出现 -->
          <template v-if="store.hasMask">
            <label class="te-mask-row">
              <span>羽化</span>
              <input
                :value="store.recipe.mask.featherPx"
                type="range"
                min="0"
                max="120"
                step="1"
                @pointerdown="store.beginInteraction()"
                @input="
                  (e) =>
                    void store.applyMaskParams(
                      { featherPx: Number((e.target as HTMLInputElement).value) },
                      false,
                    )
                "
                @change="
                  (e) =>
                    void store.endInteraction({
                      ...store.recipe,
                      mask: { ...store.recipe.mask, featherPx: Number((e.target as HTMLInputElement).value) },
                    })
                "
              />
              <b>{{ Math.round(store.recipe.mask.featherPx) }}px</b>
            </label>

            <label class="te-mask-row">
              <span>强度</span>
              <input
                :value="store.recipe.mask.strength"
                type="range"
                min="0"
                max="1"
                step="0.01"
                @pointerdown="store.beginInteraction()"
                @input="
                  (e) =>
                    void store.applyMaskParams(
                      { strength: Number((e.target as HTMLInputElement).value) },
                      false,
                    )
                "
                @change="
                  (e) =>
                    void store.endInteraction({
                      ...store.recipe,
                      mask: { ...store.recipe.mask, strength: Number((e.target as HTMLInputElement).value) },
                    })
                "
              />
              <b>{{ Math.round(store.recipe.mask.strength * 100) }}%</b>
            </label>

            <label class="te-mask-check">
              <input
                type="checkbox"
                :checked="store.recipe.mask.invert"
                @change="
                  (e) =>
                    void store.applyMaskParams(
                      { invert: (e.target as HTMLInputElement).checked },
                      true,
                    )
                "
              />
              <span>反相（选中区外生效）</span>
            </label>

            <label class="te-mask-check">
              <input
                type="checkbox"
                :checked="store.recipe.mask.enabled"
                @change="
                  (e) =>
                    void store.applyMaskParams(
                      { enabled: (e.target as HTMLInputElement).checked },
                      true,
                    )
                "
              />
              <span>启用局部调整</span>
            </label>

            <div class="te-mask-actions">
              <span v-if="store.maskInfo" class="te-mask-cov">
                选中 {{ Math.round(store.maskInfo.coverage * 100) }}%
              </span>
              <button class="te-mask-new danger" type="button" @click="store.clearMask()">清除蒙版</button>
            </div>

            <p v-if="store.maskError" class="te-mask-err">{{ store.maskError }}</p>
          </template>
        </section>

        <!-- 组件顺序 = 处理顺序：先调色（出底图），再图像处理（叠加在底图上）。
             两者并列而非二选一：只用调色、只用算子、或两者叠加都成立。 -->
        <EditorPanel v-model:collapsed="panelCollapsed" />

        <StudioOpsPanel
          :ops="studioOps"
          :loading="opsLoading"
          :error="opsError"
          :get-input="opsInput"
          :get-mask="opsMask"
          :apply="applyStudioOp"
          :mask-active="store.hasMask"
          :result-blob="opsResultBlob"
          :running="opsRunning"
          :run-error="opsRunError"
          @changed="onOpsChanged"
        />
      </aside>
    </div>

    <!-- 状态栏 -->
    <footer class="te-status">
      <span>{{ statusLine }}</span>
      <span v-if="store.rendering" class="te-spin">渲染中…</span>
      <span v-if="store.hasMask" class="te-tag">局部调整</span>
      <span v-if="opsStepCount > 0" class="te-tag ops">
        {{ opsStepCount }} 步算子{{ opsRunning ? "（处理中）" : "" }}
      </span>
      <span v-if="store.hasSidecar" class="te-tag">已保存方案</span>
    </footer>
  </div>
</template>

<style scoped>
.tone-editor {
  display: flex;
  flex-direction: column;
  height: calc(100vh - 160px);
  min-height: 520px;
  border-radius: var(--radius-lg, 16px);
  overflow: hidden;
}

/* ---------------- 工具条 ---------------- */

.te-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  padding: 8px 12px;
  border-bottom: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.2));
}

.te-btn {
  padding: 4px 10px;
  font-size: 12px;
  color: var(--color-text, #e6e9f0);
  background: var(--pm-btn-bg, rgba(128, 128, 138, 0.14));
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 7px;
  cursor: pointer;
  white-space: nowrap;
}
.te-btn:hover:not(:disabled) {
  background: var(--pm-btn-hover, rgba(128, 128, 138, 0.26));
}
.te-btn:disabled {
  opacity: 0.42;
  cursor: not-allowed;
}
.te-btn.primary {
  background: var(--color-primary, rgba(140, 175, 235, 0.9));
  color: #171b22;
  border-color: transparent;
  font-weight: 600;
}
.te-btn.accent {
  background: rgba(255, 214, 120, 0.9);
  color: #20242c;
  border-color: transparent;
  font-weight: 600;
}
.te-btn.active {
  background: rgba(255, 214, 120, 0.85);
  color: #20242c;
  border-color: transparent;
}

.te-div {
  width: 1px;
  height: 18px;
  background: var(--pm-soft-border, rgba(128, 128, 138, 0.22));
}
.te-spacer {
  flex: 1;
}
.te-select,
.te-num {
  padding: 3px 6px;
  font-size: 11px;
  color: var(--color-text, #e6e9f0);
  background: var(--pm-input-bg, rgba(128, 128, 138, 0.14));
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.22));
  border-radius: 6px;
}
.te-num {
  width: 56px;
}

/* ---------------- 三栏 ---------------- */

.te-body {
  flex: 1;
  display: grid;
  grid-template-columns: 300px 1fr 340px;
  min-height: 0;
}

.te-left {
  padding: 12px 10px;
  border-right: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.18));
  overflow-y: auto;
}
.te-hint {
  margin-top: 12px;
  font-size: 11px;
  line-height: 1.7;
  color: var(--pm-hint, var(--color-text-2));
}
.te-hint p {
  margin: 0 0 4px;
}
.te-hint kbd {
  padding: 0 4px;
  font-size: 10px;
  border: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.3));
  border-radius: 3px;
  background: rgba(128, 128, 138, 0.12);
}

.te-stage {
  position: relative;
  overflow: hidden;
  background: rgba(12, 14, 18, 0.35);
  cursor: default;
}
.te-stage.cropping {
  cursor: crosshair;
}

.te-image-holder {
  position: absolute;
}

.te-image {
  display: block;
  width: 100%;
  height: 100%;
  object-fit: contain;
  user-select: none;
  -webkit-user-drag: none;
}

.te-image-split {
  position: absolute;
  inset: 0;
  /* 左侧用「结果」、右侧用「调整前」：clipPath 把左半裁掉，
     于是右半露出的是下面那张「零调整」渲染结果 */
  filter: none;
}

/* 并排对比时每格左上/右上角的角标 */
.te-pane-tag {
  position: absolute;
  top: 8px;
  left: 8px;
  padding: 2px 8px;
  font-size: 11px;
  line-height: 1.6;
  color: #f2f2f5;
  background: rgba(20, 20, 26, 0.62);
  border-radius: 10px;
  pointer-events: none;
  user-select: none;
}

.te-pane-tag.right {
  left: auto;
  right: 8px;
}

/* 「调整前」那帧要单独渲染 ≈1s，等它时占位而不是留白 */
.te-pane-wait {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
  color: var(--color-text-2);
  background: rgba(120, 120, 130, 0.1);
  border: 1px dashed rgba(128, 128, 138, 0.3);
  border-radius: 6px;
}

.te-split {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 2px;
  margin-left: -1px;
  background: rgba(255, 255, 255, 0.9);
  cursor: ew-resize;
  touch-action: none;
}
.te-split-grip {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  padding: 2px 6px;
  font-size: 10px;
  color: #20242c;
  background: rgba(255, 255, 255, 0.92);
  border-radius: 8px;
}

.te-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  text-align: center;
}
.te-empty-title {
  margin: 0;
  font-size: 15px;
  color: var(--color-text, #e6e9f0);
}
.te-empty-sub {
  margin: 0 0 8px;
  font-size: 12px;
  color: var(--pm-hint, var(--color-text-2));
}

.te-error {
  position: absolute;
  left: 50%;
  bottom: 16px;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  font-size: 12px;
  color: #ffd7d7;
  background: rgba(120, 30, 34, 0.9);
  border-radius: 8px;
}
.te-error strong {
  white-space: nowrap;
}
.te-error span {
  max-width: 380px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.te-right {
  border-left: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.18));
  overflow-y: auto;
}

/* 算子结果层：与预览同尺寸同位置，靠 DOM 顺序压在上方 */
/* 算子结果层：必须绝对定位覆盖在预览之上。
   `.te-image` 只有 `display:block; width/height:100%`，没有定位 ——
   两张这样的 img 会在 holder 里纵向排开并溢出，看起来就是「同一张图
   出现了两次、下面那半被裁掉」。`position:absolute; inset:0` 才能叠住。 */
.te-ops-layer {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

/* ---------------- 局部蒙版参数 ---------------- */

.te-mask-params {
  padding: 12px 14px;
  border-bottom: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.18));
  background: rgba(255, 107, 96, 0.06);
}

.te-mask-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 10px;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-text);
}

.te-mask-new {
  height: 24px;
  padding: 0 10px;
  font-size: 11px;
  color: var(--color-text);
  background: rgba(120, 120, 130, 0.16);
  border: 1px solid rgba(128, 128, 138, 0.24);
  border-radius: 12px;
  cursor: pointer;
  white-space: nowrap;
}

.te-mask-new.active {
  background: rgba(255, 107, 96, 0.26);
  border-color: rgba(255, 107, 96, 0.55);
}

.te-mask-new.danger {
  color: #d4483b;
}

.te-mask-new:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

/* 新建方式弹层：Lightroom 的「点局部 → 选新建方式」 */
.te-mask-picker {
  padding: 8px;
  margin-bottom: 10px;
  border: 1px solid rgba(128, 128, 138, 0.24);
  border-radius: 9px;
  background: rgba(120, 120, 130, 0.09);
}

.te-mask-picker-title {
  margin: 0 0 6px;
  font-size: 11px;
  color: var(--color-text-2);
}

.te-mask-opt {
  display: flex;
  flex-direction: column;
  gap: 1px;
  width: 100%;
  padding: 6px 9px;
  margin-bottom: 4px;
  text-align: left;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 7px;
  cursor: pointer;
}

.te-mask-opt:hover {
  background: rgba(120, 120, 130, 0.14);
}

.te-mask-opt.active {
  background: rgba(255, 107, 96, 0.18);
  border-color: rgba(255, 107, 96, 0.45);
}

.te-mask-opt b {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text);
}

.te-mask-opt span {
  font-size: 11px;
  color: var(--color-text-2);
}

.te-mask-brush {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 4px 2px 2px;
  font-size: 11px;
  color: var(--color-text-2);
}

.te-mask-brush > span {
  flex-shrink: 0;
}

.te-mask-brush input[type="range"] {
  flex: 1;
  min-width: 0;
}

.te-mask-brush b {
  flex-shrink: 0;
  min-width: 38px;
  text-align: right;
  font-variant-numeric: tabular-nums;
  color: var(--color-text);
}

.te-mask-note {
  margin: 6px 2px 0;
  font-size: 11px;
  line-height: 1.55;
  color: var(--color-text-2);
}

.te-mask-note b {
  color: var(--color-text);
}

.te-mask-tip {
  margin: 0 0 8px;
  font-size: 11px;
  line-height: 1.5;
  color: var(--color-text-2);
}

.te-mask-check {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 400;
  color: var(--color-text-2);
  cursor: pointer;
}

.te-mask-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  font-size: 12px;
  color: var(--color-text-2);
}

.te-mask-row > span {
  flex-shrink: 0;
  width: 34px;
}

.te-mask-row input[type="range"] {
  flex: 1;
  min-width: 0;
}

.te-mask-row b {
  flex-shrink: 0;
  min-width: 40px;
  text-align: right;
  font-variant-numeric: tabular-nums;
  color: var(--color-text);
}

.te-mask-check {
  margin: 6px 0 10px;
}

.te-mask-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.te-mask-cov {
  font-size: 11px;
  color: var(--color-text-2);
  font-variant-numeric: tabular-nums;
}

.te-mask-err {
  margin: 8px 0 0;
  font-size: 11px;
  color: #d4483b;
}

.te-btn.active {
  background: rgba(255, 107, 96, 0.24);
  border-color: rgba(255, 107, 96, 0.5);
}

/* ---------------- 状态栏 ---------------- */

.te-status {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 5px 12px;
  font-size: 11px;
  color: var(--pm-hint, var(--color-text-2));
  border-top: 1px solid var(--pm-soft-border, rgba(128, 128, 138, 0.2));
  font-variant-numeric: tabular-nums;
}
.te-spin {
  color: var(--color-primary, rgba(140, 175, 235, 0.95));
}
.te-tag {
  padding: 1px 7px;
  border-radius: 8px;
  background: rgba(120, 190, 120, 0.18);
  color: #a8dca8;
}

/* 算子结果标签：与「已保存方案」区分开，一眼看出这一帧被算子烘焙过 */
.te-tag.ops {
  background: rgba(120, 170, 255, 0.2);
  color: #a9c8ff;
}

@media (max-width: 1180px) {
  .te-body {
    grid-template-columns: 1fr 320px;
  }
  .te-left {
    display: none;
  }
}
</style>