<script setup lang="ts">
/**
 * 模型下载列表（FEAT-052 / FEAT-114）
 *
 * 纯展示 + 操作，不含标题/外框：设置页与「模型缺失」弹窗共用同一份列表，
 * 避免两处各写一套进度/重试逻辑而漂移。
 *
 * FEAT-114 起 Rust 侧下载表覆盖 语义两档 + 人脸两档 + OCR（此前只有 CLIP），
 * 因此每行展示 label（人类可读名）与 note（体积/取舍），而不是裸模型 id。
 */
import { computed, onMounted, ref, watch } from "vue";import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import type { ModelDlStatus } from "../../types/content";

const store = useContentStore();
const notify = useNotify();

const downloads = computed(() => store.modelDownloads);
const allReady = computed(() => downloads.value.length > 0 && downloads.value.every((d) => d.done));
const missing = computed(() => downloads.value.filter((d) => !d.done));
const missingBytes = computed(() =>
  missing.value.reduce((sum, d) => sum + Math.max(0, d.total - d.bytes), 0),
);
const anyRunning = computed(() => downloads.value.some((d) => d.running));

onMounted(() => void store.listModelDownloads());

// 某个模型刚下完 → 刷新语义档位清单，让「未下载」的选项立刻可选
const prevDone = ref(new Set<string>());
watch(
  () => store.modelDownloads,
  (list) => {
    for (const d of list) {
      if (d.done && !prevDone.value.has(d.name)) {
        prevDone.value.add(d.name);
        void store.fetchVcrModels();
      }
      if (!d.done) prevDone.value.delete(d.name);
    }
  },
  { deep: true },
);

function progressPct(d: ModelDlStatus): number {
  if (d.stage === "exporting") return 100;
  if (!d.total) return 0;
  return Math.min(100, Math.round((d.bytes / d.total) * 100));
}
function fmtMB(bytes: number): string {
  return `${(bytes / 1e6).toFixed(1)} MB`;
}
function fmtBytes(d: ModelDlStatus): string {
  return `${fmtMB(d.bytes)}${d.total ? ` / ${fmtMB(d.total)}` : ""}`;
}
function startDl(name: string) {
  store.startModelDownload(name).catch((e) => notify.error("启动下载失败", String(e)));
}
function cancelDl(name: string) {
  store.cancelModelDownload(name).catch((e) => notify.error("取消失败", String(e)));
}

/** 一键补全：只挑缺失且没在跑的，避免重复点击把已在下载的任务顶掉 */
async function downloadAllMissing() {
  for (const d of missing.value) {
    if (d.running) continue;
    try {
      await store.startModelDownload(d.name);
    } catch (e) {
      notify.error(`启动 ${d.label || d.name} 下载失败`, String(e));
    }
  }
}

defineExpose({ allReady, missing, downloadAllMissing });
</script>

<template>
  <div class="mdr">
    <div class="mdr-toolbar">
      <p v-if="allReady" class="hint ok">✓ 本地模型已全部就绪，无需再下载。</p>
      <p v-else class="hint">
        缺失 {{ missing.length }} 项 · 共 {{ fmtMB(missingBytes) }} · 支持断点续传与多镜像择优
      </p>
      <button
        v-if="!allReady"
        class="btn btn-primary"
        type="button"
        :disabled="anyRunning && missing.every((d) => d.running)"
        @click="downloadAllMissing"
      >
        ⬇ 一键下载全部缺失
      </button>
    </div>

    <div class="dl-list">
      <div v-for="d in downloads" :key="d.name" class="dl-row">
        <span class="dl-name">
          <b>{{ d.label || d.name }}</b>
          <span v-if="d.required" class="dl-tag">必需</span>
          <em class="dl-note">{{ d.note }}</em>
        </span>

        <template v-if="d.done">
          <span class="stat ok">✓ 已下载</span>
        </template>
        <template v-else-if="d.running">
          <div class="dl-progress" role="progressbar" :aria-valuenow="progressPct(d)">
            <div class="dl-fill" :style="{ width: progressPct(d) + '%' }"></div>
          </div>
          <span class="stat dim">{{ d.stage === "exporting" ? "导出中…" : fmtBytes(d) }}</span>
          <button class="btn btn-sm" @click="cancelDl(d.name)">取消</button>
        </template>
        <template v-else-if="d.stage === 'error'">
          <span class="stat err" :title="d.error ?? ''">失败：{{ (d.error ?? "").slice(0, 60) }}</span>
          <button class="btn btn-sm" @click="startDl(d.name)">重试</button>
        </template>
        <template v-else>
          <button class="btn btn-sm" @click="startDl(d.name)">⬇ 下载</button>
        </template>
      </div>
      <p v-if="!downloads.length" class="hint">暂无待下载模型。</p>
    </div>
  </div>
</template>

<style scoped>
.mdr {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.mdr-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.dl-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.dl-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  padding: 8px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
}
.dl-name {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 230px;
  font-size: 13px;
  color: var(--color-text);
}
.dl-note {
  font-size: 11px;
  font-style: normal;
  color: var(--color-text-3);
}
.dl-tag {
  align-self: flex-start;
  margin-top: 2px;
  padding: 1px 6px;
  border-radius: 999px;
  background: var(--color-neutral-soft);
  color: var(--color-text-2);
  font-size: 11px;
}
.dl-progress {
  flex: 1;
  min-width: 120px;
  max-width: 220px;
  height: 8px;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.18);
  overflow: hidden;
}
.dl-fill {
  height: 100%;
  background: linear-gradient(90deg, var(--color-primary), #5a8ce8);
  transition: width var(--dur-3) ease;
}
.hint {
  margin: 0;
  font-size: 12px;
  line-height: 1.7;
  color: var(--color-text-3);
}
.hint.ok {
  color: var(--color-ok);
}
.stat {
  font-size: 12.5px;
  color: var(--color-text-2);
}
.stat.ok {
  color: var(--color-ok);
}
.stat.dim {
  color: var(--color-text-3);
}
.stat.err {
  color: var(--color-danger);
}
.btn {
  min-height: 26px;
  padding: 3px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
  color: var(--color-text);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.btn:hover:not(:disabled) {
  border-color: var(--color-primary);
  color: var(--color-primary-text);
}
.btn:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--color-primary) 55%, transparent);
  outline-offset: 2px;
}
.btn:disabled {
  background: var(--color-neutral-soft);
  color: var(--color-text-3);
  cursor: not-allowed;
}
.btn-primary {
  background: var(--color-primary);
  border-color: transparent;
  color: var(--color-on-primary, #fff);
}
.btn-primary:hover:not(:disabled) {
  color: var(--color-on-primary, #fff);
  filter: brightness(1.06);
}
</style>
