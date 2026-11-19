<script setup lang="ts">
/**
 * AI 修图工作台（独立视图 · FEAT-160 系列 / FEAT-177 重构 / FEAT-180 布局重排 / FEAT-197）
 * ===================================================================
 * 创意工坊（`/workshop`）左栏的「工作台」到达本页（FEAT-197 之前叫「批量套用」）。
 * 工坊保持原样；本页是一个**专门的修图工作空间**：导入一批 → 逐图状态 →
 * 前后对比 → 批量出片 → 导出并自动进「创意工坊」相册。
 *
 * ## 三条出片路线（FEAT-177 / FEAT-197）
 *   ① 配方：与 ①一键出片共用 `RecipePicker` 与同一个 recipe 作业（默认，懒人主路径）
 *   ② 模型理解：语言模型规划 + 文生图模型执行（按调用计费）。
 *      **只用云端模型**：原图 → 短边 1080 → 规划 → 执行 → 1K 出图 →（可选）上采样。
 *      可调「文生图步数」与「上采样」（默认不勾、默认 2×、4× 用 2× 跑两遍）。
 *   ③ 自己组合：原子组件串成链，本机与云端都能插、同一组件可重复、上限 10 步。
 * 三者只换「提交哪个作业、带什么参数」，导入/进度回填/前后对比/导出配册完全共用。
 *
 * ## 固定工作区（FEAT-180）
 * 页面不再整页滚动：顶栏/面包屑之后，三栏吃满剩余高度。
 *   - 中栏 = 对比大图（占满剩余高度）+ 底部 LR 式胶片条；要一次看完就切「网格」；
 *   - 左右栏可折叠成 40px 竖排标签，折叠后中栏吃掉整窗宽度；
 *   - 右栏卡片自己滚，最后一张卡不会再被视口裁掉。
 * 为什么不再让对比框锁 16:9/420px：竖构图照片在 420px 高的框里只剩三百多像素宽，
 * 用户看不清修图细节（FEAT-180 的直接动因）。
 *
 * ## 单步微调（FEAT-180）
 * 配方每一步的参数面板默认**存在且折叠**，抽完样片自动展开一次。
 * 参数表由 `op_catalog.json` 驱动（`workbenchPlan.paramViews`），与后端
 * `TUNABLE_PARAMS` 逐键一致；改完只把差量 `paramsOverride` 交给 `/preview`
 * 与正式作业，不再手工维护第二份白名单。
 *
 * ## 为什么本页没有 `.panel` 类名
 * 全局 `src/assets/main.css` 的 `body.theme-dark .panel` 特异性 (0,2,1) 会压过本页
 * scoped 样式，把右栏卡片刷成藏青玻璃（页面底色是松夜绿）。本页统一用 `.wb-card`，
 * 容器面走一个 `--wb-surface`，见下方样式注释。
 *
 * 编排都在这里（CS1 入口层）：作业提交、进度回填、相册入库、费用预估；
 * 纯计算在 `utils/workbenchPlan.ts`、抽样在 `utils/samplePick.ts`、预览请求在
 * `composables/useWorkbenchPreview.ts`，对比框/胶片条/网格/微调各自一个组件。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";

import AppHeader from "../components/ui/AppHeader.vue";
import LocalModelHint from "../components/workshop/ai/LocalModelHint.vue";
import RecipePicker from "../components/workshop/ai/RecipePicker.vue";
import JobStepsPanel from "../components/workshop/ai/JobStepsPanel.vue";
import WorkbenchCompare from "../components/workshop/ai/WorkbenchCompare.vue";
import WorkbenchSteps from "../components/workshop/ai/WorkbenchSteps.vue";
import WorkbenchTune from "../components/workshop/ai/WorkbenchTune.vue";
import WorkbenchFilmstrip from "../components/workshop/ai/WorkbenchFilmstrip.vue";
import WorkbenchGrid from "../components/workshop/ai/WorkbenchGrid.vue";
import { pickPhotos, pickDirectory } from "../composables/useOpenDialog";
import { opById, toolLabel } from "../utils/toolCatalog";import { useWorkbenchStore, type WbEngine } from "../stores/workbench";
import { useWorkshopUiStore } from "../stores/workshopUi";
import { useAiStudioStore } from "../stores/aiStudio";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import { useAiBilling } from "../composables/useAiBilling";
import { useWorkbenchPreview } from "../composables/useWorkbenchPreview";
import { usePhotoSrcMap } from "../composables/usePhotoSrcMap";
import { useWorkbenchThumbs } from "../composables/useWorkbenchThumbs";
import { baseName } from "../utils/pathParts";
import { dedupePhotoPaths, splitPhotoPaths } from "../utils/workshopHandoff";
import { qcHeadline, qcTone, summarizeQc, verdictLabel } from "../utils/workbenchQc";
import { pickSample } from "../utils/samplePick";
import {
  MODEL_STYLES,
  modelStyleLabel,
  switchStyleText,
  toggleStyle,
} from "../utils/modelStylePresets";
import type { RunPlan } from "../types/ai";
import type { WbThumbItem } from "../types/workbench";
import {
  chainHasCloud,
  chainPlan,
  chainSummary,
  isCloudOp,
  opLabel,
  planRunnable,
  recipeById,
  RECIPES,
  chainFromRecipe,
} from "../utils/workbenchPlan";
import {
  originOfChain,
  originOfRecipe,
  originOfTool,
  type OriginInfo,
} from "../utils/modelOrigin";

const route = useRoute();
const router = useRouter();
const wb = useWorkbenchStore();
const ui = useWorkshopUiStore();
const ai = useAiStudioStore();
const albumStore = useAlbumStore();
const contentStore = useContentStore();
const notify = useNotify();
const billing = useAiBilling();
const preview = useWorkbenchPreview();

/* ---------------- 缩略图 ---------------- */
// 两套地址表，用途完全不同：
//   - srcMap      → **原图**：对比框「原图」层要看清细节，只有它该用原图；
//   - thumbSrcMap → **≤384px 缩略图**：胶片条（96×64）与网格（148px）用。
// 过去三者共用 srcMap，等于让 96px 的格子去解码 24MP 原图（一屏 10~18 次）。
const { srcMap, ensureSrc, prune } = usePhotoSrcMap();
const { srcMap: thumbSrcMap, ensure: ensureThumb, prune: pruneThumbs } = useWorkbenchThumbs();
const neededPaths = computed(() => {
  const s = new Set<string>();
  for (const it of wb.items) {
    s.add(it.path);
    if (it.outputPath) s.add(it.outputPath);
  }
  return [...s];
});
/** 只有原图路径需要大图地址（产物路径只喂对比框用它自己的解析） */
const neededPhotoPaths = computed(() => wb.items.map((it) => it.path));
watch(
  neededPaths,
  (list) => {
    // 先剪掉已移出工作集的地址，再补齐新增的；条目数不会随「导入过多少张」单调增长
    prune(list);
    list.forEach((p) => void ensureSrc(p));
  },
  { immediate: true },
);
watch(
  neededPhotoPaths,
  (list) => {
    pruneThumbs(list);
    list.forEach((p) => ensureThumb(p));
  },
  { immediate: true },
);

/** 胶片条 / 网格共用的行数据（状态机仍在 store 里） */
const thumbs = computed<WbThumbItem[]>(() =>
  wb.items.map((it) => ({
    path: it.path,
    name: it.name,
    // 胶片条/网格用缩略图（≤384px），不是原图 —— 见 useWorkbenchThumbs 头注
    src: thumbSrcMap.value[it.path] ?? "",
    status: it.status,
    progress: it.progress,
    selected: wb.selected.includes(it.path),
    sample: wb.samplePaths.includes(it.path),
  })),
);

/* ---------------- 当前对比项 ---------------- */
const currentPath = ref<string>("");
/**
 * 只依赖「路径序列」：当前图被移出时改选第一张。
 * 不用 `deep:true` —— 进度回填每 700ms 改一次 items 内层字段，deep 会跟着整表遍历；
 * 也不能只看 `items.length` —— 先删后加同一批（长度不变）会漏掉。
 */
const itemPaths = computed(() => wb.items.map((i) => i.path));
watch(
  itemPaths,
  (list) => {
    if (!list.includes(currentPath.value)) {
      currentPath.value = list[0] ?? "";
    }
  },
  { immediate: true },
);
const current = computed(() => wb.items.find((i) => i.path === currentPath.value) ?? null);
const beforeSrc = computed(() => (current.value ? srcMap.value[current.value.path] ?? "" : ""));

/** 对比框按当前照片真实宽高比排版（3:2 / 4:5 都不裁切） */
const aspect = ref(16 / 9);
let aspectSeq = 0;
watch(
  beforeSrc,
  (src) => {
    if (!src) return;
    // 只认最后一次：快速切图时旧的 onload 后到会覆盖新图的宽高比
    const mine = ++aspectSeq;
    const img = new Image();
    img.onload = () => {
      if (mine !== aspectSeq) return;
      if (img.naturalWidth && img.naturalHeight) aspect.value = img.naturalWidth / img.naturalHeight;
    };
    img.src = src;
  },
  { immediate: true },
);

/* ---------------- 出片质检（FEAT-173） ---------------- */
const job = computed(() => ai.jobOfTool(activeTool.value));
const qc = computed(() => summarizeQc(job.value?.meta?.verifications));
const qcText = computed(() => qcHeadline(qc.value));
const qcClass = computed(() => qcTone(qc.value));
const currentVerdict = computed(() => {
  const list = job.value?.meta?.verifications ?? [];
  const i = wb.items.findIndex((it) => it.path === currentPath.value);
  return i >= 0 ? list[i] : undefined;
});

/* ---------------- 即时预览（本机算子才开） ---------------- */
const previewOn = ref(true);

/**
 * 本次出片的参数载体（预览 / 样片 / 全量共用一处，避免三处各拼一份）：
 *   - 自己组合 → 显式 plan；
 *   - 配方且存在「不处理」→ 显式 plan（recipeId + 覆盖参数无法跳过步骤）；
 *   - 配方且没有不处理 → recipeId + 差量覆盖（保持原路径与后端白名单语义）。
 *
 * `steps` 是文生图采样步数：只有自己组合里出现云端步时才随配方通道下发
 * （纯本机链条无云端调用，传了也没人读）。
 */
type EditOptions =
  | { plan: RunPlan; steps?: number }
  | { recipeId: string; plan: RunPlan }
  | {
      recipeId: string;
      paramsOverride: Record<string, Record<string, number | string | boolean>>;
      steps?: number;
    };

const editOptions = computed<EditOptions>(() => {
  if (wb.engine === "chain") {
    // 纯本机链条不传步数：免得「选了步数」给人一种云端真的跑了的错觉
    return chainHasCloud(wb.chain)
      ? { plan: chainPlan(wb.chain), steps: wb.renderSteps }
      : { plan: chainPlan(wb.chain) };
  }
  if (wb.tuneUsesPlan) return { recipeId: wb.recipeId, plan: wb.tunePlan };
  return { recipeId: wb.recipeId, paramsOverride: wb.tuneOverrides, steps: wb.renderSteps };
});

const previewReq = computed(() => {
  const p = current.value?.path;
  if (!p || wb.engine === "ai" || !previewOn.value) return null;
  const opts = editOptions.value;
  // 所有步都不处理 → 没有可执行计划，交给空态而不是让后端报「计划为空」
  if ("plan" in opts && !opts.plan.steps.length) return null;
  // 预览只跑本机算子，`steps`（文生图步数）在预览链路里没有消费者，不往请求里塞
  if ("plan" in opts) return { path: p, plan: opts.plan };
  return { path: p, recipeId: opts.recipeId, paramsOverride: opts.paramsOverride };
});

/** 产物图（批量跑出来的） */
const productSrc = computed(() =>
  current.value?.outputPath ? srcMap.value[current.value.outputPath] ?? "" : "",
);
/** 效果图：优先即时预览（反映当前参数），否则用批量产物 */
const afterSrc = computed(() => {
  if (previewOn.value && preview.src.value) return preview.src.value;
  return productSrc.value;
});

const slider = ref(52);
/** 对比框当前缩放倍率（1 = 适应窗口）；只用于说明文案，缩放状态在组件内 */
const viewerZoom = ref(1);

/* ---------------- 入口：带入照片 ---------------- */
/**
 * 进入工作台时的照片来源，按优先级合并（全部去重保序）：
 *   1. `?photos=a|b|c` / 旧键 `?photo=`：从相册/灯箱/多选条带进来的；
 *   2. 工坊素材托盘：**工作台模式默认载入**（FEAT-197）。只在本次工作集为空时载入，
 *      否则「离开再回来」会把已经调过参数的批次和托盘混在一起。
 */
onMounted(async () => {
  const q = route.query;
  const one = typeof q.photo === "string" ? q.photo : "";
  const incoming = dedupePhotoPaths([one, ...splitPhotoPaths(q.photos)]);
  if (incoming.length) wb.addPaths(incoming);

  if (!wb.items.length && ui.tray.length) {
    wb.addPaths(dedupePhotoPaths(ui.tray));
    notify.info("已载入工坊素材", `${ui.tray.length} 张照片进入工作集`);
  }

  // 来源标注（FEAT-199）要读已绑定的供应商地址；读失败静默（标注只是提示）
  void ai.loadSettings().catch(() => undefined);
});

async function addPhotos() {
  wb.addPaths(await pickPhotos());
}

/** 导入文件夹：借扫描命令拿到目录内全部图片路径（递归） */
async function importFolder() {
  const dir = await pickDirectory("选择要导入的文件夹");
  if (!dir) return;
  try {
    const rows = await invoke<{ path: string }[]>("scan_album_photos", { path: dir });
    wb.addPaths(rows.map((r) => r.path).filter(Boolean));
  } catch (e) {
    notify.error("导入失败", String(e));
  }
}

function loadFromWorkshop() {
  if (!ui.tray.length) {
    notify.info("工坊素材是空的", "先到创意工坊里添加照片，再回这里载入");
    return;
  }
  const before = wb.total;
  wb.addPaths(dedupePhotoPaths(ui.tray));
  const added = wb.total - before;
  notify.info(
    added ? `已载入 ${added} 张` : "没有新照片",
    added ? "已在工作集里的不会重复加入" : "工坊素材都已经在工作集里了",
  );
}

async function pickOutputDir() {
  const dir = await pickDirectory("选择输出目录");
  if (dir) wb.outputDir = dir;
}

/* ---------------- 删除 ---------------- */
function removeOne(path: string) {
  wb.removePath(path);
}
function removeSelected() {
  if (!wb.selectedCount) {
    notify.info("先勾选照片", "勾选后再点「删除所选」");
    return;
  }
  const n = wb.selectedCount;
  wb.removePaths([...wb.selected]);
  notify.info(`已移出 ${n} 张`, "只移出工作集，原图不动");
}

/* ---------------- 花费预估 ---------------- */
const visionPrice = computed(() => billing.modelPrice("vision"));
const editPrice = computed(() => billing.modelPrice("edit"));
const upscalePrice = computed(() => billing.modelPrice("upscale"));
const targetCount = computed(() => (wb.selectedCount > 0 ? wb.selectedCount : wb.total));

/**
 * 每张预计调用次数（**必须与后端真实调用一致**，否则用户在确认框里看到的是假价）：
 *   - 模型理解：规划 1 次理解；规划出的云端步各 1 次编辑（步数在跑之前未知，
 *     所以按「最少 1 次」估下限，并在文案里标明「按实际步数」）；上采样 N 遍超分；
 *     反思再 +1 次理解。
 *   - 配方 / 自己组合：按计划里云端步的数量算（本机步 0 元）。
 */
function expectedCalls(): { v: number; e: number; u: number } {
  if (wb.engine === "ai") {
    const ups = wb.upsample ? (wb.upsampleScale === 4 ? 2 : 1) : 0;
    return { v: 1 + (wb.reflect ? 1 : 0), e: 1, u: ups };
  }
  const plan = "plan" in editOptions.value ? editOptions.value.plan : null;
  let e = 0;
  let u = 0;
  for (const st of plan?.steps ?? []) {
    if (!isCloudOp(st.op)) continue;
    if (st.op === "upscale") u += 1;
    else e += 1;
  }
  return { v: 0, e, u };
}

const estimateText = computed(() => {
  const n = targetCount.value;
  // 未配置模型（FEAT-199）：比「多少钱」更该先说「现在还跑不起来」
  if (engineOrigin.value.origin === "unconfigured") return `⚠ ${engineOrigin.value.detail}`;
  // 整条路线都不出本机（含绑到本机地址的本地服务）→ 直接说免费，不摆价格栏
  if (!engineOrigin.value.leavesDevice) {
    return `免费（${engineOrigin.value.detail}）`;
  }
  if (wb.engine === "chain" && !chainHasCloud(wb.chain))
    return "免费（整条链都是本机算子，不调用外部模型）";
  const { v, e, u } = expectedCalls();
  if (wb.engine === "recipe" && !e && !u) return "免费（本机算子，不调用外部模型）";
  if (visionPrice.value <= 0 && editPrice.value <= 0 && upscalePrice.value <= 0)
    return "未配置模型单价，无法预估（在创意工坊 › 模型设置 › 计费里填写）";
  const total = (v * visionPrice.value + e * editPrice.value + u * upscalePrice.value) * n;
  const parts = [
    v ? `理解 ${v}` : "",
    wb.engine === "ai" ? `编辑 ≥${e}` : e ? `编辑 ${e}` : "",
    u ? `超分 ${u}` : "",
  ].filter(Boolean);
  return `≈ ${billing.formatMoney(total)}（${n} 张 × [${parts.join(" + ")}]）`;
});

/* ---------------- 样片两阶段 ---------------- */
const SAMPLE_COUNT = 5;
const sampleJob = computed(() => ai.jobOfTool("recipe"));

/** 单步微调面板展开状态：默认折叠，抽完样片自动展开一次（FEAT-180） */
const tuneOpen = ref(false);

/** 右栏 ② 卡头的小标：改了/跳过了几个参数（FEAT-182） */
const tuneSummary = computed(() =>
  [
    wb.tuneChangedCount ? `已改 ${wb.tuneChangedCount} 处` : "",
    wb.tuneSkippedCount ? `跳过 ${wb.tuneSkippedCount} 项` : "",
  ]
    .filter(Boolean)
    .join(" · "),
);

/** 配方步骤就是微调对象：参数来自 recipes.json + catalog 默认值，换配方才重建基线 */
watch(
  () => wb.recipeId,
  (id) => {
    wb.ensureTune(id, chainFromRecipe(id));
    if (wb.samplePhase !== "idle") wb.endSample();
  },
  { immediate: true },
);

const currentVerdictOf = (p: string) => {
  const list = sampleJob.value?.meta?.verifications ?? [];
  const i = wb.samplePaths.indexOf(p);
  return i >= 0 ? list[i] : undefined;
};
const sampleVerdictLabel = (p: string) => {
  const v = currentVerdictOf(p);
  return v ? verdictLabel(v) : "";
};
const outputSrcOf = (path: string): string => {
  const it = wb.items.find((i) => i.path === path);
  return it?.outputPath ? srcMap.value[it.outputPath] ?? "" : "";
};

/** 样片作业产物回填 → 进入微调，并把微调面板推到眼前 */
watch(
  () => {
    const j = sampleJob.value;
    return wb.samplePhase === "sampling" && j ? `${j.status}|${j.outputs.length}` : "";
  },
  () => {
    const j = sampleJob.value;
    if (!j || wb.samplePhase !== "sampling") return;
    if (j.status === "done") {
      j.outputs.forEach((out, i) => {
        const p = wb.samplePaths[i];
        if (p && out) wb.patch(p, { status: "done", progress: 100, outputPath: out });
      });
      wb.beginTuning();
      tuneOpen.value = true;
    } else if (j.status === "failed" || j.status === "cancelled") {
      notify.error("样片失败", j.error || "已取消");
      wb.endSample();
    }
  },
);

/** 自动抽 5 张（等距，不是取前 5 张）→ 跑样片 */
async function runSampleAuto() {
  if (busy.value) return;
  if (!guardReady()) return;
  const paths = pickSample(wb.items.map((i) => i.path), SAMPLE_COUNT);
  if (!paths.length) {
    notify.warning("还没有照片", "先导入一批照片");
    return;
  }
  await submitSample(paths, true);
}

/** 自选样片：进入中栏点选模式（张数不限） */
function startPicking() {
  if (busy.value) return;
  if (!wb.total) {
    notify.warning("还没有照片", "先导入一批照片");
    return;
  }
  wb.startPicking();
  wb.setViewMode("grid");
}

async function startPickedSample() {
  if (!wb.samplePaths.length) {
    notify.warning("还没选样片", "在中栏点缩略图勾选，想选几张都行");
    return;
  }
  await submitSample([...wb.samplePaths], false);
}

function guardReady(): boolean {
  if (!wb.total) {
    notify.warning("还没有照片", "先导入一批照片");
    return false;
  }
  if (!wb.outputDir) {
    notify.warning("还没有输出目录", "先选择结果要放在哪个文件夹（左侧「输出」）");
    return false;
  }
  if (wb.engine === "chain" && !planRunnable(wb.chain)) {
    notify.warning("步骤链是空的", "至少启用一步，或在 ② 里从配方起步");
    return false;
  }
  if (wb.engine === "recipe" && !wb.tuneRunnable) {
    notify.warning(
      "所有步骤都设成了不处理",
      "至少保留一个参数参与出片；再点一次「不处理」即可恢复",
    );
    return false;
  }
  return true;
}

async function submitSample(paths: string[], auto: boolean) {
  wb.runningBatch = paths;
  for (const p of paths) wb.patch(p, { status: "queued", progress: 0, error: undefined });
  wb.startSample(paths, auto);
  // 样片跑起来就回单张对比：选片是上一动作，接下来要看的是效果
  wb.setViewMode("loupe");
  try {
    await ai.run({
      tool: "recipe",
      paths,
      prompt: "",
      outputDir: wb.outputDir,
      options: editOptions.value,
    });
  } catch (e) {
    for (const p of paths) wb.patch(p, { status: "failed", error: String(e) });
    wb.runningBatch = [];
    wb.endSample();
    notify.error("样片提交失败", String(e));
  }
}

/** 应用到全部：带当前参数跑全量 */
async function applyTuned() {
  if (busy.value) return;
  const ok = await applyBatch();
  if (ok) wb.endSample();
}

function discardSample() {
  wb.endSample();
  notify.info("已回到主路径", "样片产物保留在输出目录，可直接使用");
}

/* ---------------- 步骤链 ---------------- */
const chainOpsLabel = computed(() => chainSummary(wb.chain));

/** 链条里需要的本地模型（缺了只在悬停提示，不阻断） */
const chainModelIds = computed(() => {
  const ids = new Set<string>();
  for (const s of wb.chain) {
    if (!s.enabled) continue;
    // 依赖由 catalog 的 requires.models 声明，这里不另抄一份映射（漏抄会静默不提示）
    const models = (opById(s.op)?.requires?.models ?? []) as unknown[];
    for (const m of models) {
      // det_10g 是扫描区共享件，不在工坊下载表里
      if (typeof m === "string" && m !== "det_10g") ids.add(m);
    }
  }
  return [...ids];
});

function loadRecipeIntoChain(id: string) {
  wb.loadChainFromRecipe(id);
  notify.info(`已载入「${recipeById(id)?.label}」`, `${chainFromRecipe(id).length} 步，可继续增减`);
}

/**
 * 选中一款影调风格（FEAT-198）：把它的中文原文接在用户描述后面，界面可见可编辑。
 * 再点同一款 = 取消（摘回那段原文；用户已手改过则原样保留，不擅自删他的字）。
 */
function pickStyle(id: string): void {
  if (busy.value) return;
  const next = toggleStyle(wb.styleId, id);
  if (next === wb.styleId) return;
  wb.prompt = switchStyleText(wb.prompt, wb.styleId, next);
  wb.styleId = next;
}

/* ---------------- 批量出片 ---------------- */
const activeTool = computed(() => (wb.engine === "ai" ? "smart_edit" : "recipe"));
const busy = computed(
  () =>
    wb.busy ||
    (!!job.value && (job.value.status === "queued" || job.value.status === "running")),
);

async function applyBatch(): Promise<boolean> {
  if (busy.value) return false;
  const target = wb.targets;
  if (!target.length) {
    notify.warning("还没有照片", "先导入一批照片再出片");
    return false;
  }
  if (!guardReady()) return false;

  const paths = target.map((i) => i.path);
  const what =
    wb.engine === "recipe" ? "一键出片" : wb.engine === "ai" ? "智能修图" : "自己组合";
  const ok = await notify.confirm(
    `批量${what} ${paths.length} 张？`,
    `${estimateText.value}；产物写到 ${wb.outputDir}，原图不动。`,
    { confirmText: "开始处理", type: "primary" },
  );
  if (!ok) return false;
  wb.runningBatch = paths;
  for (const p of paths) wb.patch(p, { status: "queued", progress: 0, error: undefined });
  try {
    await ai.run(
      wb.engine === "ai"
        ? {
            tool: "smart_edit",
            paths,
            prompt: wb.prompt,
            outputDir: wb.outputDir,
            options: {
              reflect: wb.reflect,
              rounds: 1,
              steps: wb.renderSteps,
              upsample: wb.upsample,
              upsampleScale: wb.upsampleScale,
              styleId: wb.styleId,
              strength: wb.strength / 100,
            },
          }
        : {
            tool: "recipe",
            paths,
            prompt: "",
            outputDir: wb.outputDir,
            options: editOptions.value,
          },
    );
  } catch (e) {
    for (const p of paths) wb.patch(p, { status: "failed", error: String(e) });
    wb.runningBatch = [];
    notify.error("提交失败", String(e));
    return false;
  }
  return true;
}

/* 预览触发：参数/方案/当前图任一变化就重算（防抖在 composable 里）。
   `previewReq` 已经把「步骤链 / 配方 / 引擎 / 当前图 / 预览开关」全部读进依赖，
   不再额外拼 `JSON.stringify(wb.chain)` 当 watch 源（每次改参数都做一次全链序列化）。
   放在这里而不是文件靠前位置 —— 这里所有被依赖的 computed 都已定义。 */
watch(
  previewReq,
  (req) => {
    if (req) preview.schedule(req);
    else preview.reset();
  },
  { immediate: true },
);

/** 切图：先丢掉上一张的预览图（否则新照片会先显示旧照片的效果），再立刻算一次 */
watch(currentPath, () => {
  preview.reset();
  const req = previewReq.value;
  if (req) void preview.run(req);
});

function onCancel() {
  const id = ai.activeJobId(activeTool.value);
  if (id) void ai.cancel(id);
}

/* 作业状态 → 回填每张图状态 */
watch(
  () => {
    const j = job.value;
    return j ? `${j.status}|${j.progress}|${j.outputs.length}` : "";
  },
  () => {
    const j = job.value;
    const batch = wb.runningBatch;
    if (!j || !batch.length) return;
    if (j.status === "queued" || j.status === "running") {
      for (const p of batch) wb.patch(p, { status: "running", progress: j.progress });
    } else if (j.status === "done") {
      batch.forEach((p, i) => {
        const out = j.outputs[i];
        wb.patch(p, { status: "done", progress: 100, outputPath: out ?? undefined });
      });
      void afterBatchDone(j.outputs.length || batch.length);
    } else if (j.status === "failed" || j.status === "cancelled") {
      for (const p of batch) wb.patch(p, { status: "failed", error: j.error || "已取消" });
      wb.runningBatch = [];
    }
  },
);

/** 批量完成后：提示 + 自动导入「创意工坊」相册（目录就是 outputDir） */
async function afterBatchDone(count: number) {
  wb.runningBatch = [];
  notify.success("修图完成", `已处理 ${count} 张，产物在 ${wb.outputDir}`);
  if (!wb.autoImport || !wb.outputDir) return;
  try {
    const albumId = await ensureAlbum();
    try {
      await contentStore.scanAlbumContent(albumId);
    } catch {
      /* 入库失败降级：文件已在输出目录，打开相册可再扫 */
    }
    await albumStore.fetchAlbums();
    notify.success("已自动导入『创意工坊』相册", "打开相册即可看到本次成品");
  } catch (e) {
    notify.warning("产物已写入目录，但相册入库失败", String(e));
  }
}

/** 确保「创意工坊」相册存在并指向 outputDir */
async function ensureAlbum(): Promise<number> {
  const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase();
  const albums = await albumStore.fetchAlbums();
  const hit = albums.find((a) => norm(a.path) === norm(wb.outputDir));
  if (hit) return hit.id;
  const created = await albumStore.createAlbum({
    name: "创意工坊",
    path: wb.outputDir,
    description: "AI 修图工作台产物自动导入",
  });
  return created.id;
}

/* ---------------- 视图与选片 ---------------- */
const layoutCols = computed(
  () =>
    `${wb.leftRailCollapsed ? "40px" : "208px"} minmax(0, 1fr) ${wb.rightRailCollapsed ? "40px" : "336px"}`,
);

/** 当前图：自选样片模式下点格子是勾样片，不是换对比对象 */
function selectPhoto(path: string) {
  currentPath.value = path;
  if (wb.samplePhase === "picking") wb.toggleSample(path);
}

/** 从样片卡/网格双击进单张对比 */
function focusPhoto(path: string) {
  currentPath.value = path;
  wb.setViewMode("loupe");
}

function toggleAll() {
  if (wb.selectedCount === wb.total) wb.clearSelection();
  else wb.selectAll();
}

const modeNotes: Record<WbEngine, string> = {
  recipe: "选一个配方整批跑完；主路径两次点击出片。",
  ai: "一句话说需求：语言模型规划 → 文生图模型执行 → 1K 出图；按调用计费。",
  chain: "自己组合原子组件：本机与云端都能插，同一组件可重复；本机步免费。",
};

/**
 * 当前路线的模型来源（FEAT-199）：路线按钮旁与卡头都用它。
 *
 * 三条路线分别按「它实际会跑什么」判：
 *   - 配方 → 该配方的算子集合（「人像」要本地形变/磨皮，「自然」是纯算法）；
 *   - 模型理解 → 只用云端（FEAT-197 已锁死），但仍可能被绑到本机网关，所以照常
 *     走 `originOfTool` 读绑定地址，而不是硬编码「云端 API」；
 *   - 自己组合 → 只看启用中的步。
 */
const engineOrigin = computed<OriginInfo>(() => {
  if (wb.engine === "ai") return originOfTool("smart_edit", ai.settings);
  if (wb.engine === "chain") return originOfChain(wb.chain, ai.settings);
  return originOfRecipe(wb.recipeId, ai.settings);
});

function setMode(m: WbEngine) {
  if (busy.value || wb.engine === m) return;
  wb.engine = m;
  if (wb.samplePhase !== "idle") wb.endSample();
}

const previewNote = computed(() => {
  if (!afterSrc.value) return "";
  return wb.engine !== "ai" && previewOn.value && preview.src.value ? "即时预览 · 1280px" : "";
});
</script>

<template>
  <div class="wb-page">
    <AppHeader
      title="AI 修图工作台"
      icon="🖼"
      :meta="`${wb.total} 张 · 已完成 ${wb.doneCount} · 进行中 ${wb.runningCount}`"
    />

    <div class="wb-crumb">
      <button class="wb-back" type="button" @click="router.push('/workshop')">← 创意工坊</button>
      <span class="wb-sep">/</span>
      <span class="wb-cur">AI 修图工作台</span>
      <span class="crumb-hint">固定工作区 · 胶片条横向滑动 · 网格一次看完</span>
    </div>

    <div class="wb-layout" :style="{ gridTemplateColumns: layoutCols }">
      <!-- ============ 左栏：工作集 / 导入 / 输出（可折叠） ============ -->
      <aside class="wb-left" :class="{ collapsed: wb.leftRailCollapsed }">
        <div v-if="!wb.leftRailCollapsed" class="rail-panel">
          <div class="ws-head">
            <span class="ws-num">{{ wb.total }}</span>
            <span class="ws-unit">张 · 本次工作集</span>
            <button
              class="rail-toggle"
              type="button"
              title="收起左栏"
              @click="wb.toggleRail('left')"
            >
              «
            </button>
          </div>

          <dl class="ws-stat">
            <div><dt>已完成</dt><dd class="ok">{{ wb.doneCount }}</dd></div>
            <div><dt>进行中</dt><dd>{{ wb.runningCount }}</dd></div>
            <div><dt>失败</dt><dd :class="{ bad: wb.failedCount }">{{ wb.failedCount }}</dd></div>
          </dl>

          <div class="imp">
            <button class="imp-btn primary" type="button" @click="addPhotos">＋ 添加照片</button>
            <button class="imp-btn" type="button" @click="importFolder">⤓ 导入文件夹</button>
            <button class="imp-btn" type="button" @click="loadFromWorkshop">⧉ 载入工坊素材</button>
          </div>

          <div class="sel-actions">
            <button class="mini" type="button" @click="wb.selectAll()">全选</button>
            <button class="mini" type="button" @click="wb.invertSelection()">反选</button>
            <button class="mini danger" type="button" @click="removeSelected">删除所选</button>
            <button class="mini" type="button" @click="wb.clearAll()">清空</button>
          </div>

          <div class="side-group">
            <h3 class="side-h">输出</h3>
            <div class="dir-row">
              <span class="dir-label">目录</span>
              <button class="dir-btn" type="button" title="选择输出目录" @click="pickOutputDir">
                {{ wb.outputDir || "未选择" }}
              </button>
            </div>
            <label class="row-check">
              <input v-model="wb.autoImport" type="checkbox" />
              出片后加入「创意工坊」
            </label>
            <p class="side-note">
              产物写进上面这个目录，原图不动。勾选后相册「创意工坊」<b>直接指向它</b>。
            </p>
          </div>
        </div>

        <div v-else class="rail-mini">
          <button
            class="rail-toggle"
            type="button"
            title="展开左栏"
            @click="wb.toggleRail('left')"
          >
            »
          </button>
          <span class="rail-mini-n">{{ wb.total }}</span>
          <span class="rail-mini-label">工作集</span>
        </div>
      </aside>

      <!-- ============ 中栏：对比 / 网格 + 胶片条 ============ -->
      <section class="wb-main">
        <div class="main-head">
          <h2 class="main-title">工作集</h2>
          <span class="main-counts">
            <span>{{ wb.total }} 张</span>
            <span v-if="wb.doneCount">· 已完成 {{ wb.doneCount }}</span>
            <span v-if="wb.runningCount">· 进行中 {{ wb.runningCount }}</span>
            <span v-if="wb.failedCount" class="bad">· 失败 {{ wb.failedCount }}</span>
          </span>

          <div class="main-actions">
            <div v-if="wb.total" class="seg2" role="group" aria-label="中栏视图">
              <button
                type="button"
                :class="{ on: wb.viewMode === 'loupe' }"
                title="单张对比"
                @click="wb.setViewMode('loupe')"
              >
                单张
              </button>
              <button
                type="button"
                :class="{ on: wb.viewMode === 'grid' }"
                title="网格选片（双击回单张）"
                @click="wb.setViewMode('grid')"
              >
                网格
              </button>
            </div>
            <button v-if="wb.total" class="main-act" type="button" @click="toggleAll">
              {{ wb.selectedCount === wb.total ? "取消全选" : "全选" }}
            </button>
            <button
              v-if="wb.selectedCount"
              class="main-act danger"
              type="button"
              title="只移出工作集，原图不动"
              @click="removeSelected"
            >
              删除所选
            </button>
            <span v-if="wb.selectedCount" class="sel-n">{{ wb.selectedCount }} 张已选</span>
            <button
              v-if="wb.selectedCount"
              class="apply-sel"
              type="button"
              :disabled="busy"
              :title="estimateText"
              @click="applyBatch"
            >
              批量套用
            </button>
          </div>
        </div>

        <!-- 自选样片提示条：张数不限 -->
        <div v-if="wb.samplePhase === 'picking'" class="pick-banner">
          <span>正在选样片：点缩略图勾选，<b>张数不限</b>（已选 {{ wb.samplePaths.length }} 张）</span>
          <span class="pick-actions">
            <button type="button" @click="startPickedSample">开始出样片</button>
            <button type="button" @click="wb.endSample()">取消</button>
          </span>
        </div>

        <div v-if="!wb.total" class="empty-tray">
          <p class="empty-title">工作集还是空的</p>
          <p class="dim">从左侧「添加照片 / 导入文件夹 / 载入工坊素材」放一批照片进来。</p>
          <button class="empty-add" type="button" @click="addPhotos">＋ 添加照片</button>
        </div>

        <template v-else>
          <div v-if="wb.viewMode === 'loupe'" class="wb-stage">
            <div class="wb-viewer">
              <WorkbenchCompare
                v-model:pos="slider"
                v-model:zoom="viewerZoom"
                :before-src="beforeSrc"
                :after-src="afterSrc"
                :aspect="aspect"
                :previewing="preview.previewing.value"
                :preview-note="previewNote"
              >
                <template #empty>尚未出片 · 先跑样片，或直接批量套用</template>
              </WorkbenchCompare>
            </div>
            <div v-if="current" class="cmp-cap">
              <span class="cmp-name">{{ current.name }}</span>
              <span v-if="currentVerdict" class="cmp-verdict" :class="currentVerdict.verdict">
                {{ verdictLabel(currentVerdict) }}
              </span>
              <span class="cmp-hint">
                {{
                  afterSrc
                    ? viewerZoom > 1
                      ? "拖动平移 · 拖白线对比 · Ctrl+滚轮缩放"
                      : "拖动竖线对比 · Ctrl+滚轮缩放"
                    : "点胶片条切换对比对象"
                }}
              </span>
              <span class="cmp-right">
                <label
                  v-if="wb.engine !== 'ai'"
                  class="pv-toggle"
                  title="改参数后用当前这张重算预览（本机免费）"
                >
                  <input v-model="previewOn" type="checkbox" />
                  即时预览
                </label>
                <span
                  v-if="preview.dropped.value.length"
                  class="pv-warn"
                  :title="preview.error.value || '云端步每次都是计费调用，预览只跑本机算子；正式出片时仍会执行'"
                >
                  未包含 {{ preview.dropped.value.map(opLabel).join("、") }}
                </span>
              </span>
            </div>
          </div>

          <WorkbenchGrid
            v-else
            class="wb-gridview"
            :items="thumbs"
            :current-path="currentPath"
            :picking="wb.samplePhase === 'picking'"
            @pick="selectPhoto"
            @toggle-select="wb.toggleSelect"
            @remove="removeOne"
            @loupe="wb.setViewMode('loupe')"
          />

          <WorkbenchFilmstrip
            v-if="wb.viewMode !== 'grid'"
            :items="thumbs"
            :current-path="currentPath"
            :picking="wb.samplePhase === 'picking'"
            @pick="selectPhoto"
            @toggle-select="wb.toggleSelect"
          />
        </template>
      </section>

      <!-- ============ 右栏：五段式（可折叠） ============ -->
      <aside class="wb-right" :class="{ collapsed: wb.rightRailCollapsed }">
        <template v-if="!wb.rightRailCollapsed">
          <div class="rail-head">
            <span class="rail-head-t">出片方案</span>
            <button
              class="rail-toggle"
              type="button"
              title="收起右栏"
              @click="wb.toggleRail('right')"
            >
              »
            </button>
          </div>

          <div class="rail-scroll">
            <!-- 1 怎么出片 -->
            <section class="wb-card">
              <div class="card-h"><span class="num">1</span><h3>怎么出片</h3></div>
              <div class="seg3">
                <button
                  v-for="m in (['recipe', 'ai', 'chain'] as WbEngine[])"
                  :key="m"
                  type="button"
                  :class="{ on: wb.engine === m }"
                  :disabled="busy"
                  @click="setMode(m)"
                >
                  {{ m === "recipe" ? "配方 · 一键" : m === "ai" ? "模型理解" : "自己组合" }}
                </button>
              </div>
              <p class="side-note">{{ modeNotes[wb.engine] }}</p>
            </section>

            <!-- 2 对应方案的细调面板 -->
            <section class="wb-card">
              <div class="card-h">
                <span class="num">2</span>
                <h3>{{ wb.engine === "chain" ? "步骤 · 按顺序执行" : wb.engine === "ai" ? "修图意图" : "配方" }}</h3>
                <span class="card-extra">
                  {{
                    wb.engine === "chain"
                      ? `${wb.chain.filter((s) => s.enabled).length} 步 · ${chainHasCloud(wb.chain) ? "含云端步" : "全本机免费"}`
                      : wb.engine === "ai"
                        ? "按模型计费"
                        : tuneSummary || "配方级"
                  }}
                </span>
              </div>

              <template v-if="wb.engine === 'recipe'">
                <RecipePicker v-model="wb.recipeId" :disabled="busy" />
                <WorkbenchTune
                  v-model:open="tuneOpen"
                  :steps="wb.tuneSteps"
                  :base="wb.tuneBase"
                  :off="wb.tuneOff"
                  :changed-count="wb.tuneChangedCount"
                  :skipped-count="wb.tuneSkippedCount"
                  :disabled="busy"
                  @param="wb.setTuneParam"
                  @off="wb.setTuneOff"
                  @reset-step="wb.resetTuneStep"
                  @reset-all="wb.resetTune"
                />
              </template>

              <template v-else-if="wb.engine === 'ai'">
                <textarea
                  v-model="wb.prompt"
                  class="intent"
                  rows="4"
                  placeholder="用一句话描述想要的统一风格，例如：把整组照片调成温暖通透的胶片感，皮肤自然通透一点，天空压暗但保留层次。"
                />
                <div class="chips">
                  <button
                    v-for="s in MODEL_STYLES"
                    :key="s.id"
                    class="chip"
                    type="button"
                    :class="{ on: wb.styleId === s.id }"
                    :disabled="busy"
                    :title="s.zh"
                    @click="pickStyle(s.id)"
                  >
                    {{ s.label }}
                  </button>
                  <button v-if="wb.styleId" class="chip" type="button" :disabled="busy" @click="pickStyle('')">
                    清空风格
                  </button>
                </div>
                <p v-if="wb.styleId" class="side-note">
                  已把「{{ modelStyleLabel(wb.styleId) }}」的描述接在下面这段文字后面（可直接改）。
                  它描述影调与光线，不改构图与物体形状。
                </p>
                <div class="field">
                  <div class="field-head"><span>强度</span><span class="field-val">{{ wb.strength }}%</span></div>
                  <input v-model.number="wb.strength" class="strength" type="range" min="0" max="100" />
                </div>

                <!-- 文生图采样步数（FEAT-197）：不可设置时默认 1，默认档不写进请求体 -->
                <div class="field">
                  <div class="field-head"><span>步数</span><span class="field-val">{{ wb.renderSteps }}</span></div>
                  <input
                    v-model.number="wb.renderSteps"
                    class="strength"
                    type="range"
                    min="1"
                    max="30"
                    step="1"
                    :disabled="busy"
                  />
                  <p class="side-note">文生图的采样步数，越大越细但越慢越贵；默认 1。</p>
                </div>

                <label class="row-check">
                  <input v-model="wb.upsample" type="checkbox" :disabled="busy" />
                  上采样（出图后再放大）
                </label>
                <div v-if="wb.upsample" class="seg">
                  <button
                    v-for="s in [2, 4]"
                    :key="s"
                    type="button"
                    class="seg-btn"
                    :class="{ on: wb.upsampleScale === s }"
                    :disabled="busy"
                    @click="wb.upsampleScale = s"
                  >
                    {{ s }}×
                  </button>
                </div>
                <p v-if="wb.upsample" class="side-note">
                  默认 2×；4× 用 2× 跑两遍，细节比一次写 4× 更实，但费用也是两倍。
                </p>

                <label class="row-check" style="margin-top: 8px">
                  <input v-model="wb.reflect" type="checkbox" :disabled="busy" />
                  反思复核（出图后自检一轮）
                </label>
                <p class="side-note">
                  这条路线只用云端模型：原图 → 短边 1080 → 语言模型规划 → 文生图执行 → 1K 出图。
                  想用本机免费组件请切「自己组合」。
                </p>
              </template>

              <template v-else>
                <div class="from-recipe">
                  <span>从配方开始：</span>
                  <button
                    v-for="r in RECIPES"
                    :key="r.id"
                    type="button"
                    :disabled="busy"
                    @click="loadRecipeIntoChain(r.id)"
                  >
                    {{ r.label }}
                  </button>
                </div>
                <WorkbenchSteps
                  :steps="wb.chain"
                  :disabled="busy"
                  @add="wb.addChainOp"
                  @remove="wb.removeChainOp"
                  @toggle="wb.toggleChainOp"
                  @param="wb.setChainParam"
                />
                <div v-if="chainModelIds.length" class="model-hint">
                  <LocalModelHint :ids="chainModelIds" />
                </div>
              </template>

              <JobStepsPanel :job="job" />
            </section>

            <!-- 3 样片 -->
            <section v-if="wb.engine !== 'ai'" class="wb-card">
              <div class="card-h">
                <span class="num">3</span><h3>样片</h3>
                <span class="card-extra">
                  {{
                    wb.samplePhase === "idle"
                      ? "自动 5 张"
                      : wb.samplePhase === "picking"
                        ? `已选 ${wb.samplePaths.length} 张`
                        : wb.samplePhase === "sampling"
                          ? "正在出样片…"
                          : "调完可套全量"
                  }}
                </span>
              </div>

              <template v-if="wb.samplePhase === 'idle'">
                <div class="s-row">
                  <button class="s-btn primary" type="button" :disabled="busy" @click="runSampleAuto">
                    自动抽 5 张
                  </button>
                  <button class="s-btn" type="button" :disabled="busy" @click="startPicking">自选样片</button>
                </div>
                <p class="side-note">
                  先跑几张看效果，满意再套全量；自动抽是<b>等距取</b>（首中尾都有），自选<b>不限张数</b>。
                </p>
              </template>

              <template v-else>
                <div class="sample-strip">
                  <div
                    v-for="p in wb.samplePaths"
                    :key="p"
                    class="sample-cell"
                    :class="{ on: p === currentPath }"
                    @click="focusPhoto(p)"
                  >
                    <img v-if="srcMap[p]" :src="srcMap[p]" :alt="baseName(p)" loading="lazy" />
                    <span v-else class="sample-ph" />
                    <span
                      v-if="outputSrcOf(p)"
                      class="sample-after"
                      :style="{ backgroundImage: `url(${outputSrcOf(p)})` }"
                    />
                    <span class="sample-name">{{ baseName(p) }}</span>
                    <span v-if="sampleVerdictLabel(p)" class="sample-verdict">{{ sampleVerdictLabel(p) }}</span>
                  </div>
                </div>

                <template v-if="wb.samplePhase === 'tuning'">
                  <p class="side-note">
                    要改就展开 <b>② 单步微调</b>（同一处改参，右侧对比框即时刷新）。
                  </p>
                  <div class="sample-actions">
                    <button class="sample-apply" type="button" :disabled="busy" @click="applyTuned">
                      应用到全部 {{ targetCount }} 张
                    </button>
                    <button class="sample-discard" type="button" :disabled="busy" @click="discardSample">
                      放弃
                    </button>
                  </div>
                </template>
              </template>
            </section>

            <!-- 4 质检 -->
            <section class="wb-card qc" :class="qcClass">
              <div class="card-h">
                <span class="num">4</span><h3>出片质检</h3>
                <span class="qc-badge" :class="qcClass">{{ qcText }}</span>
              </div>
              <p v-if="!qc.total" class="side-note">出片后自动复核高光 / 暗部 / 肤色 / 锐度；本地零模型、免费。</p>
              <template v-else>
                <dl class="qc-grid">
                  <div><dt>通过</dt><dd class="ok">{{ qc.pass }}</dd></div>
                  <div><dt>异常</dt><dd :class="{ bad: qc.fail }">{{ qc.fail }}</dd></div>
                  <div><dt>待核</dt><dd>{{ qc.unknown }}</dd></div>
                </dl>
                <ul v-if="qc.issues.length" class="qc-issues">
                  <li v-for="(s, i) in qc.issues" :key="i">{{ s }}</li>
                </ul>
                <p v-else class="side-note">未发现客观指标异常。</p>
              </template>
            </section>

            <!-- 5 导出 -->
            <section class="wb-card">
              <div class="card-h"><span class="num">5</span><h3>导出</h3></div>
              <dl class="spec">
                <div><dt>格式</dt><dd>JPG · 品质高</dd></div>
                <div><dt>尺寸</dt><dd>{{ chainPlan(wb.chain).steps.some((s) => s.op === "local_upscale") ? "按倍数放大" : "保持原图" }}</dd></div>
                <div><dt>加入相册</dt><dd>{{ wb.autoImport ? "「创意工坊」" : "关闭" }}</dd></div>
                <div><dt>当前方案</dt><dd>{{ wb.engine === "recipe" ? `配方 · ${toolLabel("recipe")}` : wb.engine === "ai" ? "模型理解" : chainOpsLabel }}</dd></div>
              </dl>
              <div class="cost">{{ estimateText }}</div>
              <button class="apply" type="button" :disabled="busy || !wb.total" @click="applyBatch">
                {{ busy ? "处理中…" : `批量套用到 ${targetCount} 张` }}
              </button>
              <button v-if="busy" class="cancel" type="button" @click="onCancel">取消</button>
            </section>
          </div>
        </template>

        <div v-else class="rail-mini">
          <button
            class="rail-toggle"
            type="button"
            title="展开右栏"
            @click="wb.toggleRail('right')"
          >
            «
          </button>
          <span v-if="wb.tuneChangedCount" class="rail-mini-n">{{ wb.tuneChangedCount }}</span>
          <span class="rail-mini-label">出片方案</span>
        </div>
      </aside>
    </div>
  </div>
</template>

<style scoped>
/* ==========================================================================
   容器面：一个令牌统一所有卡片。
   为什么不用 `.panel` 这个类名：全局 main.css 里的 `body.theme-dark .panel`
   特异性更高，会把卡片刷成藏青玻璃（页面是松夜绿），这就是用户问的
   「为什么面板和背景色不同」。改名 + 一个 surface 令牌，两边都不再打架。
   ========================================================================== */
.wb-page {
  /* 两侧组件底色 = 主题预设里的「组件底色」（主题 store 下发的 --color-surface，
     与 Home / 其他 glass-card 同一个填充），中间查看器仍保持自己的黑底。 */
  --wb-surface: var(--color-surface, rgba(255, 255, 255, 0.03));
  /* FEAT-180：固定工作区 —— 顶栏/面包屑之后所有高度归三栏，页面本身不滚 */
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}

.wb-crumb {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px var(--gutter) 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.wb-back {
  border: none;
  background: none;
  padding: 2px 4px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  cursor: pointer;
  border-radius: 5px;
}
.wb-back:hover {
  color: var(--fg);
}
.wb-sep {
  opacity: 0.5;
}
.wb-cur {
  color: var(--fg);
}
.crumb-hint {
  margin-left: auto;
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.wb-layout {
  flex: 1;
  min-height: 0;
  display: grid;
  gap: 18px;
  padding: 12px var(--gutter) 0;
  align-items: stretch;
  transition: grid-template-columns var(--dur-2, 0.2s) var(--ease-out, ease);
}

/* ---------- 侧栏通用（折叠 = 40px 竖排标签） ---------- */
.wb-left,
.wb-right {
  min-width: 0;
  min-height: 0;
}
.wb-left {
  overflow-y: auto;
  scrollbar-width: thin;
}
/* 左栏整块是一个组件面板：底色与其他卡片一致，内容不再直接压在壁纸上 */
.rail-panel {
  padding: 10px 10px 12px;
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  background: var(--wb-surface);
}
.wb-right {
  display: flex;
  flex-direction: column;
}
.rail-toggle {
  flex: none;
  width: 22px;
  height: 22px;
  margin-left: auto;
  padding: 0;
  border: 1px solid var(--c-rule);
  border-radius: 5px;
  background: none;
  color: var(--fg-2);
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
}
.rail-toggle:hover {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.wb-left.collapsed,
.wb-right.collapsed {
  align-items: center;
  padding-top: 2px;
}
.rail-mini {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  height: 100%;
  padding-top: 2px;
}
.rail-mini .rail-toggle {
  margin-left: 0;
}
.rail-mini-n {
  font-size: var(--fs-body);
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  color: var(--fg);
}
.rail-mini-label {
  writing-mode: vertical-rl;
  font-size: 10.5px;
  letter-spacing: 0.22em;
  color: var(--fg-2);
  white-space: nowrap;
}

/* ---------- 左栏 ---------- */
.ws-head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 10px;
}
.ws-num {
  font-size: var(--fs-display);
  font-weight: 700;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}
.ws-unit {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.ws-stat {
  display: flex;
  gap: 14px;
  margin: 0 0 12px;
  padding: 9px 0;
  border-top: 1px solid var(--c-rule);
  border-bottom: 1px solid var(--c-rule);
}
.ws-stat > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.ws-stat dt {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.ws-stat dd {
  margin: 0;
  font-size: var(--fs-body);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.ws-stat dd.ok {
  color: var(--c-ok, #6cd48a);
}
.ws-stat dd.bad {
  color: var(--c-danger, #e2706d);
}

.imp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 12px;
}
.imp-btn {
  height: 34px;
  padding: 0 12px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-body);
  text-align: left;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), border-color var(--dur-2) var(--ease-out);
}
.imp-btn:hover {
  color: var(--fg);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.imp-btn.primary {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 40%, transparent);
}

.sel-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  padding: 11px 0 0;
  border-top: 1px solid var(--c-rule);
}
.mini {
  height: 24px;
  padding: 0 6px;
  border-radius: 5px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.mini:hover {
  color: var(--fg);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.mini.danger:hover {
  color: var(--c-danger);
}

.side-group {
  margin-top: 14px;
  border-top: 1px solid var(--c-rule);
  padding-top: 12px;
}
.side-h {
  margin: 0 0 8px;
  font-size: var(--fs-label);
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
}
.side-note {
  margin: 8px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-3, var(--fg-2));
  line-height: 1.55;
}
.side-note b {
  color: var(--fg-2);
  font-weight: 600;
}
.dir-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.dir-label {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  flex: 0 0 auto;
}
.dir-btn {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 8px;
  border-radius: 6px;
  border: 1px dashed var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  text-align: left;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  cursor: pointer;
}
.dir-btn:hover {
  color: var(--c-accent);
}
.row-check {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  margin-top: 10px;
  font-size: var(--fs-body);
  color: var(--fg);
  cursor: pointer;
}
.row-check input {
  accent-color: var(--c-accent);
  width: 14px;
  height: 14px;
}

/* ---------- 中栏 ---------- */
.wb-main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}
.main-head {
  flex: none;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 10px;
  margin-bottom: 8px;
}
.main-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
}
.main-counts {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  white-space: nowrap;
}
.main-counts .bad {
  color: var(--c-danger);
}
.main-actions {
  margin-left: auto;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}
.seg2 {
  display: flex;
  gap: 2px;
  padding: 2px;
  border-radius: 7px;
  border: 1px solid var(--c-rule);
}
.seg2 button {
  height: 22px;
  padding: 0 9px;
  border: none;
  border-radius: 5px;
  background: none;
  color: var(--fg-2);
  font-size: 10.5px;
  cursor: pointer;
}
.seg2 button.on {
  color: #1a1608;
  background: var(--c-accent);
}
.main-act {
  height: 26px;
  padding: 0 10px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.main-act:hover {
  color: var(--fg);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.main-act.danger:hover {
  color: var(--c-danger);
}
.sel-n {
  font-size: var(--fs-meta);
  color: var(--c-accent);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.apply-sel {
  height: 26px;
  padding: 0 13px;
  border: none;
  border-radius: var(--r-pill);
  background: var(--c-accent);
  color: #221803;
  font-size: var(--fs-meta);
  font-weight: 650;
  cursor: pointer;
}
.apply-sel:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.pick-banner {
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
  padding: 7px 12px;
  border-radius: var(--r-ui);
  border: 1px solid color-mix(in srgb, var(--c-accent) 55%, transparent);
  background: color-mix(in srgb, var(--c-accent) 10%, transparent);
  font-size: var(--fs-meta);
  color: var(--fg);
}
.pick-banner b {
  color: var(--c-accent);
}
.pick-actions {
  margin-left: auto;
  display: flex;
  gap: 6px;
}
.pick-actions button {
  height: 24px;
  padding: 0 10px;
  border-radius: 5px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.pick-actions button:hover {
  color: var(--fg);
}

.wb-stage {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.wb-viewer {
  position: relative;
  flex: 1;
  min-height: 0;
}
.wb-gridview {
  flex: 1;
  min-height: 0;
}
.cmp-cap {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 6px 0 2px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  min-height: 20px;
}
.cmp-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cmp-hint {
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cmp-right {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
}
.pv-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  cursor: pointer;
  white-space: nowrap;
}
.pv-toggle input {
  accent-color: var(--c-accent);
  width: 12px;
  height: 12px;
}
.pv-warn {
  font-size: 10px;
  color: var(--c-danger);
}
.cmp-verdict {
  flex: none;
  padding: 1px 7px;
  border-radius: 999px;
  font-size: var(--fs-meta);
  border: 1px solid currentColor;
}
.cmp-verdict.pass {
  color: var(--c-ok, #6cd48a);
}
.cmp-verdict.fail {
  color: var(--c-danger, #e2706d);
}
.cmp-verdict.unknown {
  color: var(--fg-2);
}

.empty-tray {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 4px;
  padding: 40px 20px;
  border: 1px dashed var(--c-rule);
  border-radius: var(--r-ui);
  text-align: center;
  color: var(--fg-2);
  font-size: var(--fs-body);
}
.empty-title {
  margin: 0;
  color: var(--fg);
  font-weight: 600;
}
.empty-tray .dim {
  margin: 0;
  opacity: 0.8;
  font-size: var(--fs-meta);
}
.empty-add {
  margin-top: 10px;
  height: 30px;
  padding: 0 14px;
  border: 1px solid color-mix(in srgb, var(--c-accent) 45%, transparent);
  border-radius: var(--r-ui);
  background: none;
  color: var(--c-accent);
  font-size: var(--fs-body);
  cursor: pointer;
}

/* ---------- 右栏 ---------- */
.rail-head {
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.rail-head-t {
  font-size: 10.5px;
  letter-spacing: 0.18em;
  color: var(--fg-2);
  text-transform: uppercase;
}
.rail-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding-right: 2px;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.16) transparent;
}
.rail-scroll::-webkit-scrollbar {
  width: 6px;
}
.rail-scroll::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.14);
  border-radius: 3px;
}

.wb-card {
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  padding: 12px 14px;
  background: var(--wb-surface);
}
.card-h {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}
.card-h h3 {
  margin: 0;
  flex: none;
  white-space: nowrap;
  font-size: var(--fs-label);
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
}
.card-extra {
  margin-left: auto;
  min-width: 0;
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.num {
  flex: none;
  width: 17px;
  height: 17px;
  border-radius: 50%;
  display: grid;
  place-items: center;
  font-size: 9.5px;
  color: var(--c-accent);
  border: 1px solid color-mix(in srgb, var(--c-accent) 55%, transparent);
}

.seg3 {
  display: flex;
  gap: 6px;
}
.seg3 button {
  flex: 1;
  height: 28px;
  border-radius: 6px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 11.5px;
  cursor: pointer;
  transition: color var(--dur-2), border-color var(--dur-2), background var(--dur-2);
}
.seg3 button:hover:not(:disabled) {
  color: var(--fg);
}
.seg3 button.on {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
  background: color-mix(in srgb, var(--c-accent) 12%, transparent);
}
.seg3 button:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.intent {
  width: 100%;
  resize: vertical;
  min-height: 72px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  /* 深浅主题自适应：深底上是一层微亮，浅底上是一层微暗 */
  background: color-mix(in srgb, var(--fg) 8%, transparent);
  color: var(--fg);
  font-family: inherit;
  font-size: var(--fs-body);
  line-height: 1.6;
  padding: 9px 11px;
  box-sizing: border-box;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
  margin: 10px 0 4px;
}
.chip {
  height: 25px;
  padding: 0 10px;
  border-radius: var(--r-pill);
  border: none;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}
.chip:hover {
  color: var(--fg);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
/* 选中的影调风格（FEAT-198）：用金色描边而不是填充，与左栏「已选用」一致 */
.chip.on {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px var(--c-accent);
}
.chip:disabled {
  opacity: 0.45;
  cursor: default;
}
.field {
  margin: 12px 0;
}
.field-head {
  display: flex;
  justify-content: space-between;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  margin-bottom: 6px;
}
.field-val {
  color: var(--c-accent);
  font-variant-numeric: tabular-nums;
}
.strength {
  width: 100%;
  accent-color: var(--c-accent);
}
.seg {
  display: flex;
  gap: 6px;
  margin-top: 8px;
}
.seg-btn {
  flex: 1;
  height: 26px;
  border-radius: 6px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.seg-btn.on {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
}

.from-recipe {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}
.from-recipe span {
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
}
.from-recipe button {
  height: 22px;
  padding: 0 8px;
  border-radius: var(--r-pill);
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 10.5px;
  cursor: pointer;
}
.from-recipe button:hover:not(:disabled) {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 45%, transparent);
}
.model-hint {
  margin-top: 10px;
}

.s-row {
  display: flex;
  gap: 6px;
}
.s-btn {
  flex: 1;
  height: 30px;
  border-radius: 6px;
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.s-btn:hover:not(:disabled) {
  color: var(--fg);
}
.s-btn.primary {
  color: var(--c-accent);
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
  background: color-mix(in srgb, var(--c-accent) 10%, transparent);
}
.s-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.sample-strip {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  padding-bottom: 4px;
}
.sample-cell {
  position: relative;
  flex: none;
  width: 96px;
  height: 72px;
  border-radius: var(--r-ui);
  overflow: hidden;
  border: 1px solid var(--c-rule);
  cursor: pointer;
}
.sample-cell.on {
  border-color: var(--c-accent);
  box-shadow: 0 0 0 1px var(--c-accent);
}
.sample-cell img,
.sample-ph {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.sample-ph {
  display: grid;
  place-items: center;
  background: var(--hover, rgba(120, 120, 130, 0.08));
  color: var(--fg-2);
}
.sample-after {
  position: absolute;
  inset: 0 0 0 50%;
  background-size: cover;
  background-position: center;
  box-shadow: inset 1px 0 0 rgba(255, 255, 255, 0.6);
}
.sample-name {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 1px 4px;
  font-size: 9.5px;
  color: #fff;
  background: rgba(0, 0, 0, 0.45);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.sample-verdict {
  position: absolute;
  top: 3px;
  right: 3px;
  padding: 0 5px;
  border-radius: 999px;
  font-size: 8.5px;
  background: rgba(10, 9, 13, 0.7);
  /* 压在缩略图上的深色角标：不跟主题文字色走，浅色主题下也要白字 */
  color: #fff;
}
.sample-actions {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
.sample-apply {
  flex: 1;
  height: 32px;
  border: none;
  border-radius: var(--r-ui);
  background: var(--c-accent);
  color: #221803;
  font-size: var(--fs-body);
  font-weight: 650;
  cursor: pointer;
}
.sample-discard {
  flex: none;
  height: 32px;
  padding: 0 12px;
  border-radius: var(--r-ui);
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-body);
  cursor: pointer;
}

.qc .card-h {
  justify-content: flex-start;
}
.qc-badge {
  margin-left: auto;
  padding: 1px 8px;
  border-radius: 999px;
  font-size: var(--fs-meta);
  letter-spacing: 0;
  text-transform: none;
  border: 1px solid currentColor;
}
.qc-badge.idle {
  color: var(--fg-2);
}
.qc-badge.ok {
  color: var(--c-ok, #6cd48a);
}
.qc-badge.warn {
  color: var(--c-danger, #e2706d);
}
.qc-badge.muted {
  color: var(--fg-2);
}
.qc-grid {
  display: flex;
  gap: 16px;
  margin: 0 0 8px;
}
.qc-grid > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.qc-grid dt {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.qc-grid dd {
  margin: 0;
  font-size: var(--fs-body);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}
.qc-grid dd.ok {
  color: var(--c-ok, #6cd48a);
}
.qc-grid dd.bad {
  color: var(--c-danger, #e2706d);
}
.qc-issues {
  margin: 0;
  padding-left: 16px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.6;
}

.spec {
  margin: 0;
}
.spec div {
  display: flex;
  justify-content: space-between;
  gap: 10px;
  padding: 3px 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.spec dd {
  margin: 0;
  color: var(--fg);
  text-align: right;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cost {
  margin-top: 8px;
  padding: 8px 10px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--fg) 6%, transparent);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}
.apply {
  width: 100%;
  height: 34px;
  margin-top: 9px;
  border: none;
  border-radius: var(--r-ui);
  background: var(--c-accent);
  color: #221803;
  font-size: var(--fs-body);
  font-weight: 650;
  cursor: pointer;
}
.apply:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.cancel {
  width: 100%;
  height: 30px;
  margin-top: 8px;
  border: 1px solid var(--c-danger);
  border-radius: var(--r-ui);
  background: none;
  color: var(--c-danger);
  font-size: var(--fs-body);
  cursor: pointer;
}

/* ---------- 窄窗：回退为整页滚动，三栏折成一栏 ---------- */
@media (max-width: 1180px) {
  .wb-page {
    height: auto;
    min-height: 100vh;
    overflow: visible;
  }
  .wb-layout {
    display: block;
    padding: 12px var(--sp-5) 96px;
  }
  .wb-left,
  .wb-right {
    position: static;
    max-height: none;
    overflow: visible;
    margin-bottom: 14px;
  }
  .wb-right {
    display: block;
  }
  .rail-scroll {
    overflow: visible;
  }
  /* 窄窗没有「剩余高度」可分，给一个明确视口高度；必须先解掉 base 的 flex:1，
     否则 flex-basis:0 会压过 height，只剩 min-height。 */
  .wb-stage {
    flex: none;
    height: 56vh;
    min-height: 320px;
  }
  .wb-gridview {
    flex: none;
    height: 62vh;
  }
  .rail-mini {
    flex-direction: row;
    height: auto;
    gap: 8px;
    padding: 4px 0;
  }
  .rail-mini .rail-toggle {
    margin-left: 0;
  }
  .rail-mini-label {
    writing-mode: horizontal-tb;
    letter-spacing: 0.08em;
  }
  .crumb-hint {
    display: none;
  }
}
</style>
