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
 *      线程数在「⚙ 性能设置」里可调，并支持**按当前相册实测推荐最优值**。
 *
 * FEAT-151 改造（导航与界面与模块其余分区统一）：
 *   · 页头换成模块同一个 `AppHeader`：分区表来自 `utils/scanModuleTabs.ts`，
 *     和 `/scan` 完全同源 —— 之前这里自带一份硬编码分区，「重复清理」还写着
 *     "功能待开发"，且另加了一个「← 返回图片扫描」按钮，等于同一排导航做两遍；
 *   · 页面壳与其余分区对齐：数据带（StatBand）+ var(--sp-8) 留白，
 *     不再自造 `--ts-*` 局部配色（浅色预设下白字融进浅背景的老问题一并消失）；
 *   · 结果表补表头：原来只有四列裸数据，用户不知道哪列是坐标。
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import type { TestPhoto } from "../types/photo";
import { useScanTaskStore } from "../stores/scanTask";
import { useNotify } from "../composables/useNotify";
import ScanPerfSettings from "../components/ScanPerfSettings.vue";
import AppHeader from "../components/ui/AppHeader.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import EmptyState from "../components/EmptyState.vue";
import { scanModuleTabs } from "../utils/scanModuleTabs";

const notify = useNotify();
const task = useScanTaskStore();
const router = useRouter();

/** 页头分区：与 /scan 同源（子页不带页内分区高亮，路由型 tab 由 AppHeader 自动高亮） */
const moduleTabs = scanModuleTabs();

/**
 * 本页点其他分区 tab：回到 Hub 并落到那块
 *
 * 本页是 `/scan` 的**路由型子页**，三个页内分区不在这页渲染，所以不能只切状态。
 * 用 `?tab=` 深链（与智慧相册同一套口径），Hub 会在挂载时读 query。
 */
function pickSection(key: string) {
  router.push({ path: "/scan", query: { tab: key } }).catch(() => {});
}

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
        notify.success("地名解析完成", `${snap.place_count} / ${snap.photo_count} 张有地点`, 5000);
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
    title: recursive.value
      ? "选择要扫描的文件夹（递归子目录）"
      : "选择要扫描的文件夹（只扫直接图片，不递归子目录）",
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
    notify.info("扫描已开始", `后台并行执行（${task.threadsUsed} 线程），离开本页面不会中断`, 4000);
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

/** 数据带：把原来挤在结果条里的四个统计搬上来（与模块其余分区同一版式） */
const statItems = computed<StatItem[]>(() => {
  const s = stats.value;
  const st = task.snapshot.status;
  const jobText =
    st === "idle" ? "空闲" : st === "running" ? statusText.value : st === "done" ? "已完成" : "已停止";
  return [
    { label: "扫描照片", value: s.total, unit: "张" },
    {
      label: "有时间",
      value: s.withTime,
      unit: "张",
      progress: s.total ? s.withTime / s.total : 0,
      progressTone: "muted",
    },
    { label: "有 GPS", value: s.withGps, unit: "张", progress: s.total ? s.withGps / s.total : 0 },
    { label: "有地点", value: s.withPlace, unit: "张" },
    {
      label: "任务",
      value: jobText,
      tone: st === "running" ? "up" : "mute",
      note: task.threadsUsed ? `${task.threadsUsed} 线程` : undefined,
    },
  ];
});

/** 速率/剩余时间展示（运行中才有意义） */
const rateText = computed(() => {
  const p = progress.value;
  if (!running.value || !p || !p.rate) return "";
  const eta = p.eta_sec;
  const etaStr =
    eta == null
      ? ""
      : eta < 60
        ? ` · 剩余约 ${Math.ceil(eta)} 秒`
        : ` · 剩余约 ${Math.ceil(eta / 60)} 分钟`;
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
  <div class="scan-tool-page">
    <!-- 模块页头：与 /scan 同一份分区表（子页不高亮任何页内分区） -->
    <AppHeader
      title="扫描入库"
      icon="📥"
      meta="相册扫描分组工具"
      :tabs="moduleTabs"
      @pick="pickSection"
    />

    <!-- 数据带：扫描张数 / 时间 / GPS / 地点 / 任务状态 -->
    <StatBand :items="statItems" class="tool-stats" />

    <section class="tool-body">
      <!-- ① 目录与操作：一行一件事（选目录 → 扫描 → 解析 → 移动），危险操作靠右且换成危险色 -->
      <div class="tool-card">
        <div class="dir-row">
          <input
            v-model="dirPath"
            class="dir-input"
            placeholder="输入文件夹路径，或点「浏览」选择（勾选递归则扫子目录）"
            :disabled="running"
            aria-label="要扫描的文件夹路径"
            @keyup.enter="scanPhotos"
          />
          <button class="btn" :disabled="running" @click="browseDir">浏览…</button>
          <button class="btn btn-primary" :disabled="running || !dirPath" @click="scanPhotos">
            {{ scanning ? "扫描中…" : "扫描" }}
          </button>
        </div>

        <div class="act-row">
          <label class="recursive-toggle" :class="{ active: recursive, locked: running }">
            <input type="checkbox" v-model="recursive" :disabled="running" />
            <span class="recursive-label">递归子目录</span>
            <span class="recursive-desc">{{
              recursive ? "扫描所选文件夹及其所有子目录" : "只扫所选文件夹直接图片（不递归）"
            }}</span>
          </label>

          <span class="act-sep" aria-hidden="true" />

          <button
            class="btn"
            :disabled="running || !photos.length || !dirPath"
            title="GPS 聚类 → 本地省/市离线判断，未命中才联网"
            @click="resolvePlaces"
          >
            {{ resolving ? "地名解析中…" : "解析地名" }}
          </button>
          <button class="btn" :disabled="running" title="扫描线程数 / 按当前相册实测推荐" @click="perfOpen = true">
            ⚙ 性能设置
          </button>

          <span class="act-sp" />

          <button v-if="running" class="btn btn-stop" @click="stopTask">■ 停止</button>
          <button
            v-else-if="task.snapshot.status !== 'idle'"
            class="btn btn-sm"
            @click="clearTask"
          >
            🧹 清除记录
          </button>
          <button
            class="btn btn-danger"
            :disabled="running || !photos.length || !dirPath"
            title="按「年份/地点」建两级文件夹并移动照片（破坏性，执行前会二次确认）"
            @click="organizePhotos"
          >
            {{ organizing ? "移动中…" : "按年·地点组织移动" }}
          </button>
        </div>

        <p class="hint">
          本工具<b>不落库</b>：只提取时间与 GPS、按年·地点分组预览，用来验证识别结果。
          组织移动是破坏性操作（移动后原目录里就没有了），执行前有确认。
          任务在后台执行，离开本页面不会中断。
        </p>
      </div>

      <!-- ② 任务状态（FEAT-064：状态/进度/线程数/停止；退出页面回来仍可见） -->
      <div v-if="task.snapshot.status !== 'idle'" class="tool-card job-card" :class="{ 'job-running': running }">
        <div class="job-head">
          <span class="job-status" :class="`st-${task.snapshot.status}`">
            <span v-if="running" class="job-dot" />
            {{ statusText }}
          </span>
          <span v-if="task.threadsUsed" class="job-threads">{{ task.threadsUsed }} 线程</span>
          <span v-if="running" class="job-bg-tip">后台执行中 · 可安全离开本页面</span>
          <button v-if="running" class="btn btn-stop btn-sm" @click="stopTask">■ 停止</button>
        </div>

        <template v-if="progress && progress.total > 0">
          <div class="progress-track">
            <div
              class="progress-fill"
              :class="{ 'fill-done': progress.current >= progress.total }"
              :style="{ width: task.percent + '%' }"
            />
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
      </div>

      <p v-if="error" class="scan-error">{{ error }}</p>
      <p v-if="jobError && task.snapshot.status === 'failed'" class="scan-error">
        任务失败：{{ jobError }}
      </p>

      <!-- ③ 移动报告 -->
      <div v-if="report" class="tool-card">
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
      </div>

      <!-- ④ 结果区 -->
      <template v-if="photos.length">
        <div class="result-bar">
          <span class="stat-line">
            共 <b>{{ stats.total }}</b> 张 · 有时间 <b>{{ stats.withTime }}</b> · 有 GPS
            <b>{{ stats.withGps }}</b> · 有地点 <b>{{ stats.withPlace }}</b>
          </span>
          <div class="view-switch" role="group" aria-label="分组方式">
            <button
              class="view-btn"
              :class="{ on: viewMode === 'time' }"
              :aria-pressed="viewMode === 'time'"
              @click="viewMode = 'time'"
            >
              按时间
            </button>
            <button
              class="view-btn"
              :class="{ on: viewMode === 'place' }"
              :aria-pressed="viewMode === 'place'"
              @click="viewMode = 'place'"
            >
              按地点
            </button>
          </div>
        </div>

        <!-- 按时间分组（年 → 月 → 表） -->
        <div v-if="viewMode === 'time'" class="group-list">
          <section v-for="g in groupByTime()" :key="g.year" class="group">
            <h3 class="group-title">
              {{ g.year }}
              <span class="group-n">{{
                g.months.reduce((n, m) => n + m.photos.length, 0)
              }} 张</span>
            </h3>
            <div v-for="m in g.months" :key="m.month" class="month-block">
              <h4 class="month-title">{{ m.month }}</h4>
              <table class="photo-table">
                <thead>
                  <tr>
                    <th scope="col">文件名</th>
                    <th scope="col">拍摄时间</th>
                    <th scope="col">GPS 坐标</th>
                    <th scope="col">地点</th>
                  </tr>
                </thead>
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
          <section v-for="g in groupByPlace()" :key="g.place" class="group">
            <h3 class="group-title">
              📍 {{ g.place }} <span class="group-n">{{ g.photos.length }} 张</span>
            </h3>
            <table class="photo-table">
              <thead>
                <tr>
                  <th scope="col">文件名</th>
                  <th scope="col">拍摄时间</th>
                  <th scope="col">GPS 坐标</th>
                </tr>
              </thead>
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

      <!-- ⑤ 空态：未开始 / 扫到 0 张，两种原因文案不同 -->
      <EmptyState
        v-else-if="!running && !error && task.snapshot.status === 'idle'"
        icon="📁"
        title="选一个文件夹开始"
        description="扫描只读不落库：把所选文件夹里的图片时间与 GPS 提出来，按年·地点分组给你核对识别结果；确认无误再用「组织移动」整理到 {年份}/{地点} 两级文件夹。"
      />
      <EmptyState
        v-else-if="!running && task.snapshot.status === 'done' && task.snapshot.photo_count === 0"
        icon="🔍"
        title="扫描完成，但没发现图片"
        :description="
          recursive
            ? '请确认所选文件夹及其子目录下含有图片。'
            : '本功能只扫描所选文件夹下的直接图片（不递归子目录）：请确认照片直接放在该文件夹中，或勾选「递归子目录」。'
        "
      />
    </section>

    <!-- 性能设置弹窗（Teleport 到 body，避免被页面容器裁剪） -->
    <Teleport to="body">
      <div v-if="perfOpen" class="perf-mask" @click.self="perfOpen = false">
        <div class="perf-dialog" role="dialog" aria-label="扫描性能设置">
          <div class="perf-head">
            <h3>⚙ 扫描性能设置</h3>
            <button class="btn btn-sm" aria-label="关闭" @click="perfOpen = false">✕</button>
          </div>
          <ScanPerfSettings :dir="dirPath" :recursive="recursive" />
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
/* ---- 页面壳：与模块其余分区对齐（页头 / 数据带 / 内容左右留白一致） ---- */
.scan-tool-page {
  min-height: 100vh;
  padding-bottom: 48px;
  color: var(--fg);
}
.tool-stats {
  margin: 0 0 16px;
}
.tool-body {
  padding: 0 var(--sp-8);
  display: flex;
  flex-direction: column;
  gap: 14px;
}

/* 内容卡：全站统一的墙板玻璃（不再自造 --ts-* 局部配色） */
.tool-card {
  border: 1px solid var(--panel-glass-edge);
  border-radius: var(--panel-glass-radius);
  background: var(--panel-glass-fill);
  padding: 14px 16px;
}

/* ---- 目录行 / 操作行 ---- */
.dir-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.dir-input {
  flex: 1 1 320px;
  min-width: 220px;
  min-height: 34px;
  padding: 7px 12px;
  border: 1px solid var(--c-rule);
  border-radius: 8px;
  font-size: 13px;
  outline: none;
  background: color-mix(in srgb, var(--c-fg) 4%, transparent);
  color: var(--fg);
}
.dir-input::placeholder {
  color: var(--fg-2);
}
.dir-input:focus-visible {
  border-color: var(--color-primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--color-primary) 30%, transparent);
}
.dir-input:disabled {
  opacity: 0.65;
  cursor: not-allowed;
}

.act-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 10px;
}
.act-sep {
  width: 1px;
  height: 18px;
  background: var(--c-rule);
  margin: 0 2px;
}
.act-sp {
  flex: 1;
}

.recursive-toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 5px 11px;
  border: 1px solid var(--c-rule);
  border-radius: 8px;
  cursor: pointer;
  transition:
    border-color var(--dur-2),
    background var(--dur-2);
  user-select: none;
  min-height: 32px;
}
.recursive-toggle:hover:not(.locked) {
  border-color: var(--color-primary);
  background: color-mix(in srgb, var(--c-fg) 6%, transparent);
}
.recursive-toggle.active {
  border-color: color-mix(in srgb, var(--color-primary) 55%, transparent);
  background: color-mix(in srgb, var(--color-primary) 10%, transparent);
}
.recursive-toggle.locked {
  opacity: 0.7;
  cursor: not-allowed;
}
.recursive-toggle input {
  margin: 0;
  cursor: pointer;
  accent-color: var(--color-primary);
}
.recursive-label {
  font-size: 12.5px;
  font-weight: var(--fw-em);
  color: var(--fg);
}
.recursive-desc {
  font-size: 11.5px;
  color: var(--fg-2);
}

.hint {
  color: var(--fg-2);
  font-size: 11.5px;
  margin: 10px 0 0;
  line-height: 1.7;
}
.hint b {
  color: var(--fg);
}

/* ---- 任务卡 ---- */
.job-card {
  padding: 12px 16px;
}
.job-running {
  border-color: color-mix(in srgb, var(--color-primary) 45%, transparent);
}
.job-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.job-status {
  font-size: 12.5px;
  font-weight: var(--fw-em);
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--fg);
}
.job-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--color-primary);
  animation: job-pulse 1.2s ease-in-out infinite;
}
@keyframes job-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.3;
  }
}
@media (prefers-reduced-motion: reduce) {
  .job-dot {
    animation: none;
  }
}
.st-running {
  color: var(--color-primary-text);
}
.st-done {
  color: var(--color-ok-vivid);
}
.st-failed {
  color: var(--color-danger);
}
.st-cancelled {
  color: var(--color-warn-text);
}
.job-threads {
  font-size: 11.5px;
  color: var(--fg-2);
  border: 1px solid var(--c-rule);
  border-radius: 6px;
  padding: 1px 7px;
}
.job-bg-tip {
  font-size: 11.5px;
  color: var(--color-ok-vivid);
}

/* ---- 进度条 ---- */
.progress-track {
  height: 8px;
  background: color-mix(in srgb, var(--c-fg) 12%, transparent);
  border-radius: 4px;
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  background: linear-gradient(90deg, var(--color-primary), #5a8ce8);
  border-radius: 4px;
  transition: width var(--dur-3) ease;
}
.progress-fill.fill-done {
  background: linear-gradient(90deg, var(--color-ok-vivid), #22c55e);
}
.progress-msg {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  margin-top: 6px;
  font-size: 12px;
}
.progress-phase {
  color: var(--color-primary-text);
  margin-right: 6px;
}
.progress-file {
  color: var(--fg);
  font-family: "Consolas", monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.progress-count {
  font-family: "Consolas", monospace;
  font-size: 12px;
  color: var(--fg-2);
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
  color: var(--fg-2);
}
.progress-rate {
  color: var(--color-ok-vivid);
  font-family: "Consolas", monospace;
  white-space: nowrap;
}

/* ---- 按钮：全站 .btn 基础上做本页的尺寸收敛 ---- */
.btn {
  padding: 7px 15px;
  min-height: 34px;
  border-radius: 8px;
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--fg);
  cursor: pointer;
  font-size: 12.5px;
  transition:
    background var(--dur-2),
    color var(--dur-2),
    border-color var(--dur-2);
}
.btn:hover:not(:disabled):not(.btn-primary):not(.btn-danger):not(.btn-stop) {
  border-color: color-mix(in srgb, var(--color-primary) 55%, transparent);
  background: color-mix(in srgb, var(--c-fg) 7%, transparent);
}
.btn-primary {
  background: var(--color-primary);
  color: var(--color-on-primary);
  border-color: var(--color-primary);
  font-weight: var(--fw-em);
}
.btn-primary:hover:not(:disabled) {
  background: var(--color-primary-hover);
}
.btn-danger {
  background: color-mix(in srgb, var(--color-danger) 12%, transparent);
  color: var(--color-danger);
  border-color: color-mix(in srgb, var(--color-danger) 50%, transparent);
}
.btn-danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-danger) 20%, transparent);
}
.btn-stop {
  background: color-mix(in srgb, var(--color-warn-text) 15%, transparent);
  color: var(--color-warn-text);
  border-color: color-mix(in srgb, var(--color-warn-text) 45%, transparent);
}
.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.btn-sm {
  padding: 4px 10px;
  min-height: 28px;
  font-size: 11.5px;
}

/* ---- 错误条 ---- */
.scan-error {
  color: var(--color-danger);
  background: color-mix(in srgb, var(--color-danger) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-danger) 35%, transparent);
  border-radius: 8px;
  padding: 10px 14px;
  margin: 0;
  font-size: 12.5px;
  white-space: pre-wrap;
}

/* ---- 移动报告 ---- */
.card-title {
  margin: 0 0 10px;
  font-size: 13.5px;
  color: var(--fg);
}
.report-cancel-tag {
  font-size: 11.5px;
  font-weight: 400;
  color: var(--color-warn-text);
}
.report-stats {
  display: flex;
  gap: 18px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.report-stats .kpi {
  font-size: 12px;
  color: var(--fg-2);
}
.report-stats .kpi b {
  font-size: 15px;
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.report-stats .ok b {
  color: var(--color-ok-vivid);
}
.report-stats .warn b {
  color: var(--color-warn-text);
}
.report-stats .err b {
  color: var(--color-danger);
}
.report-root {
  color: var(--fg-2);
  font-size: 11.5px;
  margin: 6px 0;
  font-family: "Consolas", monospace;
}
.folder-detail {
  margin: 8px 0 0;
}
.folder-detail summary {
  cursor: pointer;
  color: var(--color-primary-text);
  font-size: 12.5px;
}
.folder-list {
  max-height: 180px;
  overflow-y: auto;
  margin: 8px 0 0;
  padding-left: 20px;
  font-size: 11.5px;
  color: var(--fg-2);
  font-family: "Consolas", monospace;
}

/* ---- 结果条 ---- */
.result-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  padding: 4px 2px;
  border-bottom: 1px solid var(--c-rule);
}
.stat-line {
  color: var(--fg-2);
  font-size: 12.5px;
}
.stat-line b {
  color: var(--fg);
  font-variant-numeric: tabular-nums;
}
.view-switch {
  display: inline-flex;
  gap: 2px;
  padding: 2px;
  border: 1px solid var(--c-rule);
  border-radius: 8px;
}
.view-btn {
  border: none;
  background: transparent;
  color: var(--fg-2);
  font-size: 11.5px;
  padding: 4px 12px;
  min-height: 26px;
  border-radius: 6px;
  cursor: pointer;
}
.view-btn:hover {
  color: var(--fg);
  background: color-mix(in srgb, var(--c-fg) 7%, transparent);
}
.view-btn.on {
  color: var(--fg);
  font-weight: var(--fw-em);
  background: color-mix(in srgb, var(--color-primary) 14%, transparent);
}

/* ---- 分组与表格 ---- */
.group-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.group {
  border: 1px solid var(--c-rule);
  border-radius: 12px;
  background: color-mix(in srgb, var(--c-fg) 3%, transparent);
  padding: 12px 14px;
}
.group-title {
  margin: 0 0 8px;
  font-size: 13.5px;
  color: var(--fg);
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.group-n {
  font-size: 11.5px;
  font-weight: 400;
  color: var(--fg-2);
  font-variant-numeric: tabular-nums;
}
.month-block {
  margin-bottom: 10px;
}
.month-title {
  margin: 0 0 6px;
  font-size: 12px;
  font-weight: 400;
  color: var(--fg-2);
}
.photo-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  table-layout: fixed;
}
.photo-table th {
  text-align: left;
  font-weight: 400;
  font-size: 11px;
  color: var(--fg-2);
  padding: 0 10px 5px;
  border-bottom: 1px solid var(--c-rule);
}
.photo-table td {
  padding: 6px 10px;
  border-bottom: 1px solid color-mix(in srgb, var(--c-fg) 7%, transparent);
  color: var(--fg-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.photo-table tbody tr:hover td {
  background: color-mix(in srgb, var(--c-fg) 5%, transparent);
  color: var(--fg);
}
.photo-table tr:last-child td {
  border-bottom: none;
}
.p-name {
  font-family: "Consolas", monospace;
  color: var(--fg);
  width: 40%;
}
.p-time {
  width: 22%;
  font-variant-numeric: tabular-nums;
}
.p-coord {
  width: 26%;
  font-family: "Consolas", monospace;
  font-size: 11.5px;
}
.p-place {
  color: var(--color-warn-text, var(--fg-2));
}

/* ---- 性能设置弹窗 ---- */
.perf-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 3000;
  padding: 24px;
}
.perf-dialog {
  background: var(--panel-glass-fill, var(--bg));
  border: 1px solid var(--panel-glass-edge, var(--c-rule));
  border-radius: 12px;
  padding: 16px 18px;
  max-height: 86vh;
  overflow-y: auto;
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.35);
  color: var(--fg);
  -webkit-backdrop-filter: blur(var(--panel-glass-blur, 0));
  backdrop-filter: blur(var(--panel-glass-blur, 0));
}
.perf-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 10px;
}
.perf-head h3 {
  margin: 0;
  font-size: 13.5px;
}

@media (max-width: 900px) {
  .tool-body {
    padding: 0 var(--sp-5);
  }
  .tool-card {
    padding: 12px;
  }
  .act-sp {
    display: none;
  }
}
</style>
