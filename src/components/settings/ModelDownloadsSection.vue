<script setup lang="ts">
/**
 * 📦 模型管理 · 下载（FEAT-052）
 *
 * 后台下载 + 进度 + 官方/镜像择优；.pt 下载后自动导出 onnx。
 * **不依赖识别服务** —— 服务没起来也能下（与①②③区的锁定逻辑无关）。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useContentStore } from "../../stores/content";
import { useNotify } from "../../composables/useNotify";
import type { ModelDlStatus } from "../../types/content";

const store = useContentStore();
const notify = useNotify();

const downloads = computed(() => store.modelDownloads);
/** 全部就绪 → 顶部一句绿字，让用户知道不用再管 */
const allReady = computed(() => downloads.value.length > 0 && downloads.value.every((d) => d.done));

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
function fmtBytes(d: ModelDlStatus): string {
  const b = (d.bytes / 1e6).toFixed(1);
  const t = d.total ? ` / ${(d.total / 1e6).toFixed(1)} MB` : "";
  return `${b} MB${t}`;
}
function startDl(name: string) {
  store.startModelDownload(name).catch((e) => notify.error("启动下载失败", String(e)));
}
function cancelDl(name: string) {
  store.cancelModelDownload(name).catch((e) => notify.error("取消失败", String(e)));
}
</script>

<template>
  <div class="dl-head">
    <h4 class="sub-title">📥 模型下载</h4>
    <span class="hint">
      多镜像候选（自定义优先）逐个尝试，失败自动换源 · 支持断点续传 · .pt 下载后自动导出 onnx
    </span>
  </div>

  <p v-if="allReady" class="hint ok">✓ 必需模型已全部就绪，无需再下载。</p>

  <div class="dl-list">
    <div v-for="d in downloads" :key="d.name" class="dl-row">
      <span class="dl-name">
        {{ d.name }}<span v-if="d.required" class="dl-tag">必需</span>
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
</template>

<style scoped>
.dl-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.sub-title {
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--color-text);
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
  padding: 7px 10px;
  border: 1px solid var(--color-border);
  border-radius: var(--radius-sm, 8px);
  background: var(--color-surface-2);
}
.dl-name {
  min-width: 190px;
  font-size: 13px;
  color: var(--color-text);
}
.dl-tag {
  margin-left: 6px;
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
  transition: width 0.25s ease;
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
  color: var(--color-primary);
}
.btn:focus-visible {
  outline: 2px solid rgba(57, 108, 216, 0.55);
  outline-offset: 2px;
}
.btn:disabled {
  background: var(--color-neutral-soft);
  color: var(--color-text-3);
  cursor: not-allowed;
}
</style>
