<script setup lang="ts">
/**
 * 全局照片扫描入库（FEAT-038）
 *
 * 跨相册批量扫描入库组件，托管在主页「图片扫描」板块下的专属页面：
 * - 勾选要扫描入库的相册（支持全选 / 反选，显示照片数与已入库数）
 * - 扫描类型复用相册管理中的组合扫描（EXIF 基础 / 影调分析 / AI 内容识别）+ 批次
 * - 支持启动 / 停止；两级进度条（相册级总进度 + 当前相册照片级进度）
 * - 后台执行：任务状态存于 Pinia store，离开页面扫描不中断，回来恢复显示
 */
import { computed, onMounted, ref } from "vue";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import type { GlobalScanItemStatus } from "../stores/content";
import { useNotify } from "../composables/useNotify";

const albumStore = useAlbumStore();
const contentStore = useContentStore();
const notify = useNotify();

// ---- 全局扫描任务（来自 store，脱离组件存活） ----
const job = computed(() => contentStore.globalScanJob);
const running = computed(() => job.value.running);
const stopping = computed(() => job.value.stopping);
const items = computed(() => job.value.items);

// ---- 相册勾选 ----
const albums = computed(() => albumStore.albums);
const albumsLoading = computed(() => albumStore.isLoading);
const selectedIds = ref<Set<number>>(new Set());

const selectedCount = computed(() => {
  // 仅统计当前列表中存在的相册（运行期间相册可能被删除）
  return albums.value.filter((a) => selectedIds.value.has(a.id)).length;
});
const allSelected = computed(
  () => albums.value.length > 0 && selectedCount.value === albums.value.length,
);
const someSelected = computed(() => selectedCount.value > 0 && !allSelected.value);

/** 全选 / 取消全选（全选 = 勾选当前列表全部相册） */
function toggleSelectAll() {
  if (running.value) return;
  if (allSelected.value) {
    selectedIds.value = new Set();
  } else {
    selectedIds.value = new Set(albums.value.map((a) => a.id));
  }
}

/** 反选 */
function invertSelection() {
  if (running.value) return;
  const next = new Set<number>();
  for (const a of albums.value) {
    if (!selectedIds.value.has(a.id)) next.add(a.id);
  }
  selectedIds.value = next;
}

/** 切换单个相册勾选 */
function toggleOne(id: number) {
  if (running.value) return;
  const next = new Set(selectedIds.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  selectedIds.value = next;
}

/** 相册搜索过滤（相册较多时快速定位） */
const filterKeyword = ref("");
const filteredAlbums = computed(() => {
  const kw = filterKeyword.value.trim().toLowerCase();
  if (!kw) return albums.value;
  return albums.value.filter(
    (a) => a.name.toLowerCase().includes(kw) || a.path.toLowerCase().includes(kw),
  );
});

// ---- 扫描设置（复用相册管理组合扫描的类型 + 批次） ----
const scanTypes = ref<string[]>(["basic", "tone", "ai"]);
const BATCH_OPTIONS = [8, 16, 32];
const batch = ref(8);

// ---- 进度计算 ----
/** 已到终态的相册数（done / failed / stopped） */
const finishedCount = computed(
  () => items.value.filter((i) => i.status !== "pending" && i.status !== "running").length,
);
const overallPercent = computed(() =>
  items.value.length === 0
    ? 0
    : Math.round((finishedCount.value / items.value.length) * 100),
);
const currentAlbum = computed(() =>
  job.value.currentIndex >= 0 ? items.value[job.value.currentIndex] : null,
);
const currentProgress = computed(() => job.value.currentProgress);
const currentPercent = computed(() => {
  const p = currentProgress.value;
  if (!p || p.total <= 0) return null;
  return Math.round((p.current / p.total) * 100);
});
const doneCount = computed(() => items.value.filter((i) => i.status === "done").length);
const failedCount = computed(() => items.value.filter((i) => i.status === "failed").length);
const stoppedCount = computed(() => items.value.filter((i) => i.status === "stopped").length);
const totalWritten = computed(() => items.value.reduce((n, i) => n + i.written, 0));
const hasSummary = computed(
  () => !running.value && items.value.length > 0 && job.value.finishedAt != null,
);

const STATUS_META: Record<GlobalScanItemStatus, { label: string; cls: string }> = {
  pending: { label: "等待", cls: "st-pending" },
  running: { label: "扫描中", cls: "st-running" },
  done: { label: "完成", cls: "st-done" },
  failed: { label: "失败", cls: "st-failed" },
  stopped: { label: "已停止", cls: "st-stopped" },
};

// ---- 启停 ----
function startScan() {
  if (running.value) return;
  const entries = albums.value
    .filter((a) => selectedIds.value.has(a.id))
    .map((a) => ({ id: a.id, name: a.name }));
  if (!entries.length) {
    notify.warning("请先勾选要扫描入库的相册");
    return;
  }
  if (scanTypes.value.length === 0) {
    notify.warning("请至少勾选一项扫描类型");
    return;
  }
  // 同步校验 + 启动后台循环（扫描循环在 store 中独立存活）
  const ok = contentStore.beginGlobalScan(entries, [...scanTypes.value], batch.value);
  if (ok) {
    notify.info(
      "全局扫描已开始",
      `共 ${entries.length} 个相册，后台执行中；离开本页面扫描不中断`,
      4000,
    );
  } else {
    notify.error("无法开始扫描", job.value.error);
  }
}

function stopScan() {
  if (!running.value) return;
  contentStore.stopGlobalScan();
  notify.info("正在停止", "当前相册完成后停止，已扫描部分仍会入库", 3500);
}

function clearRecords() {
  if (running.value) return;
  contentStore.clearGlobalScan();
}

onMounted(() => {
  // 相册列表为空时拉取（任务状态由 store 持有，无需恢复）
  if (!albumStore.albums.length) {
    albumStore.fetchAlbums().catch(() => {});
  }
});
</script>

<template>
  <section class="gs-area">
    <!-- 顶部说明 + 主控按钮 -->
    <div class="gs-toolbar">
      <p class="gs-desc">
        勾选要扫描入库的相册（支持全选 / 反选），一次批量执行
        <b>EXIF / 影调 / AI 内容识别</b>并写入内容库（与相册详情「组合扫描」同一扫描组件）。
        扫描在<b>后台执行</b>，离开页面不中断；可随时点击「停止」，已扫描部分仍会入库。
      </p>
      <div class="gs-actions">
        <button
          v-if="!running"
          class="btn gs-btn-primary"
          :disabled="albumsLoading"
          @click="startScan"
        >
          ▶ 开始扫描入库
        </button>
        <button v-else class="btn gs-btn-danger" :disabled="stopping" @click="stopScan">
          {{ stopping ? "停止中…" : "■ 停止" }}
        </button>
        <button
          class="btn btn-ghost"
          :disabled="running || !items.length"
          title="清空扫描记录（不影响已入库数据）"
          @click="clearRecords"
        >
          🧹 清除记录
        </button>
      </div>
    </div>

    <!-- 相册勾选区 -->
    <div class="gs-select-block">
      <div class="gs-select-head">
        <label class="gs-check-all" :class="{ disabled: running }">
          <input
            type="checkbox"
            :checked="allSelected"
            :indeterminate.prop="someSelected"
            :disabled="running"
            @change="toggleSelectAll"
          />
          <span>{{ allSelected ? "取消全选" : "全选" }}</span>
        </label>
        <span class="gs-select-count">已选 <b>{{ selectedCount }}</b> / {{ albums.length }} 个相册</span>
        <button class="btn-mini" :disabled="running || !albums.length" @click="invertSelection">反选</button>
        <div class="gs-filter">
          <input
            v-model="filterKeyword"
            class="gs-filter-input"
            placeholder="按名称 / 路径过滤相册…"
            :disabled="running"
          />
        </div>
      </div>
      <div class="gs-album-list">
        <p v-if="!filteredAlbums.length" class="gs-empty-tip">
          {{ albums.length ? "没有匹配的相册" : "暂无相册，请先在「相册管理」中创建" }}
        </p>
        <label
          v-for="a in filteredAlbums"
          :key="a.id"
          class="gs-album-row"
          :class="{ checked: selectedIds.has(a.id), locked: running }"
        >
          <input
            type="checkbox"
            :checked="selectedIds.has(a.id)"
            :disabled="running"
            @change="toggleOne(a.id)"
          />
          <span class="gs-album-name" :title="a.path">{{ a.name }}</span>
          <span class="gs-album-path" :title="a.path">{{ a.path }}</span>
          <span class="gs-album-meta">
            <span class="gs-badge gs-badge-plain">{{ a.photo_count }} 张</span>
            <span
              class="gs-badge"
              :class="a.scanned_photo_count > 0 ? 'gs-badge-ok' : 'gs-badge-todo'"
              :title="a.scanned_photo_count > 0 ? `已入库 ${a.scanned_photo_count}/${a.photo_count} 张` : '尚未入库'"
            >
              {{ a.scanned_photo_count > 0 ? `已入库 ${a.scanned_photo_count}` : "未入库" }}
            </span>
          </span>
        </label>
      </div>
    </div>

    <!-- 扫描设置 -->
    <div class="gs-controls">
      <div class="gs-checks">
        <label class="gs-check" :class="{ active: scanTypes.includes('basic') }">
          <input type="checkbox" value="basic" v-model="scanTypes" :disabled="running" />
          <span class="gs-check-label">EXIF 基础</span>
          <span class="gs-check-desc">ISO / 焦段 / 光圈 / 快门</span>
        </label>
        <label class="gs-check" :class="{ active: scanTypes.includes('tone') }">
          <input type="checkbox" value="tone" v-model="scanTypes" :disabled="running" />
          <span class="gs-check-label">影调分析</span>
          <span class="gs-check-desc">低调 / 中间调 / 高调</span>
        </label>
        <label class="gs-check" :class="{ active: scanTypes.includes('ai') }">
          <input type="checkbox" value="ai" v-model="scanTypes" :disabled="running" />
          <span class="gs-check-label">AI 内容识别</span>
          <span class="gs-check-desc">写入内容库 · 支持智能搜索</span>
        </label>
      </div>
      <label class="batch-select">批次
        <select v-model="batch" :disabled="running">
          <option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }}</option>
        </select>
      </label>
    </div>

    <p v-if="job.error" class="gs-error">{{ job.error }}</p>

    <!-- 进度区：总进度 + 当前相册照片级进度 -->
    <div v-if="items.length" class="gs-progress-block">
      <div class="gs-progress-head">
        <span v-if="running" class="gs-progress-title">🔄 全局扫描中…（{{ finishedCount }} / {{ items.length }} 个相册）</span>
        <span v-else-if="stopping" class="gs-progress-title">⏳ 正在停止…（{{ finishedCount }} / {{ items.length }} 个相册）</span>
        <span v-else class="gs-progress-title">扫描结束</span>
        <span class="gs-progress-percent">{{ overallPercent }}%</span>
      </div>
      <div class="gs-track">
        <div class="gs-fill" :style="{ width: overallPercent + '%' }"></div>
      </div>

      <!-- 当前相册照片级进度（AI 识别实时上报） -->
      <div v-if="running && currentAlbum" class="gs-current">
        <div class="gs-current-head">
          <span class="gs-current-name" :title="currentAlbum.albumName">
            📁 {{ currentAlbum.albumName }}
          </span>
          <span v-if="currentPercent != null" class="gs-current-detail">
            {{ currentProgress!.current }} / {{ currentProgress!.total }} 张
            （成功 {{ currentProgress!.done }} · 失败 {{ currentProgress!.failed }}）
          </span>
          <span v-else class="gs-current-detail">{{ stopping ? "等待收尾…" : "正在扫描…" }}</span>
        </div>
        <div class="gs-track gs-track-inner">
          <div
            class="gs-fill gs-fill-inner"
            :style="{ width: (currentPercent ?? 100) + '%' }"
            :class="{ indeterminate: currentPercent == null }"
          ></div>
        </div>
      </div>

      <!-- 完成汇总 -->
      <p v-if="hasSummary" class="gs-summary" :class="{ 'has-failed': failedCount > 0 }">
        ✅ 全局扫描结束：成功 <b>{{ doneCount }}</b> 个相册
        <template v-if="failedCount">· 失败 <b>{{ failedCount }}</b> 个</template>
        <template v-if="stoppedCount">· 停止剩余 <b>{{ stoppedCount }}</b> 个</template>
        · 累计入库 <b>{{ totalWritten }}</b> 张照片
      </p>

      <!-- 逐相册状态表 -->
      <div class="gs-table-wrap">
        <table class="gs-table">
          <thead>
            <tr>
              <th class="col-idx">#</th>
              <th>相册</th>
              <th class="col-status">状态</th>
              <th class="col-num">入库</th>
              <th class="col-num">失败</th>
              <th>备注</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(it, i) in items" :key="it.albumId" :class="{ 'row-current': i === job.currentIndex }">
              <td class="col-idx">{{ i + 1 }}</td>
              <td class="col-name" :title="it.albumName">{{ it.albumName }}</td>
              <td class="col-status">
                <span class="gs-st" :class="STATUS_META[it.status].cls">{{ STATUS_META[it.status].label }}</span>
              </td>
              <td class="col-num">{{ it.status === "pending" ? "—" : it.written }}</td>
              <td class="col-num" :class="{ 'num-failed': it.failed > 0 }">{{ it.status === "pending" ? "—" : it.failed }}</td>
              <td class="col-err" :title="it.error">{{ it.error || (it.status === "done" ? `共 ${it.total} 张` : "—") }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- 空态 -->
    <div v-if="!items.length" class="gs-empty">
      <p>勾选相册 → 选择扫描类型 → 点击「开始扫描入库」</p>
    </div>
  </section>
</template>

<style scoped>
/* ---- 区域容器（与 ScanPanel 白底卡片风格一致） ---- */
.gs-area {
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  background: #fff;
  overflow: hidden;
}

.gs-toolbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 14px 10px;
  flex-wrap: wrap;
}

.gs-desc {
  font-size: 12px;
  color: #667085;
  margin: 0;
  max-width: 640px;
  line-height: 1.6;
}

.gs-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

/* ---- 按钮（与 ScanPanel 统一） ---- */
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid #ddd;
  background: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}
.btn:hover { border-color: #396cd8; color: #396cd8; }
.btn:disabled { opacity: 0.55; cursor: not-allowed; }
.gs-btn-primary { background: #396cd8; color: #fff; border-color: #396cd8; }
.gs-btn-primary:hover { background: #2f5cc2; color: #fff; }
.gs-btn-danger { background: #e5484d; color: #fff; border-color: #e5484d; }
.gs-btn-danger:hover { background: #cf3e43; color: #fff; }
.btn-ghost { background: transparent; color: #396cd8; border-color: #396cd8; }
.btn-ghost:hover { background: rgba(57, 108, 216, 0.08); color: #2f5cc2; }
.btn-mini { padding: 2px 8px; font-size: 11px; border: 1px solid #d0d5dd; border-radius: 3px; background: #fff; cursor: pointer; }
.btn-mini:hover { border-color: #396cd8; color: #396cd8; }
.btn-mini:disabled { opacity: 0.5; cursor: not-allowed; }

/* ---- 相册勾选区 ---- */
.gs-select-block {
  margin: 0 14px;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  background: #f8f9fa;
}

.gs-select-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 10px;
  border-bottom: 1px solid #e5e7eb;
  flex-wrap: wrap;
}

.gs-check-all {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  user-select: none;
}
.gs-check-all.disabled { opacity: 0.6; cursor: not-allowed; }

.gs-select-count { font-size: 12px; color: #667085; }
.gs-select-count b { color: #396cd8; }

.gs-filter { margin-left: auto; }
.gs-filter-input {
  width: 200px;
  height: 28px;
  padding: 0 8px;
  font-size: 12px;
  border: 1px solid #d0d5dd;
  border-radius: 4px;
  background: #fff;
  outline: none;
}
.gs-filter-input:focus { border-color: #396cd8; }

.gs-album-list {
  max-height: 260px;
  overflow-y: auto;
}

.gs-empty-tip {
  padding: 18px 10px;
  text-align: center;
  font-size: 12px;
  color: #98a2b3;
  margin: 0;
}

.gs-album-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  border-bottom: 1px solid #eef0f4;
  cursor: pointer;
  transition: background 0.12s;
  font-size: 13px;
}
.gs-album-row:last-child { border-bottom: none; }
.gs-album-row:hover { background: #eef3fb; }
.gs-album-row.checked { background: #eef3fb; }
.gs-album-row.locked { cursor: default; }
.gs-album-row input[type="checkbox"] { flex: none; }

.gs-album-name {
  flex: none;
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
  color: var(--color-text);
}

.gs-album-path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11px;
  color: #98a2b3;
}

.gs-album-meta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex: none;
}

.gs-badge {
  display: inline-block;
  padding: 1px 8px;
  border-radius: 10px;
  font-size: 11px;
  white-space: nowrap;
}
.gs-badge-plain { background: #eef1f6; color: #667085; }
.gs-badge-ok { background: #e6f6ea; color: #15803d; }
.gs-badge-todo { background: #fdf3e0; color: #b45309; }

/* ---- 扫描设置 ---- */
.gs-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: center;
  padding: 12px 14px;
}

.gs-checks { display: flex; gap: 10px; flex-wrap: wrap; flex: 1; min-width: 0; }
.gs-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border: 1px solid #d0d5dd;
  border-radius: 6px;
  cursor: pointer;
  transition: all 0.15s;
  background: #fff;
}
.gs-check:hover { border-color: #396cd8; background: #eef3fb; }
.gs-check.active { border-color: #396cd8; background: #eef3fb; }
.gs-check input:disabled { cursor: not-allowed; }
.gs-check-label { font-weight: 500; font-size: 13px; }
.gs-check-desc { font-size: 11px; color: #667085; }

.batch-select {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: #667085;
}
.batch-select select {
  padding: 2px 4px;
  border: 1px solid #d0d5dd;
  border-radius: 4px;
  font-size: 12px;
  background: #fff;
}

/* ---- 错误 / 进度 ---- */
.gs-error {
  margin: 0;
  padding: 8px 14px;
  color: #e5484d;
  font-size: 13px;
  background: #fef2f2;
}

.gs-progress-block { padding: 10px 14px 14px; }

.gs-progress-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 6px;
}
.gs-progress-title { font-size: 13px; font-weight: 600; color: var(--color-text); }
.gs-progress-percent { font-size: 13px; color: #396cd8; font-weight: 600; }

.gs-track {
  height: 8px;
  background: #e5e7eb;
  border-radius: 4px;
  overflow: hidden;
}
.gs-fill {
  height: 100%;
  background: linear-gradient(90deg, #396cd8, #5a8bf7);
  transition: width 0.3s;
  border-radius: 4px;
}
.gs-track-inner { height: 6px; margin-top: 6px; }
.gs-fill-inner { background: #667085; }

/* 不确定进度（无照片级事件时往复动画） */
.gs-fill.indeterminate {
  width: 40% !important;
  animation: gs-indet 1.4s ease-in-out infinite;
}
@keyframes gs-indet {
  0% { margin-left: -40%; }
  100% { margin-left: 100%; }
}

.gs-current {
  margin-top: 12px;
  padding: 8px 10px;
  background: #f8f9fa;
  border: 1px solid #eef0f4;
  border-radius: 6px;
}
.gs-current-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  margin-bottom: 4px;
  flex-wrap: wrap;
}
.gs-current-name {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text);
  max-width: 60%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.gs-current-detail { font-size: 11px; color: #667085; }

.gs-summary {
  margin: 12px 0 0;
  padding: 8px 10px;
  font-size: 13px;
  color: #15803d;
  background: #e6f6ea;
  border-radius: 6px;
}
.gs-summary.has-failed { color: #b45309; background: #fdf3e0; }

/* ---- 逐相册状态表 ---- */
.gs-table-wrap {
  margin-top: 12px;
  max-height: 300px;
  overflow-y: auto;
  border: 1px solid #eef0f4;
  border-radius: 6px;
}

.gs-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}

.gs-table th {
  text-align: left;
  padding: 6px 8px;
  background: rgba(120, 130, 150, 0.08);
  border-bottom: 2px solid var(--color-border);
  font-weight: 600;
  color: var(--color-text);
  white-space: nowrap;
  position: sticky;
  top: 0;
  z-index: 1;
}

.gs-table td {
  padding: 5px 8px;
  border-bottom: 1px solid #f0f0f0;
  vertical-align: middle;
}

.gs-table tr:hover td { background: #fafbfc; }
.gs-table tr.row-current td { background: #eef3fb; }

.col-idx { width: 36px; text-align: center; color: var(--color-text-2); }
.col-name { max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
.col-status { width: 76px; }
.col-num { width: 60px; text-align: center; }
.num-failed { color: #e5484d; font-weight: 600; }
.col-err { max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #98a2b3; }

.gs-st {
  display: inline-block;
  padding: 1px 8px;
  border-radius: 10px;
  font-size: 11px;
  white-space: nowrap;
}
.gs-st.st-pending { background: #eef1f6; color: #667085; }
.gs-st.st-running { background: #e0e9fa; color: #396cd8; animation: gs-pulse 1.5s ease-in-out infinite; }
.gs-st.st-done { background: #e6f6ea; color: #15803d; }
.gs-st.st-failed { background: #fef2f2; color: #e5484d; }
.gs-st.st-stopped { background: #fdf3e0; color: #b45309; }

@keyframes gs-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

.gs-empty {
  padding: 22px 14px;
  text-align: center;
  color: #98a2b3;
  font-size: 13px;
}
.gs-empty p { margin: 0; }
</style>
