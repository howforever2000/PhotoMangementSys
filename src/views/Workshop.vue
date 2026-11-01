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
 * ## 结构（排版对应"工具怎么被调用"）
 *
 * 页面上是按**调用方式**分的两区，而不是四张长得一样的卡：
 *
 * - **创作**：从一组照片生成一件作品。低频、有输出物、耗时长 → 大卡 +
 *   流程条（选图 › 抽主题 › 套版 › 出图），进入后是分步界面。
 * - **处理**：对单张/一组做变换。高频、参数少、反复进 → 行式清单，
 *   三枚 pill 直接写清「输入 · 输出 · 跑在哪」，用户据此判断敢不敢按。
 *
 * 两区共用同一个**素材托盘**：这四件事的输入都是"一组照片"，
 * 所以输入不该由每个工具自己再选一遍（`PhotoTray.vue` 顶部有详细理由）。
 */
import { computed, onMounted, ref } from "vue";
import { useRoute } from "vue-router";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import AppHeader from "../components/ui/AppHeader.vue";
import PhotoTray from "../components/workshop/PhotoTray.vue";
import CompressTool from "../components/workshop/CompressTool.vue";
import RecipeCleanup from "../components/workshop/RecipeCleanup.vue";
import ToolIcon from "../components/workshop/ToolIcon.vue";

const route = useRoute();

type ToolId = "compress" | "ocr" | "enhance" | "journal";

interface ToolDef {
  id: ToolId;
  title: string;
  desc: string;
  icon: "compress" | "text" | "enhance" | "journal";
  /** 本轮是否真的能用。规划中的项不做假入口（点了必须有事发生，或明确说不能点） */
  ready: boolean;
  /** 加工区三枚 pill 的内容：输入 / 输出 / 依赖 */
  input: string;
  output: string;
  dep: string;
}

/** 处理类工具：行式清单。顺序按"最常用"排，压缩排第一（它是唯一已可用的） */
const processTools: ToolDef[] = [
  {
    id: "compress",
    title: "图像压缩",
    desc: "体积瘦身：指定目标大小、长边上限与居中裁切，输出 JPEG 副本，原图不动",
    icon: "compress",
    ready: true,
    input: "照片",
    output: "JPEG 副本",
    dep: "本机",
  },
  {
    id: "ocr",
    title: "文字提取",
    desc: "识别照片里的文字，可复制、可加入搜索词条（复用扫描入库已下载的 OCR 检测模型）",
    icon: "text",
    ready: false,
    input: "照片",
    output: "文本",
    dep: "需模型",
  },
  {
    id: "enhance",
    title: "画质增强",
    desc: "低分辨率照片放大 2×/4×，老照片去噪与轻度修复；输出新文件，不覆盖原图",
    icon: "enhance",
    ready: false,
    input: "照片",
    output: "新文件",
    dep: "需模型",
  },
];

/** 创作类工具：大卡 + 流程条。本轮只有一张卡（拼贴/海报等后续再加） */
const creativeTool = {
  id: "journal" as ToolId,
  title: "主题手账",
  desc: "从一组照片里提炼主题，排成一本手账：选图、抽主题、套版、出图",
  steps: ["选图 8–20 张", "抽主题", "套版", "长图 / 多页"],
  /** 实现路线（写在这里，免得下一轮接手的人重新想一遍）：
   *  选图 = 语义索引 + 人脸聚合 + 去重；抽主题 = CLIP 语义分类 + 时间/地点聚合；
   *  套版 = 确定性模板排版（工具已有 justifiedRows 排布算法）。
   *  生成模型只在"单张插画化点缀"这一档出场 —— 版式与文字交给模型糊得厉害。 */
  note: "选图与主题抽取用本机已有能力，后续由 agent 编排执行",
};

/* ---------------- 工具状态 / 素材托盘 ---------------- */

const active = ref<ToolId | null>(null);
const tray = ref<string[]>([]);

const activeDef = computed<ToolDef | null>(() => {
  if (!active.value) return null;
  if (active.value === "journal") {
    return {
      id: "journal",
      title: creativeTool.title,
      desc: creativeTool.desc,
      icon: "journal",
      ready: false,
      input: "一组照片",
      output: "手账长图",
      dep: "本机",
    };
  }
  return processTools.find((t) => t.id === active.value) ?? null;
});

/** 工具总数（处理 3 + 创作 1）与已可用数（本轮只有压缩） */
const totalTools = processTools.length + 1;
const readyCount = computed(() => processTools.filter((t) => t.ready).length);

const headerMeta = computed(() => {
  if (activeDef.value) {
    return `${activeDef.value.title} · 素材 ${tray.value.length} 张`;
  }
  return `${totalTools} 个工具 · ${readyCount.value} 个已可用 · 全部在本机完成`;
});

/* ---------------- 入口：从相册/灯箱带照片进来 ---------------- */

/**
 * 兼容两种带入方式：
 * - `?photo=<绝对路径>`：灯箱/右键菜单「在创意工坊处理」
 * - `?photos=a|b|c`：批量入口（多选用 `|` 分隔，避免逗号与 Windows 路径冲突）
 * 带了照片就直接进压缩 —— 「打开即用」，不让用户再看一遍列表。
 */
onMounted(() => {
  const q = route.query;
  const one = typeof q.photo === "string" ? q.photo : "";
  const many = typeof q.photos === "string" ? q.photos.split("|") : [];
  const incoming = [one, ...many].map((p) => p.trim()).filter(Boolean);
  if (incoming.length) tray.value = dedupe([...tray.value, ...incoming]);

  // 带 tool 就直开那个工位（可以没有照片 —— 进去再添加）；
  // 只带 photo/photos 时默认落在压缩（唯一可用的工位，“打开即用”）。
  const tool = typeof q.tool === "string" ? (q.tool as ToolId) : "";
  if (tool) active.value = tool;
  else if (incoming.length) active.value = "compress";
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

function openTool(t: ToolDef) {
  if (!t.ready) return;
  active.value = t.id;
}

function closeTool() {
  active.value = null;
}
</script>

<template>
  <div class="ws-page">
    <AppHeader title="创意工坊" icon="🎨" :meta="headerMeta" />

    <div class="ws-body">
      <!-- ==================== 工具列表 ==================== -->
      <template v-if="!active">
        <!-- 创作区：从一组照片生成一件作品。低频 / 有输出物 → 大卡 + 流程条 -->
        <section class="sect">
          <div class="sect-head">
            <span class="sect-label">创作</span>
            <span class="sect-note">从一组照片生成一件作品</span>
            <span class="sect-line" />
          </div>

          <article class="creative" aria-disabled="true">
            <div class="cc-main">
              <div class="cc-top">
                <span class="cc-icon"><ToolIcon name="journal" :size="22" /></span>
                <h2 class="cc-title">{{ creativeTool.title }}</h2>
                <span class="chip chip-off">规划中</span>
              </div>
              <p class="cc-desc">{{ creativeTool.desc }}</p>
              <p class="cc-note">{{ creativeTool.note }}</p>
            </div>

            <!-- 流程条：竖着排的管道。大卡必须回答"作品是怎么造出来的"，
                 否则它和下面那些行式工具没有区别，只是被放大了一号。 -->
            <ol class="cc-flow">
              <template v-for="(s, i) in creativeTool.steps" :key="s">
                <li class="cc-step">
                  <span class="cc-step-n">{{ i + 1 }}</span>
                  <span class="cc-step-t">{{ s }}</span>
                </li>
                <li v-if="i < creativeTool.steps.length - 1" class="cc-arrow" aria-hidden="true">
                  ↓
                </li>
              </template>
            </ol>
          </article>
        </section>

        <!-- 处理区：对单张/一组做变换。高频 / 参数少 → 行式清单 -->
        <section class="sect">
          <div class="sect-head">
            <span class="sect-label">处理</span>
            <span class="sect-note">对单张或一组照片做加工，输出副本</span>
            <span class="sect-line" />
          </div>

          <div class="rows">
            <button
              v-for="t in processTools"
              :key="t.id"
              class="row"
              :class="{ off: !t.ready }"
              type="button"
              :disabled="!t.ready"
              :title="t.ready ? `打开${t.title}` : '规划中，暂不可用'"
              @click="openTool(t)"
            >
              <span class="row-icon"><ToolIcon :name="t.icon" :size="22" /></span>

              <span class="row-main">
                <span class="row-title">
                  {{ t.title }}
                  <span class="chip" :class="t.ready ? 'chip-on' : 'chip-off'">
                    {{ t.ready ? "可用" : "规划中" }}
                  </span>
                </span>
                <span class="row-desc">{{ t.desc }}</span>
              </span>

              <span class="row-pills">
                <span class="pill">输入 {{ t.input }}</span>
                <span class="pill">输出 {{ t.output }}</span>
                <span class="pill">{{ t.dep }}</span>
              </span>

              <span class="row-go" aria-hidden="true">{{ t.ready ? "→" : "·" }}</span>
            </button>
          </div>
        </section>

        <!-- 维护区：旧调色配方清理（低频、破坏性 → 摆在最后、默认不显眼） -->
        <section class="sect">
          <div class="sect-head">
            <span class="sect-label">维护</span>
            <span class="sect-line" />
          </div>
          <RecipeCleanup />
        </section>

        <!-- 页脚旁注：这页的四个工具有一个共同承诺，写在最下方一句带过；
             顺便把短页面的底部空白收住，不至于让内容飘在半空 -->
        <p class="colophon">
          压缩、识别、增强都在本机跑：照片不上传，产物只写到你自己选的文件夹。
          调色仍交给 Lightroom / Photoshop —— 这里只做相册管理顺手的加工。
        </p>
      </template>

      <!-- ==================== 工具工作区 ==================== -->
      <template v-else-if="activeDef">
        <div class="back-row">
          <button class="back" type="button" @click="closeTool">← 创意工坊</button>
        </div>

        <header class="tool-head">
          <span class="tool-icon"><ToolIcon :name="activeDef.icon" :size="18" /></span>
          <h1 class="tool-title">{{ activeDef.title }}</h1>
          <span class="tool-desc">{{ activeDef.desc }}</span>
        </header>

        <!-- 素材托盘：所有工具共用一份输入 -->
        <PhotoTray
          :paths="tray"
          @add="addPhotos"
          @remove="removePhoto"
          @clear="clearTray"
        />

        <!-- 已实现：压缩 -->
        <CompressTool v-if="active === 'compress'" :paths="tray" />

        <!-- 规划中：不假装能跑，只把"将来会怎么用"说清楚 -->
        <div v-else class="planned">
          <p class="planned-title">「{{ activeDef.title }}」还在做，当前版本不可用。</p>
          <p class="planned-body">
            {{ activeDef.desc }}
          </p>
          <p class="planned-body dim">
            计划：{{ activeDef.input }} → {{ activeDef.output }}（{{ activeDef.dep }}）。
            功能上线后，这里会直接在同一个素材托盘上工作，不需要重新选照片。
          </p>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
/* 页面壳：工具台两侧留白与全站一致；宽屏下用 1080px 锁定阅读列，
   避免参数行被拉成一条横跨整屏的散板 */
.ws-page {
  min-height: 100vh;
  padding-bottom: 56px;
}

.ws-body {
  max-width: 1080px;
  margin: 0 auto;
  padding: 18px var(--sp-8) 0;
}

/* ---------- 分区标 ---------- */
.sect {
  margin-bottom: 26px;
}

/* 标签 + 注释相邻、发丝线填空白：读成一个短句（"处理 — 对单张或一组做加工"）。
   注释若被推到行尾，1080px 的空白会把两者割成两个不相干的元素。 */
.sect-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}

/* 小标：与顶栏读数、参数名同一字阶（--fs-label + 字距），全页只有一种小标语言 */
.sect-label {
  font-size: var(--fs-label);
  letter-spacing: 0.14em;
  color: var(--fg-2);
  flex-shrink: 0;
}

/* 发丝线把标签和注释分开：这一区从这里开始，到页右缘结束 */
/* 发丝线只负责把这一区的"起点"托到页右缘，不承载信息 */
.sect-line {
  flex: 1;
  height: 1px;
  background: var(--c-rule);
}

.sect-note {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  flex-shrink: 0;
}

/* ---------- 创作卡 ---------- */
.creative {
  display: grid;
  /* 左文右管：文字侧吃掉剩余宽度，管道列固定（步骤名长度可控） */
  grid-template-columns: minmax(0, 1fr) 190px;
  gap: 20px;
  padding: 18px 20px;
  border-radius: var(--r-ui);
  /* 虚线 = 占位/未上线（与托盘"＋添加"框同一语义），实线才是已可用的板 */
  border: 1px dashed var(--c-rule);
  background: var(--hover, rgba(120, 120, 130, 0.05));
}

.cc-main {
  min-width: 0;
  align-self: center;
}

.cc-top {
  display: flex;
  align-items: center;
  gap: 10px;
}

.cc-icon {
  display: grid;
  place-items: center;
  width: 34px;
  height: 34px;
  border-radius: 9px;
  color: var(--c-accent);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  flex-shrink: 0;
}

.cc-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  letter-spacing: -0.01em;
  color: var(--fg);
}

.cc-desc {
  margin: 10px 0 8px;
  font-size: var(--fs-body);
  line-height: 1.6;
  color: var(--fg-2);
}

/* 流程：竖排管道（↓ 连接）。比横排更能说"一步接一步"，
   也把大卡右半边那片空白填上。
   align-items: flex-start —— 不加的话 flex-column 会把每个 pill 拉伸成整列宽的条 */
.cc-flow {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  justify-content: center;
  gap: 1px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.cc-step {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 24px;
  padding: 0 11px 0 7px;
  border-radius: var(--r-pill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  font-size: var(--fs-meta);
  color: var(--fg);
}

/* 流程箭头单独成项：放进 pill 里会把圆角切开 */
.cc-arrow {
  height: 10px;
  line-height: 10px;
  padding-left: 17px;
  font-size: 9px;
  color: var(--fg-2);
  list-style: none;
}

.cc-step-n {
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

.cc-note {
  margin: 12px 0 0;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}

/* ---------- 处理行清单 ---------- */
.rows {
  border-top: 1px solid var(--c-rule);
}

.row {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr) auto 18px;
  align-items: center;
  gap: 14px;
  width: 100%;
  /* 行高 72px：够放两行文字 + 一排 pill，又能在首屏竖着看完全部三行 */
  min-height: 72px;
  padding: 12px 6px;
  border: none;
  border-bottom: 1px solid var(--c-rule);
  background: none;
  text-align: left;
  cursor: pointer;
  transition: background var(--dur-2) var(--ease-out);
}

.row:hover:not(.off) {
  background: var(--hover, rgba(120, 120, 130, 0.07));
}

/* 规划中：整行压暗但仍读得清（用户需要知道"有这块功能"，不是"这块坏了"） */
.row.off {
  cursor: not-allowed;
}

.row.off .row-icon,
.row.off .row-title,
.row.off .row-desc {
  opacity: 0.62;
}

.row-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  border-radius: 10px;
  color: var(--fg);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  transition: color var(--dur-2) var(--ease-out), box-shadow var(--dur-2) var(--ease-out);
}

.row:hover:not(.off) .row-icon {
  color: var(--c-accent);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}

.row-main {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.row-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  color: var(--fg);
}

.row-desc {
  font-size: var(--fs-body);
  line-height: 1.45;
  color: var(--fg-2);
}

/* 三枚 pill 固定顺序：输入 · 输出 · 依赖。
   这三条是"敢不敢按"的判断依据，比形容词有用 */
.row-pills {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: nowrap;
}

.pill {
  padding: 3px 9px;
  border-radius: var(--r-pill);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  white-space: nowrap;
}

.row-go {
  font-size: var(--fs-body);
  color: var(--fg-2);
  text-align: right;
  transition: transform var(--dur-2) var(--ease-out), color var(--dur-2) var(--ease-out);
}

.row:hover:not(.off) .row-go {
  color: var(--c-accent);
  transform: translateX(2px);
}

/* ---------- 状态徽标：两态，不用第三态 ---------- */
.chip {
  padding: 1px 7px;
  border-radius: var(--r-pill);
  font-size: 10px;
  font-weight: var(--fw-em);
  letter-spacing: 0.02em;
  white-space: nowrap;
}

.chip-on {
  color: var(--c-accent);
  background: color-mix(in srgb, var(--c-accent) 15%, transparent);
}

.chip-off {
  color: var(--fg-2);
  box-shadow: inset 0 0 0 1px var(--c-rule);
}

/* ---------- 工作区 ---------- */
.back-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}

.back {
  padding: 3px 10px;
  border-radius: var(--r-pill);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  background: transparent;
  border: none;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), box-shadow var(--dur-2) var(--ease-out);
}
.back:hover {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
}

.tool-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 14px;
}

.tool-icon {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border-radius: 8px;
  color: var(--c-accent);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  flex-shrink: 0;
}

.tool-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
  color: var(--fg);
  flex-shrink: 0;
}

.tool-desc {
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
  min-width: 0;
}

/* 规划中工具的说明块：一块虚线板，把"现在不能做什么/将来怎么做"写在里面 */
.planned {
  padding: 18px 20px;
  border-radius: var(--r-ui);
  border: 1px dashed var(--c-rule);
  background: var(--hover, rgba(120, 120, 130, 0.05));
}

.planned-title {
  margin: 0 0 8px;
  font-size: var(--fs-title);
  font-weight: var(--fw-em);
  color: var(--fg);
}

.planned-body {
  margin: 0 0 6px;
  font-size: var(--fs-body);
  line-height: 1.6;
  color: var(--fg-2);
}

.dim {
  opacity: 0.82;
}

/* ---------- 窄窗 ---------- */
@media (max-width: 900px) {
  .ws-body {
    padding: 16px var(--sp-5) 0;
  }
  /* pill 行在窄屏会让标题挤成一列竖排字；直接不显示，信息仍在 desc 里 */
  .row-pills {
    display: none;
  }
  .row {
    grid-template-columns: 40px minmax(0, 1fr) 16px;
  }
  /* 创作卡的两列折成一列：190px 的管道列在窄屏会把文字挤成窄条 */
  .creative {
    grid-template-columns: minmax(0, 1fr);
    gap: 14px;
  }
  .cc-flow {
    flex-direction: row;
    flex-wrap: wrap;
  }
  .cc-arrow {
    height: auto;
    padding: 0 2px;
  }
}

/* 页脚旁注 */
.colophon {
  margin: 2px 0 0;
  padding-top: 14px;
  border-top: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.65;
}
</style>
