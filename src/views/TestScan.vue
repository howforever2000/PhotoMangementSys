<script setup lang="ts">
/**
 * 相册扫描分组工具页（原扫描测试工具；不落库）
 *
 * 流程：
 *   1. 选择文件夹（open dialog）→ 扫描：提取每张直接图片的时间（三级兜底）+ GPS 坐标
 *   2. 「解析地名」：GPS 聚类 → 本地省/市点面判断（离线秒回；未命中才联网）
 *   3. 视图切换：按时间（年→月）/ 按地点 查看识别结果，验证准确率
 *   4. 「按年·地点组织移动」：创建 {dir}/{年份}/{地点}/ 两级文件夹并移动照片（破坏性，需确认）
 *
 * FEAT-064 架构变更（两个用户反馈的问题）：
 *   ① **退出页面任务不消失**：任务状态由后端 `ScanJobState`（进程级）+ 前端
 *      `useScanTaskStore`（脱离组件）共同持有，页面卸载/路由切换都不中断；
 *      重新进入页面用 `get_scan_job` 快照即刻恢复进度与结果。
 *   ② **多线程 + 可调核数**：扫描走 rayon 线程池并行（`scan_test_photos_cancellable`），
 *      线程数在右上角「⚙ 性能设置」里可调，并支持**按当前相册实测推荐最优值**。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import type { TestPhoto } from "../types/photo";
import { useThemeStore } from "../stores/theme";
import { useScanTaskStore } from "../stores/scanTask";
import { useNotify } from "../composables/useNotify";
import ScanPerfSettings from "../components/ScanPerfSettings.vue";

const router = useRouter();
const theme = useThemeStore();
const notify = useNotify();
const task = useScanTaskStore();

/** 页面级主题变量：面板近乎透明，文字实际落在页面背景上 ——
 *  颜色跟随实际背景的对比度（theme.onBg*），面板底/边框按 onBgDark 二分；
 *  旧写法按 mode.isDark 二分，浅色预设下白字融进浅背景（用户反馈）。 */
const tsVars = computed(() => {
  const onDark = theme.onBgDark; // true = 浅背景配深字
  return {
    "--ts-panel-bg": onDark ? "rgba(0,0,0,.04)" : "rgba(255,255,255,.045)",
    "--ts-panel-border": onDark ? "rgba(0,0,0,.1)" : "rgba(255,255,255,.1)",
    "--ts-btn-bg": onDark ? "rgba(0,0,0,.045)" : "rgba(255,255,255,.06)",
    "--ts-btn-border": onDark ? "rgba(0,0,0,.16)" : "rgba(255,255,255,.18)",
    "--ts-btn-hover": onDark ? "rgba(0,0,0,.08)" : "rgba(255,255,255,.12)",
    "--ts-text": theme.onBgColor,
    "--ts-muted": theme.onBgSubColor,
  };
});

/** 目标文件夹路径 */
const dirPath = ref("");
/** 是否递归扫描子目录（小组件功能，用户可选；后续全局扫描沿用同一开关） */
const recursive = ref(false);
/** 视图模式：time=按时间（年→月） / place=按地点 */
const viewMode = ref<"time" | "place">("time");
/** 本地错误（启动校验/操作失败；任务自身的错误走 task.snapshot.error） */
const error = ref("");
/** 性能设置弹窗 */
const perfOpen = ref(false);

/** 扫描结果（来自 store，脱离组件存活） */
const photos = computed(() => task.photos);
/** 运行中（后端权威状态） */
const running = computed(() => task.running);
/** 各阶段是否在跑（用于按钮文案/禁用） */
const scanning = computed(() => running.value && task.phase === "scan");
const resolving = computed(() => running.value && task.phase === "resolve");
const organizing = computed(() => running.value && task.phase === "organize");
/** 进度（优先实时事件，回落快照） */
const progress = computed(() => task.progress);
/** 状态条文案（含「后台执行中」提示） */
const statusText = computed(() => task.statusText);
/** 组织移动报告 */
const report = computed(() => task.organizeReport);
/** 任务错误 */
const jobError = computed(() => task.snapshot.error);

/** 页面挂载：恢复后端任务状态 + 注册事件/轮询（这是「退出后回来能续上」的入口） */
onMounted(async () => {
  await task.ensureListener();
  try {
    const snap = await task.refresh();
    // 恢复上次的目录与递归选项（任务挂在 store 上，切页面回来仍连续）
    if (snap.dir) {
      dirPath.value = snap.dir;
      recursive.value = snap.recursive;
    }
    if (snap.status === "running") {
      task.startPolling();
      notify.info(
        `${task.statusText}中…`,
        `任务在后台继续执行（${snap.threads} 线程），离开本页面不会中断`,
        4000,
      );
    }
  } catch (e) {
    console.warn("[test-scan] 恢复任务状态失败:", e);
  }
  // 有历史结果（已完成）→ 重新拉取照片列表以渲染分组
  await maybeLoadPhotos();
});

/** 卸载时只停轮询/事件，**不清任务状态** —— 这就是「退出后任务不消失」 */
onUnmounted(() => {
  task.dispose();
});

/**
 * 任务终态 → 自动拉取结果列表 / 提示
 *
 * 场景：用户在扫描跑着的时候切到别的页面，任务在后台完成了。此时页面重新挂载
 * 时 `refresh()` 已拿到终态，但照片列表还没拉（快照不含整表）；本 watch 负责补上。
 * 同时给一个 Toast 告知「后台那件事干完了」——否则用户可能不知道结果已就绪。
 */
watch(
  () => [task.snapshot.status, task.snapshot.finished_at] as const,
  async ([status, finishedAt], prev) => {
    const prevStatus = prev?.[0];
    if (status === "running") return;
    if (status === "idle") return;
    // 只在「刚进入终态」时触发（避免重复提示/重复拉取）
    if (prevStatus === status && prev?.[1] === finishedAt) return;
    await maybeLoadPhotos();
    const snap = task.snapshot;
    if (status === "done") {
      if (snap.phase === "organize") {
        const rep = snap.organize;
        notify.success(
          "组织移动完成",
          rep ? `已移动 ${rep.moved} 张${rep.conflict ? `，冲突跳过 ${rep.conflict}` : ""}` : "",
          5000,
        );
      } else if (snap.phase === "resolve") {
        notify.success(
          "地名解析完成",
          `${snap.place_count} / ${snap.photo_count} 张有地点`,
          5000,
        );
      } else {
        notify.success("扫描完成", `共 ${snap.photo_count} 张`, 5000);
      }
    } else if (status === "failed") {
      notify.error("任务失败", snap.error);
    } else if (status === "cancelled") {
      notify.warning("任务已停止", "已处理部分结果保留", 4000);
    }
  },
);

/** 任务处于终态且有目录 → 拉取照片列表（快照不含整表） */
async function maybeLoadPhotos() {
  const snap = task.snapshot;
  if (!snap.dir) return;
  if (snap.status === "idle") return;
  // organize 阶段把照片**移走**了：此时再扫原目录只会得到 0 张（照片已在新文件夹），
  // 会把结果区清空、只留一份报告 —— 报告里已有完整统计，故不重扫。
  // 若用户想看结果，切到按年目录再扫即可（那是新位置，语义正确）。
  if (snap.phase === "organize") {
    task.photosLoaded = true;
    return;
  }
  // 扫描到 0 张时无需拉列表（空态文案由模板处理）
  if (snap.photo_count === 0) {
    task.photosLoaded = true;
    return;
  }
  try {
    await task.loadPhotos(snap.dir, snap.recursive);
  } catch (e) {
    console.warn("[test-scan] 加载结果列表失败:", e);
  }
}

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

/** 扫描：时间 + GPS 坐标（recursive 控制是否递归子目录；后台并行执行） */
async function scanPhotos() {
  if (running.value || !dirPath.value) return;
  error.value = "";
  try {
    await task.start("scan", dirPath.value, recursive.value);
    notify.info(
      "扫描已开始",
      `后台并行执行（${task.threadsUsed} 线程），离开本页面不会中断`,
      4000,
    );
  } catch (e) {
    error.value = `扫描启动失败：${e}`;
  }
}

/** 解析地名：GPS 聚类 + 本地省/市优先（离线秒回，未命中联网兜底），逐张进度上报 */
async function resolvePlaces() {
  if (running.value || !dirPath.value) return;
  error.value = "";
  try {
    await task.start("resolve", dirPath.value, recursive.value);
    notify.info("地名解析已开始", "后台执行中，离开本页面不会中断", 4000);
  } catch (e) {
    error.value = `地名解析启动失败：${e}`;
  }
}

/** 按年·地点组织移动（破坏性操作，需确认） */
async function organizePhotos() {
  if (running.value || !dirPath.value || !photos.value.length) return;
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
  error.value = "";
  try {
    await task.start("organize", dirPath.value, recursive.value);
    notify.info("组织移动已开始", "破坏性操作执行中，可在进度条查看", 4000);
  } catch (e) {
    error.value = `组织移动启动失败：${e}`;
  }
}

/** 停止当前任务（已处理部分保留） */
async function stopTask() {
  await task.cancel();
  notify.info("正在停止", "当前任务会尽快收敛，已处理部分结果保留", 3500);
}

/** 清除记录（回到空闲态） */
async function clearTask() {
  if (running.value) return;
  await task.clear();
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
  if (!photos.value.length) return [];
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
  if (!photos.value.length) return [];
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
const stats = computed(() => {
  const ps = photos.value;
  return {
    total: ps.length,
    withTime: ps.filter((p) => p.shoot_time).length,
    withGps: ps.filter((p) => p.lat !== null).length,
    withPlace: ps.filter((p) => p.place).length,
  };
});

/** 速率/剩余时间展示（运行中才有意义） */
const rateText = computed(() => {
  const p = progress.value;
  if (!running.value || !p || !p.rate) return "";
  const eta = p.eta_sec;
  const etaStr =
    eta == null ? "" : eta < 60 ? ` · 剩余约 ${Math.ceil(eta)} 秒` : ` · 剩余约 ${Math.ceil(eta / 60)} 分钟`;
  return `${p.rate.toFixed(0)} 张/秒${etaStr}`;
});

/** 进度条标题 */
const phaseLabel = computed(() => {
  const ph = progress.value?.phase ?? task.phase;
  if (ph === "scan") return "🔍 扫描中";
  if (ph === "resolve") return "📍 解析地名";
  if (ph === "organize") return "📁 组织移动";
  return "";
});
</script>

<template>
  <div class="scan-page" :style="tsVars">
    <header class="page-header">
      <button class="btn" @click="router.push('/scan')">← 返回图片扫描</button>
      <div class="header-text">
        <h1>📁 相册扫描分组工具</h1>
        <p class="page-sub">
          扫描文件夹提取拍摄时间 / GPS → 按年·地点分组预览 → 一键组织移动（不落库，需确认）
        </p>
      </div>
      <!-- FEAT-064：右上角性能设置（扫描线程数 + 按相册实测推荐） -->
      <button
        class="btn perf-btn"
        title="扫描线程数 / 按当前相册实测推荐最优线程"
        @click="perfOpen = true"
      >
        ⚙ 性能设置
      </button>
    </header>

    <!-- 性能设置弹窗（Teleport 到 body，避免被页面容器裁剪） -->
    <Teleport to="body">
      <div v-if="perfOpen" class="perf-mask" @click.self="perfOpen = false">
        <div class="perf-dialog" :style="tsVars">
          <div class="perf-head">
            <h3>⚙ 扫描性能设置</h3>
            <button class="btn btn-sm" @click="perfOpen = false">✕</button>
          </div>
          <ScanPerfSettings :dir="dirPath" :recursive="recursive" />
        </div>
      </div>
    </Teleport>

    <!-- 目录选择 + 操作（扫描小组件：支持递归模式选择） -->
    <section class="toolbar glass-card">
      <div class="dir-row">
        <input
          v-model="dirPath"
          class="dir-input"
          placeholder="输入文件夹路径，或点击「浏览」选择（勾选递归则扫子目录）"
          :disabled="running"
          @keyup.enter="scanPhotos"
        />
        <button class="btn" :disabled="running" @click="browseDir">浏览…</button>
        <button
          class="btn btn-primary"
          :disabled="running || !dirPath"
          @click="scanPhotos"
        >
          {{ scanning ? "扫描中…" : "扫描" }}
        </button>
        <button
          class="btn btn-primary"
          :disabled="running || !photos.length || !dirPath"
          @click="resolvePlaces"
        >
          {{ resolving ? "地名解析中…" : "解析地名" }}
        </button>
        <button
          class="btn btn-danger"
          :disabled="running || !photos.length || !dirPath"
          @click="organizePhotos"
        >
          {{ organizing ? "移动中…" : "按年·地点组织移动" }}
        </button>
        <!-- FEAT-064：运行中显示「停止」；空闲且有记录显示「清除记录」 -->
        <button v-if="running" class="btn btn-stop" @click="stopTask">■ 停止</button>
        <button
          v-else-if="task.snapshot.status !== 'idle'"
          class="btn btn-sm"
          @click="clearTask"
        >
          🧹 清除记录
        </button>
      </div>
      <!-- 递归模式开关（小组件功能） -->
      <label class="recursive-toggle" :class="{ active: recursive, locked: running }">
        <input type="checkbox" v-model="recursive" :disabled="running" />
        <span class="recursive-label">递归子目录</span>
        <span class="recursive-desc">{{ recursive ? "扫描所选文件夹及其所有子目录" : "只扫所选文件夹直接图片（不递归）" }}</span>
      </label>
      <p class="hint">
        解析地名：本地省/市离线查询（GPS 聚类，秒回）；仅未命中（国外/公海）时才联网。组织移动为破坏性操作，执行前有确认。<br />
        <b>任务在后台执行</b>：离开本页面不会中断，重新进入可继续查看进度与结果；扫描为多线程并行，
        线程数可在右上角「⚙ 性能设置」中调整（支持按当前相册实测推荐）。
      </p>
    </section>

    <!-- 任务状态条（FEAT-064：状态/进度/线程数/停止；退出页面回来仍可见） -->
    <section
      v-if="task.snapshot.status !== 'idle'"
      class="job-card glass-card"
      :class="{ 'job-running': running }"
    >
      <div class="job-head">
        <span class="job-status" :class="`st-${task.snapshot.status}`">
          <span v-if="running" class="job-dot"></span>
          {{ statusText }}
        </span>
        <span v-if="task.threadsUsed" class="job-threads">{{ task.threadsUsed }} 线程</span>
        <span v-if="running" class="job-bg-tip">后台执行中 · 可安全离开本页面</span>
        <button v-if="running" class="btn btn-stop btn-sm" @click="stopTask">■ 停止</button>
      </div>

      <!-- 进度条 -->
      <template v-if="progress && progress.total > 0">
        <div class="progress-track">
          <div
            class="progress-fill"
            :class="{ 'fill-done': progress.current >= progress.total }"
            :style="{ width: task.percent + '%' }"
          ></div>
        </div>
        <div class="progress-msg">
          <span class="progress-file" :title="progress.file_name">
            <b class="progress-phase">{{ phaseLabel }}</b>
            {{ progress.file_name || "—" }}
          </span>
          <span class="progress-count">
            {{ progress.current }} / {{ progress.total }}（{{ task.percent }}%）
          </span>
        </div>
        <div class="progress-sub">
          <span class="progress-result">{{ progress.message }}</span>
          <span v-if="rateText" class="progress-rate">{{ rateText }}</span>
        </div>
      </template>
    </section>

    <p v-if="error" class="scan-error">{{ error }}</p>
    <p v-if="jobError && task.snapshot.status === 'failed'" class="scan-error">
      任务失败：{{ jobError }}
    </p>

    <!-- 移动报告 -->
    <section v-if="report" class="glass-card report-card">
      <h3 class="card-title">
        组织移动报告
        <span v-if="report.cancelled" class="report-cancel-tag">（用户中途停止，为已处理部分的统计）</span>
      </h3>
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
    </section>

    <!-- 结果区 -->
    <template v-if="photos.length">
      <div class="result-bar glass-card">
        <div class="stat-line">
          共 <b>{{ stats.total }}</b> 张（直接图片）｜有时间 <b>{{ stats.withTime }}</b> ｜
          有 GPS <b>{{ stats.withGps }}</b> ｜ 有地点 <b>{{ stats.withPlace }}</b>
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

    <p v-else-if="!running && !error && task.snapshot.status === 'idle'" class="empty-tip">
      选择文件夹后点击「扫描」，验证时间/地点识别准确率与照片移动功能。
    </p>
    <p v-else-if="!running && task.snapshot.status === 'done' && task.snapshot.photo_count === 0" class="empty-tip">
      扫描完成但未发现图片：
      {{ recursive ? "请确认所选文件夹及其子目录下含有图片。" : "本功能只扫描所选文件夹下的直接图片（不递归子目录），请确认照片直接放在该文件夹中，或勾选「递归子目录」。" }}
    </p>
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
.header-text {
  flex: 1;
}
.header-text h1 {
  font-size: 20px;
  margin: 0;
}
/* FEAT-064：性能设置按钮固定在头部右侧 */
.perf-btn {
  flex-shrink: 0;
  white-space: nowrap;
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
  border: 1px solid var(--ts-btn-border);
  border-radius: 8px;
  font-size: 13px;
  outline: none;
  background: var(--ts-btn-bg);
  color: var(--ts-text);
}
.dir-input:focus {
  border-color: #396cd8;
}
/* placeholder 别被 body.theme-dark 全局白系规则盖掉：本页输入框是浅底（onBg 系） */
.dir-input::placeholder {
  color: var(--ts-muted);
}
.dir-input:disabled {
  opacity: 0.7;
  cursor: not-allowed;
}
.hint {
  color: var(--ts-muted);
  font-size: 12px;
  margin: 10px 0 0;
  line-height: 1.65;
}
.recursive-toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  padding: 6px 12px;
  border: 1px solid var(--ts-btn-border);
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s;
  user-select: none;
}
.recursive-toggle:hover:not(.locked) {
  border-color: #396cd8;
  background: var(--ts-btn-hover);
}
.recursive-toggle.active {
  border-color: #396cd8;
  background: var(--ts-btn-hover);
}
.recursive-toggle.locked {
  opacity: 0.7;
  cursor: not-allowed;
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
  color: var(--ts-muted);
}
/* FEAT-064：任务状态卡 */
.job-card {
  padding: 12px 16px;
}
.job-running {
  border-color: rgba(57, 108, 216, 0.45);
}
.job-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.job-status {
  font-size: 13px;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.job-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #396cd8;
  animation: job-pulse 1.2s ease-in-out infinite;
}
@keyframes job-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}
.st-running { color: #396cd8; }
.st-done { color: #16a34a; }
.st-failed { color: #e5484d; }
.st-cancelled { color: #d97706; }
.job-threads {
  font-size: 12px;
  color: var(--ts-muted);
  border: 1px solid var(--ts-btn-border);
  border-radius: 6px;
  padding: 1px 7px;
}
.job-bg-tip {
  font-size: 11.5px;
  color: #16a34a;
}
/* 进度条 */
.progress-track {
  height: 8px;
  background: var(--ts-btn-hover);
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
.progress-phase {
  color: #396cd8;
  margin-right: 6px;
}
.progress-file {
  color: var(--ts-text);
  font-family: "Consolas", monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.progress-count {
  font-family: "Consolas", monospace;
  font-size: 12px;
  color: var(--ts-muted);
  white-space: nowrap;
}
.progress-sub {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  margin-top: 4px;
  font-size: 11.5px;
}
.progress-result {
  color: var(--ts-muted);
}
.progress-rate {
  color: #16a34a;
  font-family: "Consolas", monospace;
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
.btn:hover:not(.btn-primary):not(.btn-danger):not(.btn-stop):not(.btn-active) {
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
  background: var(--color-danger);
  color: #fff;
  border-color: #e5484d;
}
.btn-danger:hover {
  background: var(--color-danger-hover);
  color: #fff;
}
.btn-stop {
  background: #d97706;
  color: #fff;
  border-color: #d97706;
}
.btn-stop:hover {
  background: #b45309;
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
  background: rgba(229, 72, 77, 0.08);
  border: 1px solid rgba(229, 72, 77, 0.35);
  border-radius: 8px;
  padding: 10px 14px;
  margin-bottom: 14px;
  font-size: 13px;
  white-space: pre-wrap;
}
.report-card .card-title {
  margin: 0 0 10px;
  font-size: 15px;
}
.report-cancel-tag {
  font-size: 12px;
  font-weight: 400;
  color: #d97706;
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
  color: var(--ts-text);
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
  border-bottom: 1px solid var(--ts-panel-border);
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
  color: var(--ts-muted);
  padding: 40px 0;
  font-size: 13px;
}
/* 性能设置弹窗 */
.perf-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
  padding: 24px;
}
.perf-dialog {
  background: var(--ts-panel-bg);
  border: 1px solid var(--ts-panel-border);
  border-radius: 12px;
  padding: 16px 18px;
  max-height: 86vh;
  overflow-y: auto;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.3);
  color: var(--ts-text);
}
.perf-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}
.perf-head h3 {
  margin: 0;
  font-size: 15px;
}
</style>
