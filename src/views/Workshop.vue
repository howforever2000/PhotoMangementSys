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
import { computed, onMounted, ref, watch } from "vue";
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

/* ---------------- 单步工具：分组手风琴 + 搜索（BUG-2026-1122-009） ---------------- */

/**
 * 展开的分组 id（一次只展开一组）。
 *
 * 为什么一次只开一组：14 个工具平铺在一栏里是 **17 个可读项**（含 5 个分组头），
 * 远超人一次能扫读的量（工作记忆 4±1）。改成手风琴后，
 * 任一决策点上的可见选项回到「1 个分组 + ≤4 个工具」。
 *
 * 一次只开一组（而不是允许多组同开）是刻意的：多组同开等于回到平铺，
 * 只是多了几道折叠的仪式。
 */
const openGroup = ref<string>("");

/** 工具搜索框的内容（空 = 不筛选） */
const toolQuery = ref("");

/** 当前激活工具所属的分组：切工具时自动展开它，用户不用先找 */
function ensureGroupVisible(groupId: string) {
  if (groupId) openGroup.value = groupId;
}

/**
 * 按搜索词过滤后的分组列表。
 * 搜到工具名、描述或分组名都算命中 —— 分组名命中时整组都留下，
 * 否则搜「人像」会把「人像」组里的工具全滤掉，用户以为没搜到。
 */
const filteredGroups = computed(() => {
  const q = toolQuery.value.trim().toLowerCase();
  if (!q) return grouped;
  const out: typeof grouped = [];
  for (const g of grouped) {
    const groupHit = g.label.toLowerCase().includes(q);
    const items = groupHit
      ? g.items
      : g.items.filter(
          (t) =>
            t.label.toLowerCase().includes(q) ||
            t.desc.toLowerCase().includes(q) ||
            t.id.toLowerCase().includes(q),
        );
    if (items.length) out.push({ ...g, items });
  }
  return out;
});

/** 搜索时自动展开所有命中的分组（否则搜到了却看不见） */
watch(filteredGroups, (gs) => {
  if (!toolQuery.value.trim()) return;
  if (gs.length === 1) openGroup.value = gs[0].id;
});

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
 * 工具来源（FEAT-199）：本机算法 / 本机模型 / API 模型·本地 / API 模型·云端 / 未配置。
 *
 * 判据全在 `utils/modelOrigin.ts`：执行方式取 catalog，走供应商的按 baseUrl 主机名分流
 * （回环 → 本地，免费；其它 → 云端，计费）。**不能**直接用 `activeDef.backend` ——
 * 那个字段只说明「跑本机算子还是调模型」，分不出「零模型算法」与「本地 ONNX 模型」。
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
  // 切到某个工具时，把它所在的分组展开 —— 否则用户点了深链进来
  // 看到的是"选中了某个工具，但它在收起的组里"，等于没选。
  ensureGroupVisible(t.group);
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
            <span class="side-ico"><ToolIcon name="workbench" :size="15" /></span>
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
            <!--
              工具搜索（BUG-2026-1122-009）：14 个工具不再靠扫读。
              放在分组之上 —— 搜索是"跨分组找"的入口，位置必须在所有分组之上。
            -->
            <label class="side-search">
              <svg viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="7" cy="7" r="4.2" />
                <path d="M10 10l3.5 3.5" />
              </svg>
              <input
                v-model="toolQuery"
                type="search"
                placeholder="搜工具"
                aria-label="搜索单步工具"
                @keydown.esc="toolQuery = ''"
              />
            </label>

            <p v-if="toolQuery.trim() && !filteredGroups.length" class="side-empty">
              没有匹配「{{ toolQuery.trim() }}」的工具
            </p>

            <!--
              按类型分组（FEAT-169）：用户想的是「我要改什么」，不是「跑在哪」。
              改成手风琴（BUG-2026-1122-009）：一次只展开一组，
              任一决策点的可见选项回到 ≤4。
            -->
            <template v-for="g in filteredGroups" :key="g.id">
              <button
                class="side-sub side-sub-toggle"
                type="button"
                :aria-expanded="openGroup === g.id"
                :title="g.desc"
                @click="openGroup = openGroup === g.id ? '' : g.id"
              >
                <span>{{ g.label }}</span>
                <span class="side-sub-count">{{ g.items.length }}</span>
                <span class="side-chevron" :class="{ open: openGroup === g.id }" aria-hidden="true">›</span>
              </button>

              <div v-show="openGroup === g.id" class="side-sub-items">
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
                  <!-- 来源角标（FEAT-199）：本机算法 / 本机模型 / API 模型·本地 / API 模型·云端 / 未配置。
                       左栏窄，用 short 形态（本机 / 本地 API / 云端 API）；完整标签在标题行。
                       悬停说明由 OriginTag 自带。 -->
                  <OriginTag v-else :info="originTag(t.id)" variant="short" />
                </button>
              </div>
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

        <p class="colophon pm-reading-surface">
          这里只接 LR 导出的 JPG/PNG：本机步骤免费离线、照片不上传；模型步骤会把所选照片发到你配置的服务
          （可用本机 Ollama / LM Studio，也可用云端 API）。产物只写进你自己选的文件夹，原图不动。
        </p>
        <p class="colophon lock-note pm-reading-surface">
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

/* ③ 单步工具默认折叠：一个折叠开关，不是再加一层分组壳。
   高度显式给 26px：里面是 9.5px 的 h3，内容撑出来只有 16px —— 那不是可点的行，
   是个碰运气的小目标。
   position:relative 是必需的：.side-toggle 没有定位时，
   绝对定位的箭头会退回 .ws-side（sticky）当包含块，被推到容器右缘外 6px。 */
.side-toggle {
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  height: 26px;
  padding: 0;
  border: none;
  background: none;
  cursor: pointer;
  text-align: left;
}
.side-toggle .side-h {
  margin-bottom: 0;
  /* 标题吸收剩余空间，箭头贴右。
     用 flex:1（basis auto）而不是 1 1 0：flex-basis:0 时浏览器按
     min-content 兜底算最小尺寸，「单步工具」+ .15em 字距的 min-content
     仍比可用空间宽，于是标题占 166px，把 16px 的箭头挤出容器（实测溢出 6px）。 */
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.side-chevron {
  margin-left: auto;
  flex-shrink: 0;
  /* 固定宽度 + 文字居中：箭头是 4.3px 宽的字形，rotate(90deg) 绕自身原点转后
     变成 16px 宽，若按字形原点转就会向右溢出容器（实测 6px）。
     给一个 16px 的槽位并让字形居中，旋转后仍在槽内。 */
  width: 16px;
  text-align: center;
  font-size: var(--fs-body);
  line-height: 1;
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

/* 分组手风琴（BUG-2026-1122-009）：分组头从静态小标变成可点的开关。
   一次只展开一组 —— 任一决策点的可见选项回到 ≤4（原来是 17 个可读项）。 */
.side-sub-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  width: calc(100% - 9px);
  height: 26px;
  margin: 8px 0 0 9px;
  padding: 0 6px 0 0;
  border: none;
  border-radius: var(--r-cover);
  background: none;
  text-align: left;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), background var(--dur-2) var(--ease-out);
}
.side-sub-toggle:hover {
  color: var(--fg-2);
  background: var(--hover);
}
.side-sub-toggle .side-chevron {
  margin-left: 0;
  font-size: var(--fs-meta);
}
.side-sub-count {
  font-size: var(--fs-label);
  color: var(--fg-3, var(--fg-2));
  font-variant-numeric: tabular-nums;
}
/* 展开的分组：标题提亮，让"当前这一组是开着的"一眼可见 */
.side-sub-toggle[aria-expanded="true"] {
  color: var(--fg-2);
}
/* 组内工具：缩进一级，视觉上从属于分组头 */
.side-sub-items {
  margin: 3px 0 0;
}

/* 工具搜索框：跨分组找工具的入口，位置在所有分组之上。
   高度 28px：与 .side-item 同档，别做成一条缝。 */
.side-search {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  margin: 8px 0 2px;
  padding: 0 9px;
  border-radius: var(--r-ui);
  background: var(--slip-fill, rgba(255, 255, 255, 0.034));
  border: 1px solid var(--ctrl-line, rgba(255, 255, 255, 0.115));
  transition: border-color var(--dur-2) var(--ease-out);
}
.side-search:focus-within {
  border-color: color-mix(in srgb, var(--c-accent) 50%, transparent);
}
.side-search svg {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  stroke: var(--fg-3, var(--fg-2));
  fill: none;
  stroke-width: 1.5;
}
.side-search input {
  flex: 1;
  min-width: 0;
  height: 100%;
  padding: 0;
  border: none;
  outline: none;
  background: none;
  color: var(--fg);
  font: inherit;
  font-size: var(--fs-meta);
}
.side-search input::placeholder {
  color: var(--fg-3, var(--fg-2));
}
/* 搜索无结果时的空态：说清"没有"，并给一个退出的动作 */
.side-empty {
  margin: 10px 0 0 9px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
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

/* 工具项：整行可点。选中 = 文字提亮 + 底部 1.5px 强调下划线，底只给 --hover ——
   不用整块高亮：一栏 4 项每项一块底，会变成"一堆小卡片"，读不出层级。
   不用左侧 2px 竖条：那是「圆角盒子单侧边条」（craft-floor 绝对禁令）——
   border-radius 会把竖条两端切出弯钩，读起来像独立的小板块。
   下划线与相册页 `.side-item.on`、顶栏导航 `.mh-nav-item.on` 用同一套语言 ——
   三处左栏/导航在同一屏里切换时不能差 3px，否则会被读成"两个组件"。 */
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

.side-item.on::after {
  content: "";
  position: absolute;
  left: 9px;
  right: 7px;
  bottom: 2px;
  height: 1.5px;
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

/* 第二条旁注接着排，不再重复一条分隔线（同一段承诺的两句）。
   背景图模式下两条各自带承载面（.pm-reading-surface）：承载面本身就是分隔，
   所以原来的 border-top 分隔线要去掉（否则会叠在承载面上缘外侧，读作两条线），
   间距改用承载面之间的 8px 间隙；纯色模式下仍是原来那条连排的排版。 */
.colophon + .colophon {
  margin-top: 8px;
  padding-top: 0;
  border-top: none;
}
body.theme-on-image .colophon {
  /* 承载面自带内边距与圆角，上方的 border-top 分隔线要撤掉 */
  margin-top: 22px;
  padding-top: 10px;
  border-top: none;
}
body.theme-on-image .colophon + .colophon {
  margin-top: 8px;
  padding-top: 10px;
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
    flex-wrap: wrap;
    gap: 6px;
    overflow-x: auto;
    padding-bottom: 2px;
    /* 滚动条不再藏：原来 scrollbar-width:none + ::-webkit-scrollbar{display:none}
       把提示一起藏了 —— 390px 下 12/15 个工具在视口外，用户完全不知道右边还有。
       改成细滚动条（6px），横向溢出有形可见的提示。 */
    scrollbar-width: thin;
    scrollbar-color: rgba(255, 255, 255, 0.18) transparent;
  }
  .ws-side::-webkit-scrollbar {
    height: 6px;
  }
  .ws-side::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.18);
    border-radius: 3px;
  }
  .side-group,
  .side-group + .side-group {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 0;
  }
  /* 「单步工具」组内部要能换行：搜索框独占一行，分组与工具各自成行 */
  .side-group:last-child {
    flex: 1 0 100%;
    flex-wrap: wrap;
  }
  /* 搜索框独占一行（它是跨分组找工具的入口，不能挤在工具条里） */
  .side-search {
    flex: 1 0 100%;
    margin: 6px 0 0;
  }
  .side-empty {
    flex: 1 0 100%;
    margin: 6px 0 0;
  }
  /* 分组标题在窄窗里不再被压成 10.5px 宽的竖排（那是原来没处理 h4 的后果）：
     它现在是可点的开关，做成药丸，整行可点、不会挤成一列字。 */
  .side-sub-toggle {
    width: auto;
    margin: 6px 0 0;
    padding: 0 9px;
    height: 26px;
    border-radius: var(--r-ui);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--c-rule);
  }
  .side-sub-items {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 6px 0 0;
    flex-wrap: wrap;
  }
  .side-h {
    display: none;
  }
  /* 「单步工具」那个总开关：不能塌成 4×16px（原来 h3 隐藏后只剩一个箭头）。
     给它一个药丸形态，标题也留着 —— 窄窗下它就是那 5 个分组药丸的上一个入口。 */
  .side-toggle {
    width: auto;
    height: 26px;
    padding: 0 9px;
    border-radius: var(--r-ui);
    background: var(--hover);
    box-shadow: inset 0 0 0 1px var(--c-rule);
  }
  .side-toggle .side-h {
    display: block;
    margin: 0;
  }
  .side-item {
    width: auto;
    height: 28px;
    padding: 0 10px 0 8px;
    white-space: nowrap;
    box-shadow: inset 0 0 0 1px var(--c-rule);
  }
  /* 横向条里下划线贴在药丸内侧读不出来（药丸有描边，底部空间只有 1px），
     改成整圈强调色描边 —— 这是 craft-floor 允许的"full border"形态，
     而不是被禁的"单侧边条"。 */
  .side-item.on::after {
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
