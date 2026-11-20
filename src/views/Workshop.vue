<script setup lang="ts">
/**
 * 创意工坊（FEAT-150 → FEAT-168 三层 IA → FEAT-169 按类型分组 → FEAT-197 工作台模式）
 * ===================================================================
 * ## 定位
 *
 * **LR 导出 JPEG 之后的快速出图站**：懒人快速出图，不做精修。
 * 判断标准：从导入到出图 ≤3 次点击、全程不出现调参面板。
 * 硬约束见 `.dsh/knowledge/workshop-positioning.md`。
 *
 * ## 三层 IA（按「要不要自己选步骤」分，不按本机/云端分）
 *
 *   ① 一键出片（默认落地页）：选图 → 选配方 → 出片，不暴露任何单算子
 *   ② 工作台：独立路由 /workshop/workbench，进入时**默认载入本页素材托盘**；
 *      三条路线（配方 / 模型理解 / 创意组合）+ 前后对比 + 批量出片
 *   ③ 单步工具：默认折叠，按**类型**分组（影调与滤镜 / 人像 / 画质 / 创作 / 处理），
 *      规划中项沉底
 *
 * FEAT-197 把 ② 从「批量套用」改名为「工作台」：它已经不是「把某个配方套到一批图」
 * 的单向动作，而是一个能看前后对比、能选三条路线、能自己拼组件的工作空间。
 * 入口文案改「空间名」，页面内的「批量套用」按钮保留「动作名」——两者语义不同。
 *
 * 分组只是展示口径，不改变任何工具归属或后端能力。
 *
 * 工具定义全部来自 `op_catalog.json`（经 `utils/toolCatalog.ts`），新增工具只改那一份 JSON。
 *
 * 布局沿用相册页/时间线页的「左栏导航 + 右栏内容」，视觉沿用既有设计系统
 * （design/design-system.md），只做 refinement 不 redesign。
 */
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import AppHeader from "../components/ui/AppHeader.vue";
import PhotoTray from "../components/workshop/PhotoTray.vue";
import CompressTool from "../components/workshop/CompressTool.vue";
import ToolIcon from "../components/workshop/ToolIcon.vue";
import AiSettingsDialog from "../components/workshop/ai/AiSettingsDialog.vue";
import RecipeTool from "../components/workshop/ai/RecipeTool.vue";
import JournalTool from "../components/workshop/ai/JournalTool.vue";
import BeautyTool from "../components/workshop/ai/BeautyTool.vue";
import StyleTransferTool from "../components/workshop/ai/StyleTransferTool.vue";
import UpscaleTool from "../components/workshop/ai/UpscaleTool.vue";
import SmartEditTool from "../components/workshop/ai/SmartEditTool.vue";
import FilmSimTool from "../components/workshop/ai/FilmSimTool.vue";
import ColorTransferTool from "../components/workshop/ai/ColorTransferTool.vue";
import RestoreTool from "../components/workshop/ai/RestoreTool.vue";
import AutoGradeTool from "../components/workshop/ai/AutoGradeTool.vue";
import LocalBeautyTool from "../components/workshop/ai/LocalBeautyTool.vue";
import LocalReshapeTool from "../components/workshop/ai/LocalReshapeTool.vue";
import OcrTool from "../components/workshop/ai/OcrTool.vue";
import OriginTag from "../components/workshop/ai/OriginTag.vue";
import { pickPhotos } from "../composables/useOpenDialog";
import {
  primaryTool,
  singleToolsByGroup,
  toolById,
  TOOLS,
  type ToolDef as CatalogToolDef,
} from "../utils/toolCatalog";
import { isWorkshopTool, type WorkshopToolId } from "../stores/workshopUi";
import { useWorkshopUiStore } from "../stores/workshopUi";
import { useCompressTaskStore } from "../stores/compressTask";
import { useAiStudioStore } from "../stores/aiStudio";
import { dedupePhotoPaths, splitPhotoPaths, workbenchRouteTarget } from "../utils/workshopHandoff";
import { originOfRecipeSet, originOfTool, type OriginInfo } from "../utils/modelOrigin";
import { RECIPES, chainableOps } from "../utils/workbenchPlan";
import "../assets/ai-tool.css";

const route = useRoute();
const router = useRouter();

/** 进入 ② 工作台（独立视图：三条路线 + 前后对比 + 批量出片） */
function goWorkbench() {
  router.push("/workshop/workbench");
}

/**
 * 当前工具是否有对应的链上算子（决定「在创意组合中打开」是否显示）。
 * 判据与工作台候选表同源（`chainableOps`）：图像压缩 / 文字提取这类
 * 产物不是图像链一环的工具不显示入口，而不是显示后让用户扑空。
 */
const chainOpId = computed(() => {
  const op = activeDef.value.op;
  return op && chainableOps().some((o) => o.id === op) ? op : "";
});

/** 带素材跳工作台：落地创意组合，链里只预置当前工具对应的算子 */
function openInChain() {
  const target = workbenchRouteTarget(tray.value, { engine: "chain", op: chainOpId.value });
  if (target) router.push(target);
}

type ToolId = WorkshopToolId;

/** 工具 id → 面板组件（唯一分发表，取代 12 分支 v-else-if 链） */
const TOOL_COMPONENTS: Record<string, unknown> = {
  recipe: RecipeTool,
  compress: CompressTool,
  journal: JournalTool,
  beauty: BeautyTool,
  style_transfer: StyleTransferTool,
  upscale: UpscaleTool,
  smart_edit: SmartEditTool,
  film_sim: FilmSimTool,
  ocr: OcrTool,
  color_transfer: ColorTransferTool,
  restore: RestoreTool,
  auto_grade: AutoGradeTool,
  local_beauty: LocalBeautyTool,
  face_reshape: LocalReshapeTool,
};

const primary = primaryTool();
const grouped = singleToolsByGroup();
const singleOpen = ref(false);

/* ---------------- 工具状态 / 素材托盘 ---------------- */

/** 工具选择与素材托盘提在 store：离开页面再回来不重置，任务状态也不丢 */
const ui = useWorkshopUiStore();
const active = computed<ToolId>({
  get: () => ui.active,
  set: (v) => {
    ui.active = v;
  },
});
const tray = computed<string[]>({
  get: () => ui.tray,
  set: (v) => {
    ui.tray = v;
  },
});
/** AI 模型设置弹窗 */
const settingsOpen = ref(false);

/* 左侧运行中圆点：catalog id 与 Python 作业名同域，不再需要映射表 */
const compressTask = useCompressTaskStore();
const aiStore = useAiStudioStore();
function toolBusy(id: ToolId): boolean {
  if (id === "compress") return compressTask.running;
  const jobId = aiStore.activeJobId(id);
  if (!jobId) return false;
  const j = aiStore.jobs[jobId];
  return !!j && (j.status === "queued" || j.status === "running");
}

const activeDef = computed<CatalogToolDef>(
  () => toolById(active.value) ?? primary,
);
const activeComponent = computed(() => TOOL_COMPONENTS[active.value] ?? null);

/**
 * 工具来源（FEAT-199）：本机算法 / 本机模型 / 云端 API / 未配置模型。
 *
 * 判据全在 `utils/modelOrigin.ts`：执行方式取 catalog，模型位置取已绑定供应商的
 * baseUrl。**不能**直接用 `activeDef.backend` —— 那个字段只说明「跑本机算子还是
 * 调模型」，既分不出「零模型算法」与「本地 ONNX 模型」，也会把绑到本机 Ollama
 * 的云端工具误报成「云端 API」（照片其实没出本机）。
 */
const activeOrigin = computed<OriginInfo>(() => originOfTool(active.value, aiStore.settings));

/** 主入口「一键出片」背后是四条配方，来源要按配方集合判（人像配方要本地模型） */
const primaryOrigin = computed<OriginInfo>(() =>
  originOfRecipeSet(
    RECIPES.map((r) => r.id),
    aiStore.settings,
  ),
);

/** 左栏每一项的来源角标；主入口也走同一函数，不另写一份判定 */
function originTag(toolId: string): OriginInfo {
  return originOfTool(toolId, aiStore.settings);
}

const headerMeta = computed(
  () =>
    `${activeDef.value.label} · 素材 ${tray.value.length} 张 · ${activeOrigin.value.label}`,
);

/* ---------------- 入口：从相册/灯箱带照片进来 ---------------- */

/**
 * 兼容两种带入方式（拼/拆统一走 `utils/workshopHandoff`，入口侧不再各写一份）：
 * - `?photos=a|b|c`：单张与多选统一用这个键（FEAT-187）
 * - `?photo=<绝对路径>`：旧链接兜底（FEAT-150 曾用），保留以免历史书签失效
 * 带照片直接落在 ①一键出片 —— 「打开即用」，不让用户再看一遍侧栏。
 */
onMounted(() => {
  const q = route.query;
  const legacyOne = typeof q.photo === "string" ? q.photo : "";
  const incoming = dedupePhotoPaths([legacyOne, ...splitPhotoPaths(q.photos)]);
  if (incoming.length) {
    tray.value = [...new Set([...tray.value, ...incoming])];
    active.value = primary.id;
  }
  // `?tool=` 优先于默认工位（校验合法性，避免深链把界面切到不存在的工具）
  if (typeof q.tool === "string" && isWorkshopTool(q.tool)) active.value = q.tool;
  // 来源标注要读已绑定的供应商地址（FEAT-199）：不主动拉一次，用户没进过
  // 「模型设置」时所有云端工具会一直停在「配置未载入」的兜底文案。
  // 失败静默：标注只是提示，不该因读设置失败而挡住工坊。
  void aiStore.loadSettings().catch(() => undefined);
});

async function addPhotos() {
  const list = await pickPhotos();
  if (list.length) tray.value = [...new Set([...tray.value, ...list])];
}

function removePhoto(path: string) {
  tray.value = tray.value.filter((p) => p !== path);
}

function clearTray() {
  tray.value = [];
}

function selectTool(t: CatalogToolDef) {
  active.value = t.id;
}
</script>

<template>
  <div class="ws-page">
    <AppHeader title="创意工坊" icon="🎨" :meta="headerMeta" />

    <div class="ws-layout">
      <!-- 左栏：三层 IA。分区是展示口径，工具集合仍是一张 catalog 表 -->
      <aside class="ws-side" aria-label="功能">
        <div class="side-group">
          <h3 class="side-h">快速出图</h3>
          <button
            class="side-item"
            :class="{ on: active === primary.id }"
            type="button"
            :title="primary.desc"
            @click="selectTool(primary)"
          >
            <span class="side-ico"><ToolIcon :name="primary.icon" :size="15" /></span>
            <span class="side-name">{{ primary.label }}</span>
            <span v-if="toolBusy(primary.id)" class="side-dot" title="任务运行中" aria-label="任务运行中" />
            <OriginTag v-else :info="primaryOrigin" variant="short" no-title />
          </button>
          <button
            class="side-item"
            type="button"
            title="工作台模式：带入当前素材，逐图对比、批量出片"
            @click="goWorkbench"
          >
            <span class="side-ico"><ToolIcon name="upscale" :size="15" /></span>
            <span class="side-name">工作台</span>
            <span class="side-arrow" aria-hidden="true">↗</span>
          </button>
        </div>

        <div class="side-group">
          <button class="side-toggle" type="button" :aria-expanded="singleOpen" @click="singleOpen = !singleOpen">
            <h3 class="side-h">单步工具</h3>
            <span class="side-chevron" :class="{ open: singleOpen }" aria-hidden="true">›</span>
          </button>

          <template v-if="singleOpen">
            <!-- 按类型分组（FEAT-169）：用户想的是「我要改什么」，不是「跑在哪」 -->
            <template v-for="g in grouped" :key="g.id">
              <h4 class="side-sub" :title="g.desc">{{ g.label }}</h4>
              <button
                v-for="t in g.items"
                :key="t.id"
                class="side-item"
                :class="{ on: active === t.id }"
                type="button"
                :title="t.ready ? t.desc : `${t.desc}（规划中）`"
                @click="selectTool(t)"
              >
                <span class="side-ico"><ToolIcon :name="t.icon" :size="15" /></span>
                <span class="side-name">{{ t.label }}</span>
                <span v-if="toolBusy(t.id)" class="side-dot" title="任务运行中" aria-label="任务运行中" />
                <span v-else-if="!t.ready" class="side-tag">规划</span>
                <!-- 来源角标（FEAT-199）：本机算法 / 本机模型 / 云端 API / 未配置模型。
                     不再按 backend 一律写「模型」—— 那会把绑到本机 Ollama 的云端
                     工具也标成「调用你配置的模型」。悬停说明由 OriginTag 自带。 -->
                <OriginTag v-else :info="originTag(t.id)" />
              </button>
            </template>
          </template>
        </div>
      </aside>

      <!-- 右栏：当前工具的工作区 -->
      <div class="ws-main">
        <header class="main-head">
          <span class="main-ico"><ToolIcon :name="activeDef.icon" :size="18" /></span>
          <h2 class="main-title">{{ activeDef.label }}</h2>
          <span class="main-sub">{{ activeDef.desc }}</span>
          <button
            v-if="chainOpId"
            class="ai-ghost"
            type="button"
            style="margin-left: auto"
            title="带到工作台的创意组合：素材与本工具对应的组件已就位，可继续串其它组件"
            @click="openInChain"
          >
            在创意组合中打开 ↗
          </button>
          <button
            class="ai-ghost"
            type="button"
            :style="chainOpId ? '' : 'margin-left: auto'"
            @click="settingsOpen = true"
          >
            ⚙ 模型设置
          </button>
        </header>

        <!-- 来源行（FEAT-199）：切到哪个工具就说清「这一步跑在哪、花不花钱」。
             只在这一处渲染，不在 14 个面板组件里各加一遍 —— 面板只管自己的参数区。 -->
        <p class="origin-row" :class="activeOrigin.origin">
          <OriginTag :info="activeOrigin" no-title />
          <span class="origin-detail">{{ activeOrigin.detail }}</span>
        </p>

        <!-- 素材托盘：所有工具共用一份输入 -->
        <PhotoTray :paths="tray" @add="addPhotos" @remove="removePhoto" @clear="clearTray" />

        <!-- 由 catalog 分发表驱动，取代 12 分支 v-else-if -->
        <component :is="activeComponent" v-if="activeComponent" :paths="tray" />

        <!-- 规划中：不假装能跑，把"将来会怎么用"写清楚 -->
        <section v-else-if="!activeDef.ready" class="planned">
          <p class="planned-title">「{{ activeDef.label }}」还在做，当前版本不可用。</p>
          <ol v-if="activeDef.steps?.length" class="steps">
            <template v-for="(s, i) in activeDef.steps" :key="s">
              <li class="step">
                <span class="step-n">{{ i + 1 }}</span>
                <span class="step-t">{{ s }}</span>
              </li>
              <li v-if="i < (activeDef.steps?.length ?? 0) - 1" class="step-arrow" aria-hidden="true">›</li>
            </template>
          </ol>
          <p v-if="activeDef.note" class="planned-note">{{ activeDef.note }}</p>
          <p class="planned-note dim">功能上线后，这里会直接在同一个素材托盘上工作，不需要重新选照片。</p>
        </section>

        <section v-else class="planned">
          <p class="planned-title">「{{ activeDef.label }}」暂未接入面板。</p>
          <p class="planned-note dim">工具已在清单中登记（{{ TOOLS.length }} 项），面板组件待补齐。</p>
        </section>

        <p class="colophon">
          这里只接 LR 导出的 JPG/PNG：本机步骤免费离线、照片不上传；模型步骤会把所选照片发到你配置的服务
          （可用本机 Ollama / LM Studio，也可用云端 API）。产物只写进你自己选的文件夹，原图不动。
        </p>
        <p class="colophon lock-note">
          <span class="lock-h">形状不变</span>
          影调与滤镜类工具只改颜色与质感，不改任何物体的形状：本机走逐色查表天然几何恒等；
          模型步骤会额外做结构校验，一旦检出脸型或构图被改就告警并回退。
        </p>
      </div>
    </div>

    <AiSettingsDialog v-model="settingsOpen" />
  </div>
</template>

<style scoped>
/* 页面壳：布局与相册页 / 时间线页一致 —— 左栏导航 + 右栏内容，
   左右留白用全站 --gutter（= --sp-8 = 52px） */
.ws-page {
  min-height: 100vh;
  padding-bottom: 48px;
}

.ws-layout {
  display: grid;
  grid-template-columns: 176px minmax(0, 1fr);
  gap: 30px;
  padding: 16px var(--gutter) 0;
  align-items: start;
}

/* 左栏吸顶：工具切换始终可点，不用先滚回顶部（与相册页筛选栏同一取值的理由） */
.ws-side {
  position: sticky;
  top: 70px;
  padding-top: 2px;
}

.side-group + .side-group {
  margin-top: 18px;
}

/* ③ 单步工具默认折叠：一个折叠开关，不是再加一层分组壳 */
.side-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 0;
  border: none;
  background: none;
  cursor: pointer;
  text-align: left;
}
.side-toggle .side-h {
  margin-bottom: 0;
}
.side-chevron {
  margin-left: auto;
  font-size: var(--fs-body);
  color: var(--fg-2);
  transition: transform var(--dur-2) var(--ease-out);
}
.side-chevron.open {
  transform: rotate(90deg);
}
.side-sub {
  margin: 10px 0 4px 9px;
  font-size: var(--fs-label);
  letter-spacing: 0.1em;
  font-weight: 600;
  color: var(--fg-3, var(--fg-2));
}
.side-arrow {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}

/* 分组小标：与全站小标同一字阶（9.5px / .15em / 600 / --fg-2） */
.side-h {
  margin: 0 0 6px;
  font-size: 9.5px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
}

/* 工具项：整行可点。选中 = 左侧 2px 金条 + 文字提亮，底只给 --hover ——
   不用整块高亮：一栏 4 项每项一块底，会变成"一堆小卡片"，读不出层级。
   行高 27px / 金条上下内缩 7px 与相册页 `.side-item` 取同一组值 ——
   两页的左栏在同一屏里切换时不能差 3px，否则会被读成"两个组件"。 */
.side-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  height: 27px;
  padding: 0 7px 0 9px;
  border: none;
  border-radius: 5px;
  background: none;
  font-size: 12.5px;
  color: var(--fg-2);
  text-align: left;
  cursor: pointer;
  transition:
    color var(--dur-2) var(--ease-out),
    background var(--dur-2) var(--ease-out);
}

.side-item:hover {
  color: var(--fg);
  background: var(--hover);
}

.side-item.on {
  color: var(--fg);
  font-weight: 600;
  background: var(--hover);
}

.side-item.on::before {
  content: "";
  position: absolute;
  left: 0;
  top: 7px;
  bottom: 7px;
  width: 2px;
  border-radius: 2px;
  background: var(--c-accent);
}

.side-ico {
  display: grid;
  place-items: center;
  width: 15px;
  height: 15px;
  flex-shrink: 0;
}

.side-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 规划中标记：弱色一行小字，不做成彩色徽标 —— 它只是"暂时还不能用"，
   不是这一栏里最重要的信息。字阶与相册页的计数 `.side-n` 一致。
   注：不再在栏底写「N / M 个可用」—— 每一项自己已经标了「规划」，
   再汇总一次等于同一件事说两遍，还占掉左栏最显眼的位置。 */
.side-tag {
  font-size: 10.5px;
  color: var(--fg-3, var(--fg-2));
  font-variant-numeric: tabular-nums;
}

/* 运行中圆点：轻微脉动，只提示"这个工具还有后台任务在跑"，不抢工具名的注意力 */
.side-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--c-accent);
  flex-shrink: 0;
  animation: side-dot-pulse 1.6s ease-out infinite;
}

@keyframes side-dot-pulse {
  0% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--c-accent) 55%, transparent);
  }
  70% {
    box-shadow: 0 0 0 5px transparent;
  }
  100% {
    box-shadow: 0 0 0 0 transparent;
  }
}

/* ---------- 右栏 ---------- */
.ws-main {
  min-width: 0;
}

.main-head {
  display: flex;
  align-items: center;
  gap: 10px;
  /* 与相册页 `.main-head` 同一间距 */
  margin-bottom: 10px;
}

.main-ico {
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  border-radius: 8px;
  color: var(--c-accent);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  flex-shrink: 0;
}

.main-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  color: var(--fg);
  flex-shrink: 0;
}

.main-sub {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
  min-width: 0;
}

/* 来源行（FEAT-199）：一枚档位角标 + 一句说明。
   不做成卡片或彩色条 —— 它是「切到这个工具时顺手确认一句」的信息，
   与页脚旁注同级；用分隔线压住一段空间即可。 */
.origin-row {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin: 0 0 12px;
  padding: 0 0 10px;
  border-bottom: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
  line-height: 1.5;
}

/* 档位角标在这里承担「首要信息」，给一点字重；颜色仍由 OriginTag 决定 */
.origin-row :deep(.origin-tag) {
  font-weight: 600;
  flex: none;
}

.origin-detail {
  min-width: 0;
  color: var(--fg-2);
  text-wrap: pretty;
}

/* 未配置模型：这一行是「现在点了会报错」的预告，底色点到为止 */
.origin-row.unconfigured {
  border-bottom-color: color-mix(in srgb, var(--c-danger) 35%, var(--c-rule));
}

/* ---------- 规划中 ---------- */
.planned {
  padding: 18px 20px;
  border-radius: var(--r-ui);
  border: 1px dashed var(--c-rule);
  background: var(--hover, rgba(120, 120, 130, 0.05));
}

.planned-title {
  margin: 0 0 10px;
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  color: var(--fg);
}

/* 流程条：横排四步（右栏宽度够，横读成一句"选图 › 抽主题 › 套版 › 出图"） */
.steps {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  margin: 0 0 12px;
  padding: 0;
  list-style: none;
}

.step {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 11px 0 7px;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  font-size: var(--fs-meta);
  color: var(--fg);
}

.step-n {
  display: grid;
  place-items: center;
  width: 15px;
  height: 15px;
  border-radius: 50%;
  font-size: 9.5px;
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 16%, transparent);
  font-variant-numeric: tabular-nums;
}

/* 箭头单独成项：放进 pill 里会把圆角切开 */
.step-arrow {
  margin: 0 -2px;
  font-size: var(--fs-body);
  color: var(--fg-2);
  list-style: none;
}

.planned-note {
  margin: 0 0 6px;
  font-size: var(--fs-body);
  line-height: 1.6;
  color: var(--fg-2);
}

.dim {
  opacity: 0.82;
}

/* 页脚旁注：这页工具的承诺，写在最下方一句带过 */
.colophon {
  margin: 22px 0 0;
  padding-top: 14px;
  border-top: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.65;
}

/* 第二条旁注接着排，不再重复一条分隔线（同一段承诺的两句） */
.colophon + .colophon {
  margin-top: 8px;
  padding-top: 0;
  border-top: none;
}

/* 「形状不变」这句是硬承诺，不是补充说明：用小标 + 提亮一词点出来，
   不做成彩色徽标 —— 它不该抢走工具列表的注意力。 */
.lock-h {
  margin-right: 7px;
  font-weight: 600;
  color: var(--fg);
}

.lock-h::after {
  content: "·";
  margin-left: 7px;
  color: var(--fg-3, var(--fg-2));
}

/* ---------- 窄窗：左栏折成横向工具条 ---------- */
@media (max-width: 900px) {
  .ws-layout {
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
    padding: 12px var(--sp-5) 0;
  }
  .ws-side {
    position: static;
    display: flex;
    align-items: center;
    gap: 6px;
    overflow-x: auto;
    padding-bottom: 2px;
    scrollbar-width: none;
  }
  .ws-side::-webkit-scrollbar {
    display: none;
  }
  .side-group,
  .side-group + .side-group {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 0;
  }
  .side-h {
    display: none;
  }
  .side-item {
    width: auto;
    height: 28px;
    padding: 0 10px 0 8px;
    white-space: nowrap;
    box-shadow: inset 0 0 0 1px var(--c-rule);
  }
  /* 横向条里"左侧金条"读不出方向，改成底色 + 描边 */
  .side-item.on::before {
    display: none;
  }
  .side-item.on {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 50%, transparent);
  }
  .side-name {
    flex: 0 0 auto;
  }
  .main-sub {
    display: none;
  }
}
</style>
