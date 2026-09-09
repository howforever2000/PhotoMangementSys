<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";
import { useContentStore } from "../stores/content";
import type { VcrGpuStatus } from "../types/content";
import type { PersonInfo } from "../types/photo";
import { trace } from "../utils/trace";
import { categoryLabel } from "../utils/categoryLabel";
import PersonPanel from "./PersonPanel.vue";
import { useNotify } from "../composables/useNotify";

const props = defineProps<{ albumId: number; albumPath: string }>();
const contentStore = useContentStore();
const notify = useNotify();

// ===================== 组合扫描（FEAT-026，统一入口） =====================
const toneLabelMap: Record<string, string> = {
  "low-key": "低调", "mid-key": "中间调", "high-key": "高调",
  LowKey: "低调", MidKey: "中间调", HighKey: "高调",
};

const comboScanTypes = ref<string[]>([]);
const comboBatch = ref(8);
const BATCH_OPTIONS = [8, 16, 32];
// 任务状态来自全局 store（键 = albumId），脱离组件存活：
// 退出相册页后后端继续扫描，重新进入仍能看到进度与结果 → 支持后台工作
const job = computed(() => contentStore.jobFor(props.albumId));
const comboScanning = computed(() => job.value.running);
const comboError = computed(() => job.value.error);
const comboReport = computed(() => job.value.report);
const comboRows = computed(() => job.value.rows);
const comboProgress = computed(() => job.value.progress);

onMounted(() => {
  // 重新进入时恢复之前勾选的类型（若仍在扫描则不中断）
  if (job.value.types.length) comboScanTypes.value = [...job.value.types];
});

// 扫描结束后刷新人物列表与 GPU 状态（同人标号可能因本次识别变化）
watch(
  () => job.value.running,
  (now, prev) => {
    if (prev && !now && job.value.report) {
      loadPersons();
      fetchGpuStatus();
    }
  },
);

// ---- 分页（读表与扫描内容共用，10/20/50 每页，默认 10） ----
const PAGE_SIZES = [10, 20, 50];
const pageSize = ref(10);
const currentPage = ref(1);

const pageCount = computed(() =>
  Math.max(1, Math.ceil(comboRows.value.length / pageSize.value)),
);
const pagedRows = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value;
  return comboRows.value.slice(start, start + pageSize.value);
});

watch(pageSize, () => { currentPage.value = 1; });
watch(comboRows, () => {
  if (currentPage.value > pageCount.value) currentPage.value = pageCount.value;
});

function changePage(p: number) {
  currentPage.value = Math.min(Math.max(1, p), pageCount.value);
}

function startScan() {
  if (comboScanTypes.value.length === 0) {
    job.value.error = "请至少勾选一项扫描类型（支持单选项）";
    return;
  }
  // 至少勾选一项即可扫描（单选项亦可）；后台异步执行
  contentStore.startCombinedScan(props.albumId, comboScanTypes.value, comboBatch.value);
}

function stopScan() {
  contentStore.stopCombinedScan(props.albumId);
}

// ---- GPU 加速状态（仅展示，不参与扫描逻辑） ----
const gpuStatus = ref<VcrGpuStatus | null>(null);
const gpuLoading = ref(false);
async function fetchGpuStatus() {
  gpuLoading.value = true;
  try {
    gpuStatus.value = await contentStore.fetchGpuStatus();
  } catch {
    gpuStatus.value = null;
  } finally {
    gpuLoading.value = false;
  }
}

// ---- 读表：把已入库内容读出展示 ----
const readScanning = ref(false);
const readAlbumContent = trace("readAlbumContent", async () => {
  if (readScanning.value) return;
  readScanning.value = true;
  try {
    const { rows } = await contentStore.readAlbumContent(props.albumId, 1, 100000);
    job.value.rows = rows.map((r) => ({
      file_name: r.path.split("/").pop()?.split("\\").pop() ?? r.path,
      path: r.path,
      iso: r.iso, aperture: r.aperture, shutter_speed: r.shutter_speed,
      focal_length: r.focal_length, shoot_time: r.shoot_time,
      iso_num: r.iso_num, focal_num: r.focal_num,
      aperture_num: r.aperture_num, shutter_num: r.shutter_num,
      tone_type: r.tone_type, avg_luma: r.avg_luma,
      category: r.category, sub_category: r.sub_category,
      label: r.label, confidence: r.confidence,
      top3: [] as { category: string; label: string; confidence: number }[],
      person_ids: r.person_ids, person_count: r.person_count,
    }));
    currentPage.value = 1;
  } catch (e) {
    job.value.error = `读表失败：${e}`;
  } finally {
    readScanning.value = false;
  }
});

// ---- 人物管理 ----
const persons = ref<PersonInfo[]>([]);
const loadPersons = trace("loadPersons", async () => {
  try {
    persons.value = await invoke<PersonInfo[]>("list_persons");
  } catch {
    persons.value = [];
  }
});

const openImage = trace("openImage", async (path: string) => {
  try {
    await openPath(path);
  } catch (e) {
    notify.error("无法打开图片", `${path}\n${e}`);
  }
});
</script>

<template>
  <!-- ============ 组合扫描（FEAT-026，统一 EXIF / 影调 / AI 入口，可折叠） ============ -->
  <section class="scan-area combo-area">
    <div class="scan-toolbar">
      <div class="combo-title-wrap">
        <p class="scan-sub">勾选扫描类型（可多选，至少一项即可），一次完成 EXIF / 影调 / AI 内容识别；勾选「内容识别」时结果同时写入内容库，可用于智能搜索。扫描在后台执行，退出相册页不中断，可随时点击「停止」结束</p>
      </div>
      <div class="combo-actions">
        <button class="btn btn-primary" :disabled="comboScanning" @click="startScan">
          {{ comboScanning ? "扫描中…" : "开始组合扫描" }}
        </button>
        <button v-if="comboScanning" class="btn btn-danger" @click="stopScan">
          停止
        </button>
        <button class="btn btn-ghost" :disabled="readScanning" @click="readAlbumContent" title="把已扫描入库的内容读出到表格展示">
          {{ readScanning ? "读取中…" : "📖 读表" }}
        </button>
      </div>
    </div>

    <!-- 折叠由外层 CollapseSection 统一负责 -->
    <div>
      <div class="combo-controls">
        <div class="combo-checks">
          <label class="combo-check" :class="{ active: comboScanTypes.includes('basic') }">
            <input type="checkbox" value="basic" v-model="comboScanTypes" />
            <span class="combo-check-label">EXIF 基础</span>
            <span class="combo-check-desc">ISO / 焦段 / 光圈 / 快门</span>
          </label>
          <label class="combo-check" :class="{ active: comboScanTypes.includes('tone') }">
            <input type="checkbox" value="tone" v-model="comboScanTypes" />
            <span class="combo-check-label">影调分析</span>
            <span class="combo-check-desc">低调 / 中间调 / 高调</span>
          </label>
          <label class="combo-check" :class="{ active: comboScanTypes.includes('ai') }">
            <input type="checkbox" value="ai" v-model="comboScanTypes" />
            <span class="combo-check-label">AI 内容识别</span>
            <span class="combo-check-desc">写入内容库 · 支持搜索</span>
          </label>
        </div>
        <div class="combo-meta">
          <label class="batch-select">批次<select v-model="comboBatch"><option v-for="b in BATCH_OPTIONS" :key="b" :value="b">{{ b }}</option></select></label>
          <button class="btn btn-mini" :disabled="gpuLoading" @click="fetchGpuStatus">{{ gpuLoading ? "检测中…" : "检测 GPU" }}</button>
          <div v-if="gpuStatus" class="gpu-info" :class="{ ok: gpuStatus.use_gpu }">GPU: {{ gpuStatus.use_gpu ? "✅ " + gpuStatus.provider : "❌ CPU (" + gpuStatus.provider + ")" }}</div>
        </div>
      </div>

      <p v-if="comboError" class="scan-error">{{ comboError }}</p>

      <!-- AI 识别实时进度 -->
      <div v-if="comboScanning && comboProgress && comboProgress.total > 0" class="combo-progress">
        <div class="progress-track"><div class="progress-fill" :style="{ width: (comboProgress.current / comboProgress.total) * 100 + '%' }"></div></div>
        <p class="progress-text">{{ comboProgress.current }} / {{ comboProgress.total }} 张（成功 {{ comboProgress.done }} · 失败 {{ comboProgress.failed }}）</p>
      </div>

      <div v-if="!comboScanning && !comboRows.length && !comboError" class="scan-empty"><p>请勾选扫描类型后点击「开始组合扫描」（至少一项即可）</p></div>
      <div v-if="comboScanning && !comboRows.length" class="scan-empty"><p class="scan-loading">⏳ 正在扫描… 请稍候{{ comboProgress && comboProgress.total > 0 ? `（${comboProgress.current}/${comboProgress.total}）` : "" }}</p></div>

      <div v-if="comboRows.length" class="scan-table-wrap">
        <div class="scan-toolbar" style="padding:10px 14px 0;border:none">
          <p class="scan-sub">{{ comboReport ? "✅ 已写入内容库 " + comboReport.written + "/" + comboReport.total + " 张" : "✅ 扫描完成" }}</p>
          <div class="scan-actions" style="display:flex;gap:8px;flex-wrap:wrap">
            <button class="btn btn-primary" :disabled="comboScanning" @click="startScan">{{ comboScanning ? "扫描中…" : "重新扫描" }}</button>
            <button v-if="comboScanning" class="btn btn-danger" @click="stopScan">停止</button>
          </div>
        </div>
        <table class="scan-table combo-table">
          <thead><tr><th class="col-idx">#</th><th>照片名字</th><th>ISO</th><th>焦段</th><th>光圈</th><th>快门</th><th>影调</th><th>AI 类别</th><th>细类</th></tr></thead>
          <tbody>
            <tr v-for="(r, i) in pagedRows" :key="r.path">
              <td class="col-idx">{{ (currentPage - 1) * pageSize + i + 1 }}</td>
              <td class="col-name" :title="r.path"><span class="img-name-link" @click="openImage(r.path)">{{ r.file_name }}</span></td>
              <td>{{ r.iso ?? r.iso_num != null ? "ISO " + r.iso_num : "—" }}</td>
              <td>{{ r.focal_length ?? r.focal_num != null ? r.focal_num + "mm" : "—" }}</td>
              <td>{{ r.aperture ?? r.aperture_num != null ? "f/" + r.aperture_num : "—" }}</td>
              <td>{{ r.shutter_speed ?? (r.shutter_num ? (1 / r.shutter_num).toFixed(1) + "s" : "—") }}</td>
              <td><span v-if="r.tone_type" class="tone-badge tone-" :class="r.tone_type">{{ toneLabelMap[r.tone_type] || r.tone_type }}</span><span v-else>—</span></td>
              <td>{{ r.category ? categoryLabel(r.category) : "—" }}</td>
              <td>{{ r.label ?? "—" }}</td>
            </tr>
          </tbody>
        </table>
        <!-- 分页 -->
        <div class="vision-pager">
          <label class="pager-size">每页<select v-model="pageSize"><option v-for="s in PAGE_SIZES" :key="s" :value="s">{{ s }}</option></select>条</label>
          <div class="pager-nav">
            <button class="btn-mini" :disabled="currentPage <= 1" @click="changePage(currentPage - 1)">上一页</button>
            <span class="pager-info">第 {{ currentPage }} / {{ pageCount }} 页 · 共 {{ comboRows.length }} 张</span>
            <button class="btn-mini" :disabled="currentPage >= pageCount" @click="changePage(currentPage + 1)">下一页</button>
          </div>
        </div>
      </div>
    </div>
  </section>

  <!-- ============ 人物管理 ============ -->
  <section class="scan-area">
    <PersonPanel :persons="persons" @refresh="loadPersons" />
  </section>
</template>

<style scoped>
/* ---- 通用扫描区 ---- */
.scan-area {
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  margin-bottom: 20px;
  background: #fff;
}

.scan-toolbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 14px 10px;
  flex-wrap: wrap;
}

.scan-title {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
  color: var(--color-text);
}

.scan-sub {
  font-size: 12px;
  color: var(--color-text-2);
  margin: 4px 0 0 0;
  max-width: 640px;
  line-height: 1.5;
}

.combo-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.combo-title-wrap {
  display: flex;
  align-items: flex-start;
  gap: 8px;
}

.combo-collapse {
  background: transparent;
  border: none;
  cursor: pointer;
  color: #667085;
  padding: 2px;
  border-radius: 4px;
  margin-top: 2px;
  flex: none;
}

.combo-collapse:hover {
  background: var(--color-primary-soft, #eef1f6);
  color: var(--color-text);
}

.collapse-icon {
  width: 16px;
  height: 16px;
  display: block;
}

.scan-error {
  padding: 8px 14px;
  color: #e5484d;
  font-size: 13px;
  background: #fef2f2;
  margin: 0;
}

.scan-empty {
  padding: 24px 14px;
  text-align: center;
  color: #667085;
  font-size: 13px;
}

.scan-loading {
  animation: pulse 1.5s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 0.5; }
  50% { opacity: 1; }
}

.scan-table-wrap {
  padding: 0 14px 14px;
}

.scan-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  margin-top: 8px;
}

.scan-table th {
  text-align: left;
  padding: 6px 8px;
  background: rgba(120, 130, 150, 0.08);
  border-bottom: 2px solid var(--color-border);
  font-weight: 600;
  color: var(--color-text);
  white-space: nowrap;
}

.scan-table td {
  padding: 5px 8px;
  border-bottom: 1px solid var(--color-border);
  vertical-align: middle;
  color: var(--color-text);
}

.scan-table tr:hover td {
  background: rgba(57, 108, 216, 0.06);
}

.col-idx {
  width: 32px;
  text-align: center;
  color: var(--color-text-2);
}

.col-name {
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 图片名可点击 */
.img-name-link {
  color: #396cd8;
  cursor: pointer;
  text-decoration: underline dotted;
}

.img-name-link:hover {
  color: #2f5cc2;
}

/* 批次选择 */
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

/* 按钮（通用） */
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
.btn:disabled { opacity: 0.6; cursor: not-allowed; }
.btn-primary { background: #396cd8; color: #fff; border-color: #396cd8; }
.btn-primary:hover { background: #2f5cc2; color: #fff; }
.btn-danger { background: #e5484d; color: #fff; border-color: #e5484d; }
.btn-danger:hover { background: #cf3e43; color: #fff; }
.btn-ghost { background: transparent; color: #396cd8; border-color: #396cd8; }
.btn-ghost:hover { background: rgba(57, 108, 216, 0.08); color: #2f5cc2; }
.btn-mini { padding: 2px 6px; font-size: 11px; border: 1px solid #d0d5dd; border-radius: 3px; background: #fff; cursor: pointer; }
.btn-mini:disabled { opacity: 0.5; cursor: not-allowed; }

/* ---- 组合扫描 ---- */
.combo-area { margin-bottom: 24px; }
.combo-controls { display: flex; flex-wrap: wrap; gap: 12px; padding: 12px 14px; background: #f8f9fa; border-radius: 6px; margin-bottom: 12px; }
.combo-checks { display: flex; gap: 10px; flex-wrap: wrap; flex: 1; min-width: 0; }
.combo-check { display: inline-flex; align-items: center; gap: 6px; padding: 6px 10px; border: 1px solid #d0d5dd; border-radius: 6px; cursor: pointer; transition: all 0.15s; }
.combo-check:hover { border-color: #396cd8; background: #eef3fb; }
.combo-check.active { border-color: #396cd8; background: #eef3fb; }
.combo-check input[type="checkbox"] { margin-right: 2px; }
.combo-check-label { font-weight: 500; font-size: 13px; }
.combo-check-desc { font-size: 11px; color: #667085; }
.combo-meta { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.gpu-info { font-size: 12px; color: #667085; }
.gpu-info.ok { color: #15803d; }
.combo-table th:nth-child(7), .combo-table td:nth-child(7) { width: 80px; }
.combo-table th:nth-child(8), .combo-table td:nth-child(8) { width: 120px; }
.combo-table th:nth-child(9), .combo-table td:nth-child(9) { width: 180px; }

/* 进度条 */
.combo-progress { padding: 8px 14px; }
.progress-track { height: 6px; background: #e5e7eb; border-radius: 3px; overflow: hidden; }
.progress-fill { height: 100%; background: #396cd8; transition: width 0.3s; border-radius: 3px; }
.progress-text { font-size: 11px; color: #667085; margin: 4px 0 0; }

/* 影调徽标（组合表用） */
.tone-badge { display: inline-block; padding: 1px 6px; border-radius: 3px; font-size: 11px; }
.tone-badge.tone-low-key, .tone-badge.tone-low_key { background: #2a2a2a; color: #fff; }
.tone-badge.tone-mid-key, .tone-badge.tone-mid_key { background: #667085; color: #fff; }
.tone-badge.tone-high-key, .tone-badge.tone-high_key { background: #e5e7eb; color: #1f2328; }

/* 分页 */
.vision-pager { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-top: 8px; flex-wrap: wrap; }
.pager-size { display: flex; align-items: center; gap: 4px; font-size: 12px; color: #667085; }
.pager-size select { padding: 2px 4px; border: 1px solid #d0d5dd; border-radius: 4px; font-size: 12px; }
.pager-nav { display: flex; align-items: center; gap: 6px; }
.pager-info { font-size: 12px; color: #667085; }
</style>
