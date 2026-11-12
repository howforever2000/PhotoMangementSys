<script setup lang="ts">
/**
 * 创意工坊（FEAT-150，替代 FEAT-063/099 的调色台）
 * ===================================================================
 * ## 这页是什么
 *
 * **对已有照片做加工的工具台**：图像压缩、文字提取（OCR）、画质增强，
 * 以及后续由 agent 编排的创作类工具（主题手账）。
 *
 * ## 为什么重做（上一版的定位错在哪）
 *
 * 上一版工坊是一台 Lightroom 式的非破坏调色台（曝光/曲线/裁剪/蒙版/锐化 +
 * OpenCV 算子）。对一款**相册管理**软件来说：
 *   ① 与 LR/PS 逐项重复，赢不了的战场（Rust 侧 333KB 编辑引擎 + 一套独立
 *      Python 微服务，见 README 定位四条：本地存储 / 离线 AI / 文件夹抽象 / 检索优先）；
 *   ② 那套 Python 算子通道在**发布版根本没打包**（release.tauri.conf.json 无
 *      studio 资源），装了也只有开发机能跑通 —— 一半功能在用户机器上是死的。
 * 所以整条编辑链路连同其后端一起删掉了（git 历史里可取回）。
 *
 * ## 结构：左栏选工具 + 右栏是该工具的工作区
 *
 * 沿用相册页 / 时间线页那套「左栏导航 + 右栏内容」的布局（AlbumList.vue 的
 * `.album-side`）：176px 左栏吸顶、分组小标 + 行式条目、选中用左侧 2px 金条。
 * 为什么不沿用上一版的"一屏卡片列表"：工具是**常驻切换**的对象（压完一批接着
 * 换 OCR），不是"进入再退出"的目的地 —— 侧栏让切换只花一次点击，而不是
 * 「返回列表 → 再进下一个」两步，也不需要"返回小组件"这类中间态。
 *
 * 左栏按**输入/输出的性质**分组：
 *   - 处理：对单张或一组照片做变换，输出副本。高频、参数少、反复用。
 *   - 创作：从一组照片生成一件作品（主题手账）。低频、有输出物、耗时长。
 *
 * 两区共用同一个**素材托盘**：这几件事的输入都是"一组照片"，
 * 所以输入不该由每个工具自己再选一遍（`PhotoTray.vue` 顶部有详细理由）。
 */
import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import AppHeader from "../components/ui/AppHeader.vue";
import PhotoTray from "../components/workshop/PhotoTray.vue";
import CompressTool from "../components/workshop/CompressTool.vue";
import ToolIcon from "../components/workshop/ToolIcon.vue";
import AiSettingsDialog from "../components/workshop/ai/AiSettingsDialog.vue";
import JournalTool from "../components/workshop/ai/JournalTool.vue";
import BeautyTool from "../components/workshop/ai/BeautyTool.vue";
import StyleTransferTool from "../components/workshop/ai/StyleTransferTool.vue";
import UpscaleTool from "../components/workshop/ai/UpscaleTool.vue";
import SmartEditTool from "../components/workshop/ai/SmartEditTool.vue";
import FilmSimTool from "../components/workshop/ai/FilmSimTool.vue";
import ColorTransferTool from "../components/workshop/ai/ColorTransferTool.vue";
import RestoreTool from "../components/workshop/ai/RestoreTool.vue";
import { useWorkshopUiStore, type WorkshopToolId } from "../stores/workshopUi";
import { useCompressTaskStore } from "../stores/compressTask";
import { useAiStudioStore } from "../stores/aiStudio";
import "../assets/ai-tool.css";

const route = useRoute();
const router = useRouter();

/** 进入独立的「AI 修图工作台」视图（群图批量修图；本页保持原样） */
function goWorkbench() {
  router.push("/workshop/workbench");
}

type ToolId = WorkshopToolId;

interface ToolDef {
  id: ToolId;
  title: string;
  /** 右栏标题下方的一句话（说清"做什么"，不重复侧栏的名字） */
  desc: string;
  icon: "compress" | "text" | "enhance" | "journal" | "beauty" | "style" | "upscale";
  /** 本轮是否真的能用。规划中的项不做假入口（点开必须明确说不能跑） */
  ready: boolean;
  /** 规划中工具的流程示意 */
  steps?: string[];
  /** 规划中工具的路线说明 */
  note?: string;
}

/** 处理类：对单张/一组做变换，输出副本 */
const processTools: ToolDef[] = [
  {
    id: "compress",
    title: "图像压缩",
    desc: "体积瘦身：指定目标大小、长边上限与居中裁切，输出 JPEG 副本，原图不动",
    icon: "compress",
    ready: true,
  },
  {
    id: "ocr",
    title: "文字提取",
    desc: "识别照片里的文字，可复制、可加入搜索词条",
    icon: "text",
    ready: false,
    note: "复用扫描入库已下载的 PaddleOCR 检测模型，补齐识别（rec）模型后即可上线。",
  },
];

/** 本地增强：纯本机推理（免费、离线），与「处理」类同属本机处理 */
const localTools: ToolDef[] = [
  {
    id: "local_film",
    title: "胶片模拟",
    desc: "内置多款胶片 LUT，一键套用，可加颗粒与暗角，输出新 JPG",
    icon: "style",
    ready: true,
  },
  {
    id: "local_ctransfer",
    title: "参考图调色",
    desc: "把参考图的色调与影调迁移到素材图，输出新 JPG",
    icon: "style",
    ready: true,
  },
  {
    id: "local_restore",
    title: "画质修复",
    desc: "Real-ESRGAN 2×/4× 超分 + 人脸修复，输出高清 JPG 副本",
    icon: "enhance",
    ready: true,
  },
];

/** AI 创作类：调用外部 OpenAI 兼容模型（理解 / 编辑 / 超分） */
const aiTools: ToolDef[] = [
  {
    id: "ai_journal",
    title: "主题手账",
    desc: "从一组照片提炼主题、写配文并做风格化，排成一本手账（1 张 JPG / 多张 PDF）",
    icon: "journal",
    ready: true,
  },
  {
    id: "ai_beauty",
    title: "人物美颜",
    desc: "预设美颜风格与级别，逐张人像精修，输出新 JPG",
    icon: "beauty",
    ready: true,
  },
  {
    id: "ai_style",
    title: "滤镜迁移",
    desc: "输入内容图与参考图，迁移色调与影调；也支持仅凭提示词自主调色",
    icon: "style",
    ready: true,
  },
  {
    id: "ai_upscale",
    title: "画质增强",
    desc: "低分辨率照片 2×/4× 超分放大，去噪与细节还原，输出新文件",
    icon: "upscale",
    ready: true,
  },
  {
    id: "ai_smart",
    title: "智能修图",
    desc: "一句话描述效果，AI 规划修图步骤并逐步执行；可开启多轮反思，只输出终图",
    icon: "enhance",
    ready: true,
  },
];

const allTools = computed<ToolDef[]>(() => [...processTools, ...localTools, ...aiTools]);
const isAiTool = (id: ToolId) => id.startsWith("ai_");

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

/* 左侧运行中圆点：哪个工具还有后台作业在跑（压缩 store + AI store 各自暴露状态） */
const compressTask = useCompressTaskStore();
const aiStore = useAiStudioStore();
const AI_TOOL_NAME: Record<string, string> = {
  ai_journal: "journal",
  ai_beauty: "beauty",
  ai_style: "style_transfer",
  ai_upscale: "upscale",
  ai_smart: "smart_edit",
  local_film: "film_sim",
  local_ctransfer: "color_transfer",
  local_restore: "restore",
};
function toolBusy(id: ToolId): boolean {
  if (id === "compress") return compressTask.running;
  const name = AI_TOOL_NAME[id];
  if (!name) return false;
  const jobId = aiStore.activeJobId(name);
  if (!jobId) return false;
  const j = aiStore.jobs[jobId];
  return !!j && (j.status === "queued" || j.status === "running");
}

const activeDef = computed<ToolDef>(
  () => allTools.value.find((t) => t.id === active.value) ?? processTools[0],
);

const headerMeta = computed(
  () =>
    `${activeDef.value.title} · 素材 ${tray.value.length} 张 · ${
      isAiTool(active.value) ? "AI 模型处理" : "本机处理"
    }`,
);

/* ---------------- 入口：从相册/灯箱带照片进来 ---------------- */

/**
 * 兼容两种带入方式：
 * - `?photo=<绝对路径>`：灯箱/右键菜单「在创意工坊处理」
 * - `?photos=a|b|c`：批量入口（多选用 `|` 分隔，避免与 Windows 路径里的字符冲突）
 * 带照片就直接落在压缩工位 —— 「打开即用」，不让用户再看一遍侧栏。
 */
onMounted(() => {
  const q = route.query;
  const one = typeof q.photo === "string" ? q.photo : "";
  const many = typeof q.photos === "string" ? q.photos.split("|") : [];
  const incoming = [one, ...many].map((p) => p.trim()).filter(Boolean);
  if (incoming.length) {
    tray.value = dedupe([...tray.value, ...incoming]);
    active.value = "compress";
  }
  // `?tool=` 优先于默认工位（也允许不带照片直接深链到某个工具）
  if (typeof q.tool === "string") active.value = q.tool as ToolId;
});

function dedupe(list: string[]): string[] {
  return [...new Set(list)];
}

async function addPhotos() {
  const picked = await openDialog({
    multiple: true,
    directory: false,
    title: "选择照片（可多选）",
    filters: [
      { name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif", "heic", "heif"] },
    ],
  });
  if (!picked) return;
  const list = Array.isArray(picked) ? picked : [picked];
  tray.value = dedupe([...tray.value, ...list.filter((p): p is string => typeof p === "string")]);
}

function removePhoto(path: string) {
  tray.value = tray.value.filter((p) => p !== path);
}

function clearTray() {
  tray.value = [];
}

function selectTool(t: ToolDef) {
  active.value = t.id;
}
</script>

<template>
  <div class="ws-page">
    <AppHeader title="创意工坊" icon="🎨" :meta="headerMeta" />

    <div class="ws-layout">
      <!-- 左栏：工具导航（同样是"本页内部的分区"，与相册页筛选栏同一套结构） -->
      <aside class="ws-side" aria-label="工具">
        <div class="side-group">
          <h3 class="side-h">处理</h3>
          <button
            v-for="t in processTools"
            :key="t.id"
            class="side-item"
            :class="{ on: active === t.id }"
            type="button"
            :title="t.ready ? t.desc : `${t.desc}（规划中）`"
            @click="selectTool(t)"
          >
            <span class="side-ico"><ToolIcon :name="t.icon" :size="15" /></span>
            <span class="side-name">{{ t.title }}</span>
            <span v-if="toolBusy(t.id)" class="side-dot" title="任务运行中" aria-label="任务运行中" />
            <span v-if="!t.ready" class="side-tag">规划</span>
          </button>
        </div>

        <div class="side-group">
          <h3 class="side-h">本地增强</h3>
          <button
            v-for="t in localTools"
            :key="t.id"
            class="side-item"
            :class="{ on: active === t.id }"
            type="button"
            :title="t.desc"
            @click="selectTool(t)"
          >
            <span class="side-ico"><ToolIcon :name="t.icon" :size="15" /></span>
            <span class="side-name">{{ t.title }}</span>
            <span v-if="toolBusy(t.id)" class="side-dot" title="任务运行中" aria-label="任务运行中" />
          </button>
        </div>

        <div class="side-group">
          <h3 class="side-h">AI 创作</h3>
          <button
            v-for="t in aiTools"
            :key="t.id"
            class="side-item"
            :class="{ on: active === t.id }"
            type="button"
            :title="t.desc"
            @click="selectTool(t)"
          >
            <span class="side-ico"><ToolIcon :name="t.icon" :size="15" /></span>
            <span class="side-name">{{ t.title }}</span>
            <span v-if="toolBusy(t.id)" class="side-dot" title="任务运行中" aria-label="任务运行中" />
          </button>
        </div>
      </aside>

      <!-- 右栏：当前工具的工作区 -->
      <div class="ws-main">
        <header class="main-head">
          <span class="main-ico"><ToolIcon :name="activeDef.icon" :size="18" /></span>
          <h2 class="main-title">{{ activeDef.title }}</h2>
          <span class="main-sub">{{ activeDef.desc }}</span>
          <button class="ai-ghost" type="button" style="margin-left: auto" @click="settingsOpen = true">
            ⚙ 模型设置
          </button>
        </header>

        <!-- 入口：AI 修图工作台（独立视图，群图批量修图） -->
        <section class="wb-entry">
          <span class="wb-entry-ico" aria-hidden="true">🖼</span>
          <div class="wb-entry-body">
            <span class="wb-entry-kicker">新 · 群图批量修图</span>
            <h3 class="wb-entry-title">AI 修图工作台</h3>
            <p class="wb-entry-desc">
              把一组照片一次导入，逐张套用同一种修图风格，前后对比着看，成品自动导入「创意工坊」相册，原图不动。
            </p>
          </div>
          <button class="wb-entry-btn" type="button" @click="goWorkbench">进入工作台 →</button>
        </section>

        <!-- 素材托盘：所有工具共用一份输入 -->
        <PhotoTray :paths="tray" @add="addPhotos" @remove="removePhoto" @clear="clearTray" />

        <!-- 已实现：压缩 -->
        <CompressTool v-if="active === 'compress'" :paths="tray" />

        <!-- AI 创作：调用外部 OpenAI 兼容模型 -->
        <JournalTool v-else-if="active === 'ai_journal'" :paths="tray" />
        <BeautyTool v-else-if="active === 'ai_beauty'" :paths="tray" />
        <StyleTransferTool v-else-if="active === 'ai_style'" :paths="tray" />
        <UpscaleTool v-else-if="active === 'ai_upscale'" :paths="tray" />
        <SmartEditTool v-else-if="active === 'ai_smart'" :paths="tray" />

        <!-- 本地增强：纯本机推理 -->
        <FilmSimTool v-else-if="active === 'local_film'" :paths="tray" />
        <ColorTransferTool v-else-if="active === 'local_ctransfer'" :paths="tray" />
        <RestoreTool v-else-if="active === 'local_restore'" :paths="tray" />

        <!-- 规划中：不假装能跑，把"将来会怎么用"写清楚 -->
        <section v-else class="planned">
          <p class="planned-title">「{{ activeDef.title }}」还在做，当前版本不可用。</p>

          <!-- 流程条：创作类必须回答"作品是怎么造出来的" -->
          <ol v-if="activeDef.steps?.length" class="steps">
            <template v-for="(s, i) in activeDef.steps" :key="s">
              <li class="step">
                <span class="step-n">{{ i + 1 }}</span>
                <span class="step-t">{{ s }}</span>
              </li>
              <li v-if="i < activeDef.steps.length - 1" class="step-arrow" aria-hidden="true">›</li>
            </template>
          </ol>

          <p v-if="activeDef.note" class="planned-note">{{ activeDef.note }}</p>
          <p class="planned-note dim">
            功能上线后，这里会直接在同一个素材托盘上工作，不需要重新选照片。
          </p>
        </section>

        <p class="colophon">
          「处理」类工具（图像压缩）全程在本机跑，照片不上传；「AI 创作」类会把所选照片发送到你配置的外部 API 服务，产物只写到你自己选的文件夹。
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

/* ---------- 入口：AI 修图工作台 ---------- */
.wb-entry {
  display: flex;
  align-items: center;
  gap: 14px;
  margin-bottom: 14px;
  padding: 14px 16px;
  border-radius: var(--r-ui);
  background:
    linear-gradient(180deg, color-mix(in srgb, var(--c-accent) 8%, transparent), transparent 70%),
    var(--hover, rgba(120, 120, 130, 0.05));
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 32%, var(--c-rule));
}

.wb-entry-ico {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 10px;
  font-size: 20px;
  color: var(--c-accent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 40%, transparent);
  flex-shrink: 0;
}

.wb-entry-body {
  min-width: 0;
  flex: 1;
}

.wb-entry-kicker {
  display: inline-block;
  font-size: 9.5px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--c-accent);
  font-weight: 600;
}

.wb-entry-title {
  margin: 2px 0 3px;
  font-size: var(--fs-title);
  font-weight: var(--fw-strong);
  color: var(--fg);
}

.wb-entry-desc {
  margin: 0;
  font-size: var(--fs-meta);
  line-height: 1.55;
  color: var(--fg-2);
}

.wb-entry-btn {
  flex-shrink: 0;
  height: 34px;
  padding: 0 16px;
  border-radius: var(--r-pill);
  border: none;
  background: var(--c-accent);
  color: #1a1608;
  font-size: var(--fs-body);
  font-weight: 600;
  cursor: pointer;
  transition: filter var(--dur-2) var(--ease-out);
}
.wb-entry-btn:hover {
  filter: brightness(1.06);
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
