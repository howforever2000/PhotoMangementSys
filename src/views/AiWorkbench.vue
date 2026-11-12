<script setup lang="ts">
/**
 * AI 修图工作台（独立视图）
 * ===================================================================
 * 创意工坊（`/workshop`）里点「进入工作台」到达本页。工坊保持原样不动；本页是
 * 一个**专门的批量修图空间**：导入一批 → 群图网格逐图状态 → 前后对比 → 批量套用
 * （复用智能修图 `smart_edit` 作业）→ 导出并自动导入「创意工坊」相册（映射）。
 *
 * 为什么不塞进工坊：工坊是「选工具做一次」的工具台；工作台是「盯着一批图的每张
 * 进度」的批处理界面，两者信息密度和交互节奏不同，分开更清楚。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import AppHeader from "../components/ui/AppHeader.vue";
import { useWorkbenchStore } from "../stores/workbench";
import { useWorkshopUiStore } from "../stores/workshopUi";
import { useAiStudioStore } from "../stores/aiStudio";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
import { useAiBilling } from "../composables/useAiBilling";
import { resolvePhotoSrc } from "../utils/photoSrc";

const route = useRoute();
const router = useRouter();
const wb = useWorkbenchStore();
const ui = useWorkshopUiStore();
const ai = useAiStudioStore();
const albumStore = useAlbumStore();
const contentStore = useContentStore();
const notify = useNotify();
const billing = useAiBilling();

/* ---------------- 缩略图地址解析（HEIC 走后端预览，其余直连） ---------------- */
const srcMap = ref<Record<string, string>>({});
async function ensureSrc(path: string) {
  if (!path || srcMap.value[path] !== undefined) return;
  try {
    const url = await resolvePhotoSrc(path);
    srcMap.value = { ...srcMap.value, [path]: url };
  } catch {
    srcMap.value = { ...srcMap.value, [path]: "" };
  }
}
const neededPaths = computed(() => {
  const s = new Set<string>();
  for (const it of wb.items) {
    s.add(it.path);
    if (it.outputPath) s.add(it.outputPath);
  }
  return [...s];
});
watch(neededPaths, (list) => list.forEach((p) => void ensureSrc(p)), { immediate: true });

/* ---------------- 当前对比项 + 滑块 ---------------- */
const currentPath = ref<string>("");
watch(
  () => wb.items,
  (list) => {
    if (!list.find((i) => i.path === currentPath.value)) {
      currentPath.value = list[0]?.path ?? "";
    }
  },
  { immediate: true, deep: true },
);
const current = computed(() => wb.items.find((i) => i.path === currentPath.value) ?? null);
const beforeSrc = computed(() => (current.value ? srcMap.value[current.value.path] ?? "" : ""));
const afterSrc = computed(() =>
  current.value?.outputPath ? srcMap.value[current.value.outputPath] ?? "" : "",
);
const slider = ref(52);

/* ---------------- 入口：带入照片 ---------------- */
onMounted(() => {
  const q = route.query;
  const one = typeof q.photo === "string" ? q.photo : "";
  const many = typeof q.photos === "string" ? q.photos.split("|") : [];
  const incoming = [one, ...many].map((p) => p.trim()).filter(Boolean);
  if (incoming.length) wb.addPaths(incoming);
});

/* ---------------- 导入 ---------------- */
const IMAGE_FILTER = [
  { name: "图片", extensions: ["jpg", "jpeg", "png", "webp", "bmp", "gif", "heic", "heif"] },
];

async function addPhotos() {
  const picked = await openDialog({
    multiple: true,
    directory: false,
    title: "选择照片（可多选）",
    filters: IMAGE_FILTER,
  });
  if (!picked) return;
  const list = Array.isArray(picked) ? picked : [picked];
  wb.addPaths(list.filter((p): p is string => typeof p === "string"));
}

/** 导入文件夹：借扫描命令拿到目录内全部图片路径（递归） */
async function importFolder() {
  const dir = await openDialog({ directory: true, multiple: false, title: "选择要导入的文件夹" });
  if (!dir || typeof dir !== "string") return;
  try {
    const rows = await invoke<{ path: string }[]>("scan_album_photos", { path: dir });
    wb.addPaths(rows.map((r) => r.path).filter(Boolean));
  } catch (e) {
    notify.error("导入失败", String(e));
  }
}

/** 载入工坊里的素材托盘（跨页复用同一批照片） */
function loadFromWorkshop() {
  if (!ui.tray.length) {
    notify.info("工坊素材是空的", "先到创意工坊里添加照片，再回这里载入");
    return;
  }
  wb.addPaths(ui.tray);
}

async function pickDir(target: "output" | "album") {
  const dir = await openDialog({ directory: true, multiple: false, title: "选择目录" });
  if (!dir || typeof dir !== "string") return;
  if (target === "output") wb.outputDir = dir;
  else wb.albumDir = dir;
}

/* ---------------- 花费预估（理解 + 编辑，两能力位分别计价） ---------------- */
const visionPrice = computed(() => billing.modelPrice("vision"));
const editPrice = computed(() => billing.modelPrice("edit"));
const targetCount = computed(() => (wb.selectedCount > 0 ? wb.selectedCount : wb.total));
function expectedCalls() {
  const v = 1 + (wb.reflect ? wb.rounds : 0);
  const e = 1 + (wb.reflect ? Math.max(0, wb.rounds - 1) : 0);
  return { v, e };
}
const estimateText = computed(() => {
  const n = targetCount.value;
  const { v, e } = expectedCalls();
  if (visionPrice.value <= 0 && editPrice.value <= 0)
    return "未配置模型单价，无法预估（在创意工坊 › 模型设置 › 计费里填写）";
  const total = (v * visionPrice.value + e * editPrice.value) * n;
  return `≈ ${billing.formatMoney(total)}（${n} 张 × [理解 ${v} + 编辑 ${e}]）`;
});

/* ---------------- 批量套用（复用 smart_edit 作业） ---------------- */
const job = computed(() => ai.jobOfTool("smart_edit"));
const busy = computed(() => wb.busy);

async function applyBatch() {
  if (busy.value) return;
  const target = wb.selectedCount > 0 ? wb.items.filter((i) => wb.selected.includes(i.path)) : wb.items;
  if (!target.length) {
    notify.warning("还没有照片", "先导入一批照片再批量套用");
    return;
  }
  if (!wb.outputDir) {
    notify.warning("还没有输出目录", "先选择修图结果要放在哪个文件夹");
    return;
  }
  const paths = target.map((i) => i.path);
  const ok = await notify.confirm(
    `批量修图 ${paths.length} 张？`,
    `${estimateText.value}；产物写到 ${wb.outputDir}，同名以 {原名}_edit 保存，原图不动。`,
    { confirmText: "开始修图", type: "primary" },
  );
  if (!ok) return;
  wb.runningBatch = paths;
  for (const p of paths) wb.patch(p, { status: "queued", progress: 0, error: undefined });
  try {
    await ai.run({
      tool: "smart_edit",
      paths,
      prompt: wb.prompt,
      outputDir: wb.outputDir,
      options: { reflect: wb.reflect, rounds: wb.rounds },
    });
  } catch (e) {
    for (const p of paths) wb.patch(p, { status: "failed", error: String(e) });
    wb.runningBatch = [];
    notify.error("提交失败", String(e));
  }
}

function onCancel() {
  const id = ai.activeJobId("smart_edit");
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
      for (const p of batch) {
        wb.patch(p, { status: "failed", error: j.error || "已取消" });
      }
      wb.runningBatch = [];
    }
  },
);

/** 批量完成后：提示 + 自动导入「创意工坊」相册（映射到 albumDir） */
async function afterBatchDone(count: number) {
  wb.runningBatch = [];
  notify.success("修图完成", `已处理 ${count} 张，产物在 ${wb.outputDir}`);
  if (!wb.autoImport || !wb.albumDir) return;
  try {
    const albumId = await ensureAlbum();
    // 让相册把新落盘的照片入库（VCR 未启动时会失败，不阻断：打开相册仍可见文件）
    try {
      await contentStore.scanAlbumContent(albumId);
    } catch {
      /* 入库失败降级：文件已在映射目录，打开相册可再扫 */
    }
    await albumStore.fetchAlbums();
    notify.success("已自动导入『创意工坊』相册", "打开相册即可看到本次成品");
  } catch (e) {
    notify.warning("产物已写入目录，但相册映射失败", String(e));
  }
}

/** 确保「创意工坊」相册存在并映射到 albumDir，返回相册 id */
async function ensureAlbum(): Promise<number> {
  const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase();
  const albums = await albumStore.fetchAlbums();
  const hit = albums.find((a) => norm(a.path) === norm(wb.albumDir));
  if (hit) return hit.id;
  const created = await albumStore.createAlbum({
    name: wb.albumName || "创意工坊",
    path: wb.albumDir,
    description: "AI 修图工作台产物自动导入",
  });
  return created.id;
}

/* ---------------- 视图辅助 ---------------- */
function badgeText(status: string, progress: number): string {
  switch (status) {
    case "queued":
      return "排队";
    case "running":
      return `处理中 ${progress}%`;
    case "done":
      return "完成";
    case "failed":
      return "失败";
    default:
      return "待处理";
  }
}

function setCurrent(path: string) {
  currentPath.value = path;
}
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
    </div>

    <div class="wb-layout">
      <!-- 左栏：工作集 / 导入 / 输出 -->
      <aside class="wb-left">
        <div class="ws-head">
          <span class="ws-num">{{ wb.total }}</span>
          <span class="ws-unit">张 · 本次工作集</span>
        </div>

        <div class="imp">
          <button class="imp-btn primary" type="button" @click="addPhotos">＋ 添加照片</button>
          <button class="imp-btn" type="button" @click="importFolder">⤓ 导入文件夹</button>
          <button class="imp-btn" type="button" @click="loadFromWorkshop">⧉ 载入工坊素材</button>
        </div>

        <div class="side-group">
          <h3 class="side-h">输出</h3>
          <label class="row-check">
            <input v-model="wb.autoImport" type="checkbox" />
            自动导入「创意工坊」相册
          </label>
          <p class="side-note">成品写入映射目录后自动入库到相册，原图不动。</p>
          <div class="dir-row">
            <span class="dir-label">映射目录</span>
            <button class="dir-btn" type="button" title="选择映射目录" @click="pickDir('album')">
              {{ wb.albumDir || "未选择" }}
            </button>
          </div>
          <div class="dir-row">
            <span class="dir-label">导出目录</span>
            <button class="dir-btn" type="button" title="选择导出目录" @click="pickDir('output')">
              {{ wb.outputDir || "未选择" }}
            </button>
          </div>
        </div>
      </aside>

      <!-- 中栏：对比 + 群图网格 -->
      <section class="wb-main">
        <div class="main-head">
          <h2 class="main-title">工作集</h2>
          <span class="main-counts">
            <span>已处理 {{ wb.doneCount }}</span>
            <span>· 进行中 {{ wb.runningCount }}</span>
            <span v-if="wb.failedCount" class="bad">· 失败 {{ wb.failedCount }}</span>
          </span>
        </div>

        <!-- 前后对比 -->
        <div v-if="current" class="cmp">
          <div class="cmp-frame" :class="{ empty: !afterSrc }">
            <img v-if="beforeSrc" class="cmp-before" :src="beforeSrc" alt="原图" />
            <span v-else class="cmp-ph" />
            <img
              v-if="afterSrc"
              class="cmp-after"
              :src="afterSrc"
              alt="效果"
              :style="{ clipPath: `inset(0 0 0 ${slider}%)` }"
            />
            <span v-if="afterSrc" class="cmp-div" :style="{ left: `${slider}%` }">
              <span class="cmp-knob">↔</span>
            </span>
            <span class="cmp-tag l">原图</span>
            <span v-if="afterSrc" class="cmp-tag r">效果</span>
            <span v-else class="cmp-empty-hint">尚未修图 · 批量套用后可在此前后对比</span>
            <input v-if="afterSrc" v-model.number="slider" class="cmp-range" type="range" min="0" max="100" />
          </div>
          <div class="cmp-cap">{{ current.name }}</div>
        </div>

        <!-- 群图网格 -->
        <div v-if="wb.total" class="grid">
          <div
            v-for="it in wb.items"
            :key="it.path"
            class="tile"
            :class="{ on: it.path === currentPath }"
            @click="setCurrent(it.path)"
          >
            <div class="tile-thumb">
              <img v-if="srcMap[it.path]" :src="srcMap[it.path]" :alt="it.name" loading="lazy" />
              <span v-else class="tile-ph" />
              <button
                class="tile-check"
                :class="{ on: wb.selected.includes(it.path) }"
                type="button"
                @click.stop="wb.toggleSelect(it.path)"
              >
                ✓
              </button>
              <span class="tile-badge" :class="it.status">{{ badgeText(it.status, it.progress) }}</span>
              <div v-if="it.status === 'running' || it.status === 'queued'" class="tile-prog">
                <i :style="{ width: `${it.progress}%` }" />
              </div>
            </div>
            <div class="tile-name" :title="it.path">{{ it.name }}</div>
          </div>
        </div>
        <div v-else class="empty-tray">
          <p>工作集还是空的。</p>
          <p class="dim">从左侧「添加照片 / 导入文件夹 / 载入工坊素材」放一批照片进来。</p>
        </div>
      </section>

      <!-- 右栏：参数 + 批量套用 -->
      <aside class="wb-right">
        <section class="panel">
          <h3 class="panel-h">修图意图</h3>
          <textarea
            v-model="wb.prompt"
            class="intent"
            rows="4"
            placeholder="用一句话描述想要的统一风格，例如：把整组照片调成温暖通透的胶片感，皮肤自然通透一点，天空压暗但保留层次。"
          />
          <div class="chips">
            <button class="chip" type="button" @click="wb.prompt = '温暖通透的胶片感，皮肤自然通透一点，天空压暗但保留层次。'">胶片暖调</button>
            <button class="chip" type="button" @click="wb.prompt = '通透的人像肤色，柔和补光，背景轻微虚化。'">通透人像</button>
            <button class="chip" type="button" @click="wb.prompt = '夜景降噪、提亮暗部，保留灯光层次与色彩。'">夜景降噪</button>
            <button class="chip" type="button" @click="wb.prompt = '空气感通透风景，增强层次与色彩但不失真。'">通透风景</button>
          </div>

          <div class="field">
            <div class="field-head"><span>强度</span><span class="field-val">{{ wb.strength }}%</span></div>
            <input v-model.number="wb.strength" class="strength" type="range" min="0" max="100" />
          </div>

          <label class="row-check">
            <input v-model="wb.reflect" type="checkbox" />
            反思复核（多轮自检）
          </label>
          <div v-if="wb.reflect" class="seg">
            <button
              v-for="r in [1, 2, 3, 4]"
              :key="r"
              type="button"
              class="seg-btn"
              :class="{ on: wb.rounds === r }"
              @click="wb.rounds = r"
            >
              {{ r }} 轮
            </button>
          </div>
        </section>

        <section class="panel">
          <h3 class="panel-h">导出</h3>
          <dl class="spec">
            <div><dt>格式</dt><dd>JPG · 品质高</dd></div>
            <div><dt>尺寸</dt><dd>保持原图</dd></div>
            <div><dt>命名</dt><dd>{原名}_edit</dd></div>
            <div><dt>自动导入</dt><dd>{{ wb.autoImport ? "「创意工坊」相册" : "关闭" }}</dd></div>
          </dl>
          <div class="cost">{{ estimateText }}</div>
          <button class="apply" type="button" :disabled="busy || !wb.total" @click="applyBatch">
            {{ busy ? "处理中…" : `批量套用到 ${targetCount} 张` }}
          </button>
          <button v-if="busy" class="cancel" type="button" @click="onCancel">取消</button>
        </section>
      </aside>
    </div>

    <!-- 底部浮动批量条 -->
    <div v-if="wb.selectedCount" class="batchbar">
      <span class="bb-n">{{ wb.selectedCount }} 张已选</span>
      <span class="bb-est">预估 {{ estimateText }}</span>
      <span class="bb-tip">复用意图「{{ wb.prompt ? wb.prompt.slice(0, 12) + (wb.prompt.length > 12 ? "…" : "") : "未填写" }}」</span>
      <button class="bb-btn" type="button" @click="wb.clearSelection">取消选择</button>
      <button class="bb-apply" type="button" :disabled="busy" @click="applyBatch">批量套用</button>
    </div>
  </div>
</template>

<style scoped>
.wb-page {
  min-height: 100vh;
  padding-bottom: 96px;
}

.wb-crumb {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px var(--gutter) 0;
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

.wb-layout {
  display: grid;
  grid-template-columns: 232px minmax(0, 1fr) 300px;
  gap: 22px;
  padding: 14px var(--gutter) 0;
  align-items: start;
}

/* ---------- 左栏 ---------- */
.wb-left {
  position: sticky;
  top: 70px;
}
.ws-head {
  display: flex;
  align-items: baseline;
  gap: 6px;
  margin-bottom: 12px;
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

.imp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 20px;
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

.side-group {
  border-top: 1px solid var(--c-rule);
  padding-top: 14px;
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
  margin: 0 0 10px;
  font-size: var(--fs-meta);
  color: var(--fg-3, var(--fg-2));
  line-height: 1.55;
}

.row-check {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  font-size: var(--fs-body);
  color: var(--fg);
  cursor: pointer;
}
.row-check input {
  accent-color: var(--c-accent);
  width: 14px;
  height: 14px;
}

.dir-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 8px;
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

/* ---------- 中栏 ---------- */
.wb-main {
  min-width: 0;
}
.main-head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 12px;
}
.main-title {
  margin: 0;
  font-size: var(--fs-head);
  font-weight: var(--fw-strong);
}
.main-counts {
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.main-counts .bad {
  color: var(--c-danger);
}

.cmp {
  margin-bottom: 18px;
}
.cmp-frame {
  position: relative;
  width: 100%;
  aspect-ratio: 16 / 9;
  border-radius: var(--r-ui);
  overflow: hidden;
  background: var(--hover, rgba(120, 120, 130, 0.08));
  box-shadow: inset 0 0 0 1px var(--c-rule);
}
.cmp-before,
.cmp-after {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.cmp-ph {
  position: absolute;
  inset: 0;
  background: var(--hover, rgba(120, 120, 130, 0.08));
}
.cmp-div {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 2px;
  background: rgba(255, 255, 255, 0.85);
  transform: translateX(-1px);
}
.cmp-knob {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 30px;
  height: 30px;
  border-radius: 50%;
  background: rgba(20, 18, 26, 0.8);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.4);
  display: grid;
  place-items: center;
  font-size: 12px;
  color: #fff;
}
.cmp-tag {
  position: absolute;
  top: 10px;
  padding: 2px 8px;
  border-radius: var(--r-pill);
  font-size: 10px;
  background: rgba(10, 9, 13, 0.6);
  color: var(--fg);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.12);
}
.cmp-tag.l {
  left: 10px;
}
.cmp-tag.r {
  right: 10px;
}
.cmp-empty-hint {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}
.cmp-range {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 8px;
  width: calc(100% - 24px);
  margin: 0 12px;
  accent-color: var(--c-accent);
}
.cmp-cap {
  margin-top: 8px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
  gap: 12px;
}
.tile {
  cursor: pointer;
}
.tile-thumb {
  position: relative;
  aspect-ratio: 4 / 3;
  border-radius: var(--r-cover);
  overflow: hidden;
  background: var(--hover, rgba(120, 120, 130, 0.1));
  box-shadow: inset 0 0 0 1px var(--c-rule);
}
.tile.on .tile-thumb {
  box-shadow: inset 0 0 0 2px var(--c-accent);
}
.tile-thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.tile-ph {
  position: absolute;
  inset: 0;
  background: var(--hover, rgba(120, 120, 130, 0.1));
}
.tile-check {
  position: absolute;
  top: 6px;
  left: 6px;
  width: 18px;
  height: 18px;
  border-radius: 5px;
  border: 1px solid rgba(255, 255, 255, 0.5);
  background: rgba(10, 9, 13, 0.55);
  color: transparent;
  font-size: 11px;
  display: grid;
  place-items: center;
  cursor: pointer;
  padding: 0;
}
.tile-check.on {
  background: var(--c-accent);
  color: #1a1608;
  border-color: var(--c-accent);
}
.tile-badge {
  position: absolute;
  top: 6px;
  right: 6px;
  padding: 1px 7px;
  border-radius: var(--r-pill);
  font-size: 9.5px;
  background: rgba(10, 9, 13, 0.66);
  color: var(--fg-2);
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1);
}
.tile-badge.done {
  color: #8fd39a;
}
.tile-badge.failed {
  color: var(--c-danger);
}
.tile-badge.running,
.tile-badge.queued {
  color: var(--c-accent);
}
.tile-prog {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 3px;
  background: rgba(0, 0, 0, 0.4);
}
.tile-prog i {
  display: block;
  height: 100%;
  background: var(--c-accent);
  transition: width var(--dur-2, 0.2s) linear;
}
.tile-name {
  margin-top: 5px;
  font-size: 10.5px;
  color: var(--fg-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.empty-tray {
  padding: 40px 20px;
  border: 1px dashed var(--c-rule);
  border-radius: var(--r-ui);
  text-align: center;
  color: var(--fg-2);
  font-size: var(--fs-body);
}
.empty-tray .dim {
  opacity: 0.8;
  font-size: var(--fs-meta);
}

/* ---------- 右栏 ---------- */
.wb-right {
  position: sticky;
  top: 70px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.panel {
  border: 1px solid var(--c-rule);
  border-radius: var(--r-ui);
  padding: 14px;
  background: rgba(255, 255, 255, 0.012);
}
.panel-h {
  margin: 0 0 10px;
  font-size: var(--fs-label);
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  color: var(--fg-2);
}
.intent {
  width: 100%;
  resize: vertical;
  min-height: 72px;
  border-radius: 8px;
  border: 1px solid var(--c-rule);
  background: rgba(0, 0, 0, 0.25);
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
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 45%, transparent);
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
  border: none;
  box-shadow: inset 0 0 0 1px var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: 11px;
  cursor: pointer;
}
.seg-btn.on {
  color: var(--fg);
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c-accent) 55%, transparent);
}

.spec {
  margin: 0 0 10px;
}
.spec > div {
  display: flex;
  justify-content: space-between;
  padding: 5px 0;
  border-bottom: 1px solid var(--c-rule);
  font-size: var(--fs-meta);
}
.spec > div:last-child {
  border-bottom: none;
}
.spec dt {
  color: var(--fg-2);
}
.spec dd {
  margin: 0;
  color: var(--fg);
}
.cost {
  margin: 8px 0 10px;
  font-size: var(--fs-meta);
  color: var(--fg-2);
  line-height: 1.5;
}
.apply {
  width: 100%;
  height: 38px;
  border-radius: var(--r-pill);
  border: none;
  background: var(--c-accent);
  color: #1a1608;
  font-size: var(--fs-body);
  font-weight: 600;
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
  border-radius: var(--r-pill);
  border: 1px solid var(--c-rule);
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}

/* ---------- 底部批量条 ---------- */
.batchbar {
  position: fixed;
  left: 50%;
  bottom: 22px;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 10px 14px 10px 18px;
  border-radius: var(--r-pill);
  background: rgba(24, 21, 30, 0.92);
  box-shadow: inset 0 0 0 1px var(--c-rule), 0 18px 40px -18px rgba(0, 0, 0, 0.8);
  font-size: var(--fs-meta);
  z-index: var(--z-sticky, 40);
}
.bb-n {
  color: var(--fg);
  font-weight: 600;
}
.bb-est {
  color: var(--c-accent);
}
.bb-tip {
  color: var(--fg-2);
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.bb-btn {
  border: none;
  background: none;
  color: var(--fg-2);
  font-size: var(--fs-meta);
  cursor: pointer;
}
.bb-apply {
  height: 30px;
  padding: 0 16px;
  border-radius: var(--r-pill);
  border: none;
  background: var(--c-accent);
  color: #1a1608;
  font-weight: 600;
  font-size: var(--fs-meta);
  cursor: pointer;
}
.bb-apply:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

@media (max-width: 1180px) {
  .wb-layout {
    grid-template-columns: 200px minmax(0, 1fr);
  }
  .wb-right {
    grid-column: 1 / -1;
    position: static;
  }
}
@media (max-width: 860px) {
  .wb-layout {
    grid-template-columns: minmax(0, 1fr);
    padding: 12px var(--sp-5) 0;
  }
  .wb-left {
    position: static;
  }
}
</style>
