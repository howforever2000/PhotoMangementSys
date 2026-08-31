<script setup lang="ts">
/**
 * 图片扫描测试页（大组件测试工具，不落库）
 *
 * 流程：
 *   1. 选择文件夹（open dialog）→ 扫描：提取每张直接图片的时间（三级兜底）+ GPS 坐标
 *   2. 「解析地名」：GPS 聚类 → 本地省/市点面判断（离线秒回；未命中才联网）
 *   3. 视图切换：按时间（年→月）/ 按地点 查看识别结果，验证准确率
 *   4. 「按年·地点组织移动」：创建 {dir}/{年份}/{地点}/ 两级文件夹并移动照片（破坏性，需确认）
 */
import { computed, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { TestPhoto, OrganizeReport, ScanProgress } from "../types/photo";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";

const router = useRouter();
const theme = useThemeStore();
const notify = useNotify();

/** 页面级主题变量：卡片/按钮原为固定白底，深色模式下会突兀发白 */
const tsVars = computed(() => {
  const dark = theme.isDark;
  return {
    "--ts-panel-bg": dark ? "rgba(255,255,255,.045)" : "#fff",
    "--ts-panel-border": dark ? "rgba(255,255,255,.1)" : "#e5e7eb",
    "--ts-btn-bg": dark ? "rgba(255,255,255,.06)" : "#fff",
    "--ts-btn-border": dark ? "rgba(255,255,255,.18)" : "#ddd",
    "--ts-btn-hover": dark ? "rgba(255,255,255,.12)" : "#f2f4f7",
    "--ts-text": dark ? "#f5f7ff" : "#2c3e50",
    "--ts-muted": dark ? "rgba(214,221,240,.6)" : "#888",
  };
});

/** 目标文件夹路径 */
const dirPath = ref("");
/** 是否递归扫描子目录（小组件功能，用户可选；后续全局扫描沿用同一开关） */
const recursive = ref(false);
/** 扫描结果 */
const photos = ref<TestPhoto[] | null>(null);
/** 视图模式：time=按时间（年→月） / place=按地点 */
const viewMode = ref<"time" | "place">("time");
/** 状态 */
const scanning = ref(false);
const resolving = ref(false);
const organizing = ref(false);
const error = ref("");
/** 移动报告 */
const report = ref<OrganizeReport | null>(null);
/** 进度（resolve/organize） */
const progress = ref<ScanProgress | null>(null);
let unlistenProgress: UnlistenFn | null = null;

/** 监听后端进度事件（resolve=解析地名 / organize=组织移动，逐张上报） */
listen<ScanProgress>("test-scan-progress", (e) => {
  progress.value = e.payload;
}).then((fn) => {
  unlistenProgress = fn;
});

onUnmounted(() => {
  unlistenProgress?.();
});

/** 选择文件夹 */
async function browseDir() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: recursive.value ? "选择要扫描的文件夹（递归子目录）" : "选择要扫描的文件夹（只扫直接图片，不递归子目录）",
  });
  if (typeof selected === "string") {
    dirPath.value = selected;
  }
}

/** 扫描：时间 + GPS 坐标（recursive 控制是否递归子目录） */
async function scanPhotos() {
  if (scanning.value || !dirPath.value) return;
  scanning.value = true;
  error.value = "";
  report.value = null;
  try {
    photos.value = await invoke<TestPhoto[]>("scan_test_photos", { path: dirPath.value, recurse: recursive.value });
    if (photos.value.length === 0) {
      error.value = recursive.value
        ? "扫描到 0 张图片（已递归子目录）。请确认所选文件夹及其子目录下含有图片。"
        : "扫描到 0 张直接图片。本功能只扫描所选文件夹下的直接图片（不递归子目录），请确认照片直接放在该文件夹中，或在下方勾选「递归子目录」。";
    }
  } catch (e) {
    error.value = `扫描失败：${e}`;
    photos.value = null;
  } finally {
    scanning.value = false;
  }
}

/** 解析地名：GPS 聚类 + 本地省/市优先（离线秒回，未命中联网兜底），逐张进度上报 */
async function resolvePlaces() {
  if (resolving.value || !dirPath.value) return;
  resolving.value = true;
  error.value = "";
  progress.value = null;
  try {
    photos.value = await invoke<TestPhoto[]>("resolve_test_places", { path: dirPath.value, recurse: recursive.value });
    // 完成：进度条置满
    if (photos.value) {
      const withGps = photos.value.filter((p) => p.lat !== null).length;
      progress.value = {
        phase: "resolve",
        current: withGps,
        total: withGps,
        file_name: "完成",
        message: "解析完成",
      };
    }
  } catch (e) {
    error.value = `地名解析失败：${e}`;
  } finally {
    resolving.value = false;
  }
}

/** 按年·地点组织移动（破坏性操作，需确认） */
async function organizePhotos() {
  if (organizing.value || !dirPath.value || !photos.value) return;
  const hasPlace = photos.value.some((p) => p.place);
  const ok = await notify.confirm(
    "组织移动照片",
    (hasPlace ? "" : "将自动解析照片地点（本地省/市离线查询，秒回；未命中才联网）。\n\n") +
      "按「年份/地点」创建两级文件夹并移动照片到其中。\n" +
      "移动后原目录中照片将消失（可在新文件夹找到）。\n" +
      "确认执行移动？",
    { type: "danger", confirmText: "确认移动" },
  );
  if (!ok) return;
  organizing.value = true;
  error.value = "";
  progress.value = null;
  try {
    report.value = await invoke<OrganizeReport>("organize_test_photos", { path: dirPath.value, recurse: recursive.value });
    // 完成：进度条置满（organize 阶段 total=全部照片）
    progress.value = {
      phase: "organize",
      current: report.value.total,
      total: report.value.total,
      file_name: "完成",
      message: "组织移动完成",
    };
  } catch (e) {
    error.value = `组织移动失败：${e}`;
  } finally {
    organizing.value = false;
  }
}

/** 年份提取（兜底：无 shoot_time 用 GPS 日期不可得时按 undefined） */
function yearOf(p: TestPhoto): string {
  return p.year ?? "未知年份";
}

/** 按时间分组：年 → 月 → 照片 */
interface MonthGroup {
  month: string;
  photos: TestPhoto[];
}
interface YearGroup {
  year: string;
  months: MonthGroup[];
}
function groupByTime(): YearGroup[] {
  if (!photos.value) return [];
  const map = new Map<string, Map<string, TestPhoto[]>>();
  for (const p of photos.value) {
    const y = yearOf(p);
    const m = p.shoot_time ? p.shoot_time.slice(0, 7).replace("-", "年") + "月" : "未知月份";
    if (!map.has(y)) map.set(y, new Map());
    const mm = map.get(y)!;
    if (!mm.has(m)) mm.set(m, []);
    mm.get(m)!.push(p);
  }
  const out: YearGroup[] = [];
  for (const [y, months] of [...map.entries()].sort()) {
    const ms: MonthGroup[] = [];
    for (const [m, ps] of [...months.entries()].sort()) {
      ms.push({ month: m, photos: ps });
    }
    out.push({ year: y, months: ms });
  }
  return out;
}

/** 按地点分组：地点 → 照片（无地点排最后） */
function groupByPlace(): { place: string; photos: TestPhoto[] }[] {
  if (!photos.value) return [];
  const map = new Map<string, TestPhoto[]>();
  for (const p of photos.value) {
    const key = p.place ?? "无地点";
    if (!map.has(key)) map.set(key, []);
    map.get(key)!.push(p);
  }
  return [...map.entries()]
    .sort((a, b) => {
      if (a[0] === "无地点") return 1;
      if (b[0] === "无地点") return -1;
      return a[0].localeCompare(b[0], "zh");
    })
    .map(([place, ps]) => ({ place, photos: ps }));
}

/** 坐标格式化 */
function fmtCoord(p: TestPhoto): string {
  if (p.lat === null || p.lon === null) return "—";
  return `${Math.abs(p.lat).toFixed(4)}°${p.lat >= 0 ? "N" : "S"}, ${Math.abs(p.lon).toFixed(4)}°${p.lon >= 0 ? "E" : "W"}`;
}

/** 统计 */
const stats = () => {
  const ps = photos.value ?? [];
  return {
    total: ps.length,
    withTime: ps.filter((p) => p.shoot_time).length,
    withGps: ps.filter((p) => p.lat !== null).length,
    withPlace: ps.filter((p) => p.place).length,
  };
};
</script>

<template>
  <div class="scan-page" :style="tsVars">
    <header class="page-header">
      <button class="btn" @click="router.push('/scan')">← 返回图片扫描</button>
      <div class="header-text">
        <h1>图片扫描测试</h1>
        <p class="page-sub">扫描文件夹内直接图片 → 按时间/地点排序 → 按「年·地点」组织移动（仅测试，不入相册）</p>
      </div>
    </header>

    <!-- 目录选择 + 操作（扫描小组件：支持递归模式选择） -->
    <section class="toolbar glass-card">
      <div class="dir-row">
        <input
          v-model="dirPath"
          class="dir-input"
          placeholder="输入文件夹路径，或点击「浏览」选择（勾选递归则扫子目录）"
          @keyup.enter="scanPhotos"
        />
        <button class="btn" @click="browseDir">浏览…</button>
        <button class="btn btn-primary" :disabled="scanning || !dirPath" @click="scanPhotos">
          {{ scanning ? "扫描中…" : "扫描" }}
        </button>
        <button class="btn btn-primary" :disabled="resolving || !photos || !dirPath" @click="resolvePlaces">
          {{ resolving ? "地名解析中…" : "解析地名" }}
        </button>
        <button class="btn btn-danger" :disabled="organizing || !photos || !dirPath" @click="organizePhotos">
          {{ organizing ? "移动中…" : "按年·地点组织移动" }}
        </button>
      </div>
      <!-- 递归模式开关（小组件功能） -->
      <label class="recursive-toggle" :class="{ active: recursive }">
        <input type="checkbox" v-model="recursive" />
        <span class="recursive-label">递归子目录</span>
        <span class="recursive-desc">{{ recursive ? "扫描所选文件夹及其所有子目录" : "只扫所选文件夹直接图片（不递归）" }}</span>
      </label>
      <p class="hint">解析地名：本地省/市离线查询（GPS 聚类，秒回）；仅未命中（国外/公海）时才联网。组织移动为破坏性操作，执行前有确认。</p>
    </section>

    <!-- 进度条（解析地名 / 组织移动，逐张上报） -->
    <section v-if="progress" class="progress-card glass-card">
      <div class="progress-head">
        <span class="progress-phase">{{ progress.phase === "resolve" ? "🔍 解析地名" : "📁 组织移动" }}</span>
        <span class="progress-count">{{ progress.current }} / {{ progress.total }}</span>
      </div>
      <div class="progress-track">
        <div
          class="progress-fill"
          :class="{ 'fill-done': progress.current >= progress.total }"
          :style="{ width: (progress.total > 0 ? (progress.current / progress.total) * 100 : 0) + '%' }"
        ></div>
      </div>
      <div class="progress-msg">
        <span class="progress-file" :title="progress.file_name">{{ progress.file_name }}</span>
        <span class="progress-result">{{ progress.message }}</span>
      </div>
    </section>

    <p v-if="error" class="scan-error">{{ error }}</p>

    <!-- 移动报告 -->
    <section v-if="report" class="glass-card report-card">
      <h3 class="card-title">组织移动报告</h3>
      <div class="report-stats">
        <span class="kpi">总数 <b>{{ report.total }}</b></span>
        <span class="kpi ok">已移动 <b>{{ report.moved }}</b></span>
        <span class="kpi warn">冲突跳过 <b>{{ report.conflict }}</b></span>
        <span class="kpi warn">无时间 <b>{{ report.no_time }}</b></span>
        <span class="kpi warn">无地点 <b>{{ report.no_place }}</b></span>
        <span class="kpi err">失败 <b>{{ report.failed }}</b></span>
      </div>
      <p class="report-root">目标：{{ report.target_root }}</p>
      <details class="folder-detail">
        <summary>创建的文件夹（{{ report.folders.length }}）</summary>
        <ul class="folder-list">
          <li v-for="f in report.folders" :key="f">{{ f }}</li>
        </ul>
      </details>
      <button class="btn btn-sm" @click="report = null">关闭报告</button>
    </section>

    <!-- 结果区 -->
    <template v-if="photos">
      <div class="result-bar glass-card">
        <div class="stat-line">
          共 <b>{{ stats().total }}</b> 张（直接图片）｜有时间 <b>{{ stats().withTime }}</b> ｜
          有 GPS <b>{{ stats().withGps }}</b> ｜ 有地点 <b>{{ stats().withPlace }}</b>
        </div>
        <div class="view-switch">
          <button class="btn btn-sm" :class="{ 'btn-active': viewMode === 'time' }" @click="viewMode = 'time'">按时间</button>
          <button class="btn btn-sm" :class="{ 'btn-active': viewMode === 'place' }" @click="viewMode = 'place'">按地点</button>
        </div>
      </div>

      <!-- 按时间分组 -->
      <div v-if="viewMode === 'time'" class="group-list">
        <section v-for="g in groupByTime()" :key="g.year" class="group glass-card">
          <h3 class="group-title">{{ g.year }}（{{ g.months.reduce((n, m) => n + m.photos.length, 0) }} 张）</h3>
          <div v-for="m in g.months" :key="m.month" class="month-block">
            <h4 class="month-title">{{ m.month }}</h4>
            <table class="photo-table">
              <tbody>
                <tr v-for="p in m.photos" :key="p.path">
                  <td class="p-name" :title="p.path">{{ p.file_name }}</td>
                  <td class="p-time">{{ p.shoot_time ?? "—" }}</td>
                  <td class="p-coord">{{ fmtCoord(p) }}</td>
                  <td class="p-place">{{ p.place ?? "—" }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
      </div>

      <!-- 按地点分组 -->
      <div v-else class="group-list">
        <section v-for="g in groupByPlace()" :key="g.place" class="group glass-card">
          <h3 class="group-title">📍 {{ g.place }}（{{ g.photos.length }} 张）</h3>
          <table class="photo-table">
            <tbody>
              <tr v-for="p in g.photos" :key="p.path">
                <td class="p-name" :title="p.path">{{ p.file_name }}</td>
                <td class="p-time">{{ p.shoot_time ?? "—" }}</td>
                <td class="p-coord">{{ fmtCoord(p) }}</td>
              </tr>
            </tbody>
          </table>
        </section>
      </div>
    </template>

    <p v-else-if="!scanning && !error" class="empty-tip">选择文件夹后点击「扫描」，验证时间/地点识别准确率与照片移动功能。</p>
  </div>
</template>

<style scoped>
.scan-page {
  max-width: 1000px;
  margin: 0 auto;
  padding: 24px 20px 60px;
  font-size: 14px;
  color: var(--ts-text);
}
.page-header {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 20px;
}
.header-text h1 {
  font-size: 20px;
  margin: 0;
}
.page-sub {
  color: var(--ts-muted);
  font-size: 12.5px;
  margin: 4px 0 0;
}
.glass-card {
  background: var(--ts-panel-bg);
  border: 1px solid var(--ts-panel-border);
  border-radius: 10px;
  padding: 16px 18px;
  margin-bottom: 16px;
  color: var(--ts-text);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.04);
}
.toolbar .dir-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.dir-input {
  flex: 1;
  min-width: 260px;
  padding: 8px 12px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 13px;
  outline: none;
}
.dir-input:focus {
  border-color: #396cd8;
}
.hint {
  color: #6b7280;
  font-size: 12px;
  margin: 10px 0 0;
}
.recursive-toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  padding: 6px 12px;
  border: 1px solid #d0d5dd;
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s;
  user-select: none;
}
.recursive-toggle:hover {
  border-color: #396cd8;
  background: #eef3fb;
}
.recursive-toggle.active {
  border-color: #396cd8;
  background: #eef3fb;
}
.recursive-toggle input {
  margin: 0;
  cursor: pointer;
}
.recursive-label {
  font-size: 13px;
  font-weight: 500;
  color: var(--ts-text);
}
.recursive-desc {
  font-size: 12px;
  color: #667085;
}
/* 进度条 */
.progress-card {
  padding: 12px 16px;
}
.progress-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 8px;
}
.progress-phase {
  font-size: 13px;
  font-weight: 600;
  color: #1f2937;
}
.progress-count {
  font-family: "Consolas", monospace;
  font-size: 12px;
  color: var(--ts-muted);
}
.progress-track {
  height: 8px;
  background: #eef0f3;
  border-radius: 4px;
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  background: linear-gradient(90deg, #396cd8, #5a8ce8);
  border-radius: 4px;
  transition: width 0.25s ease;
}
.progress-fill.fill-done {
  background: linear-gradient(90deg, #16a34a, #22c55e);
}
.progress-msg {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  margin-top: 6px;
  font-size: 12px;
}
.progress-file {
  color: #396cd8;
  font-family: "Consolas", monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.progress-result {
  color: var(--ts-muted);
  white-space: nowrap;
}
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid var(--ts-btn-border);
  background: var(--ts-btn-bg);
  color: var(--ts-text);
  cursor: pointer;
  font-size: 13px;
  transition: all 0.2s;
}
/* 排除带自身语义色的按钮，避免悬停态覆盖它们的主色 */
.btn:hover:not(.btn-primary):not(.btn-danger) {
  border-color: #396cd8;
  color: #396cd8;
  background: var(--ts-btn-hover);
}
.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}
.btn-primary:hover {
  background: #2f5cc2;
  color: #fff;
}
.btn-danger {
  background: #e5484d;
  color: #fff;
  border-color: #e5484d;
}
.btn-danger:hover {
  background: #d13438;
  color: #fff;
}
.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}
.btn-sm {
  padding: 5px 12px;
  font-size: 12px;
}
.btn-active {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}
.scan-error {
  color: #e5484d;
  background: #fef2f2;
  border: 1px solid #fecaca;
  border-radius: 8px;
  padding: 10px 14px;
  margin-bottom: 14px;
  font-size: 13px;
}
.report-card .card-title {
  margin: 0 0 10px;
  font-size: 15px;
}
.report-stats {
  display: flex;
  gap: 18px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.report-stats .kpi {
  font-size: 13px;
  color: var(--ts-muted);
}
.report-stats .kpi b {
  font-size: 16px;
  color: var(--ts-text);
}
.report-stats .ok b {
  color: #16a34a;
}
.report-stats .warn b {
  color: #d97706;
}
.report-stats .err b {
  color: #e5484d;
}
.report-root {
  color: var(--ts-muted);
  font-size: 12.5px;
  margin: 6px 0;
}
.folder-detail {
  margin: 8px 0;
}
.folder-detail summary {
  cursor: pointer;
  color: #396cd8;
  font-size: 13px;
}
.folder-list {
  max-height: 180px;
  overflow-y: auto;
  margin: 8px 0;
  padding-left: 20px;
  font-size: 12.5px;
  color: var(--ts-muted);
  font-family: "Consolas", monospace;
}
.result-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  padding: 12px 18px;
}
.stat-line {
  color: var(--ts-muted);
  font-size: 13px;
}
.stat-line b {
  color: var(--ts-text);
}
.view-switch {
  display: flex;
  gap: 6px;
}
.group-list {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.group-title {
  margin: 0 0 8px;
  font-size: 15px;
  color: #1f2937;
  border-bottom: 2px solid #396cd8;
  display: inline-block;
  padding-bottom: 4px;
}
.month-block {
  margin-bottom: 10px;
}
.month-title {
  margin: 0 0 6px;
  font-size: 13px;
  color: var(--ts-muted);
}
.photo-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12.5px;
}
.photo-table td {
  padding: 7px 10px;
  border-bottom: 1px solid #f3f4f6;
  color: var(--ts-muted);
}
.photo-table tr:last-child td {
  border-bottom: none;
}
.p-name {
  font-family: "Consolas", monospace;
  color: #396cd8;
  width: 38%;
}
.p-time {
  white-space: nowrap;
}
.p-coord {
  white-space: nowrap;
  color: var(--ts-muted);
  font-family: "Consolas", monospace;
  font-size: 12px;
}
.p-place {
  color: #b45309;
}
.empty-tip {
  text-align: center;
  color: #6b7280;
  padding: 40px 0;
  font-size: 13px;
}
</style>
