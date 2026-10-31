<script setup lang="ts">
/**
 * 重复照片清理（FEAT-143）
 *
 * 全库扫描 → 分组呈现 → 逐张勾选 → 两档删除。
 *
 * 页内结构（自上而下）：
 *   ① 判据与阈值（一次说清「怎么判重复」）
 *   ② 进度 / 错误 / 能力提示（HEIC、未入库）
 *   ③ 汇总带（找到多少组、能省多少）
 *   ④ 分组列表（分页，每页 12 组；缩略图走全局排队）
 *   ⑤ 底部常驻操作条（已选张数/体积 + 全选 + 两档删除）
 *
 * 删除决策全在 store（`useDedupeStore`）：本组件只负责呈现与二次确认。
 */
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useDedupeStore, thresholdHint } from "../stores/dedupe";
import { useNotify } from "../composables/useNotify";
import { getThumbs } from "../utils/thumbQueue";
import { formatSize } from "../types/album";
import type { DedupeDeleteMode } from "../types/dedupe";
import DupeGroupCard from "./dedupe/DupeGroupCard.vue";
import ConfirmDialog from "./ConfirmDialog.vue";
import EmptyState from "./EmptyState.vue";

const store = useDedupeStore();
const notify = useNotify();
const router = useRouter();

/* ---------------- 缩略图（按当前页取，走全局排队） ---------------- */
const thumbs = ref<Record<string, string>>({});

async function loadThumbs() {
  const wanted = new Map<number, string[]>();
  for (const g of store.pageGroups) {
    for (const it of g.items) {
      if (thumbs.value[it.path]) continue;
      const list = wanted.get(it.album_id) ?? [];
      list.push(it.path);
      wanted.set(it.album_id, list);
    }
  }
  for (const [albumId, paths] of wanted) {
    try {
      await getThumbs(albumId, paths, (path, thumb) => {
        thumbs.value = { ...thumbs.value, [path]: thumb };
      });
    } catch {
      /* 缩略图取不到不阻塞勾选：卡片显示首字占位 */
    }
  }
}

watch(
  () => store.pageGroups.map((g) => g.key).join("|"),
  () => {
    void loadThumbs();
  },
  { immediate: true },
);

onMounted(() => {
  void store.refreshHeicSupport();
});

/* ---------------- 参数联动 ---------------- */
const hint = computed(() => thresholdHint(store.threshold));

const stageLabel = computed(() => {
  switch (store.stage) {
    case "collect":
      return "遍历相册目录";
    case "hash":
      return "比对文件内容";
    case "similar":
      return "向量相似聚类";
    case "done":
      return "完成";
    default:
      return "准备中";
  }
});

const progressPercent = computed(() => {
  if (store.progressTotal <= 0) return 0;
  return Math.min(100, Math.round((store.progressDone / store.progressTotal) * 100));
});

/* ---------------- 结果汇总 ---------------- */
const report = computed(() => store.report);
const groups = computed(() => store.groups);
const hasResult = computed(() => !!report.value);
const canScan = computed(() => !store.running && !store.deleting);
const needVectorHint = computed(
  () => !!report.value && (report.value.skipped_unencoded > 0 || !!report.value.similar_skipped_reason),
);

function start() {
  store
    .scan()
    .then(() => {
      const r = store.report;
      if (!r) return;
      if (!r.groups.length) {
        notify.info("扫描完成", "没有发现重复照片，可以调低阈值再试");
      } else {
        notify.success(
          "扫描完成",
          `找到 ${r.groups.length} 组重复（完全重复 ${r.exact_groups} · 画面相似 ${r.similar_groups}）`,
        );
      }
    })
    .catch((e) => notify.error("扫描失败", String(e)));
}

/* ---------------- 删除（两档） ---------------- */
const pendingMode = ref<DedupeDeleteMode | null>(null);

const dialogTitle = computed(() =>
  pendingMode.value === "trash" ? "移入回收站" : pendingMode.value === "records" ? "仅删除记录" : "",
);

const dialogMessage = computed(() => {
  const n = store.selectedCount;
  const size = formatSize(store.selectedBytes);
  if (pendingMode.value === "trash") {
    return `将把已勾选的 ${n} 张照片（共 ${size}）移入系统回收站，同时清理缩略图缓存与扫描记录。\n每组『保留』的那张不会被删除；回收站里的文件可再恢复。`;
  }
  return `将把已勾选的 ${n} 张照片（共 ${size}）从照片库中移除，磁盘文件保留不动。\n移除后可在「最近删除」恢复；每组『保留』的那张不会被处理。`;
});

function askDelete(mode: DedupeDeleteMode) {
  if (!store.canDelete) return;
  pendingMode.value = mode;
}

async function confirmDelete() {
  const mode = pendingMode.value;
  pendingMode.value = null;
  if (!mode) return;
  const outcome = await store.deleteSelected(mode);
  if (!outcome) {
    if (store.error) notify.error("删除失败", store.error);
    return;
  }
  if (outcome.failed > 0) {
    notify.warning(
      "部分未删除",
      `${outcome.failed} 张处理失败（可能文件已被移动或占用），已保留在列表里可重试`,
    );
  } else {
    notify.success(
      mode === "trash" ? "已移入回收站" : "已删除记录",
      `共处理 ${outcome.deleted} 张`,
    );
  }
  thumbs.value = {};
  void loadThumbs();
}

function goScan() {
  router.push("/scan");
}
</script>

<template>
  <section class="dp">
    <!-- ① 判据与阈值 -->
    <div class="dp-toolbar">
      <div class="dp-criteria">
        <p class="dp-desc">
          全库范围内查找重复：<b>内容哈希一致</b>（字节完全相同）与 <b>画面相似</b>（图像向量余弦 ≥
          阈值）。每组默认保留分辨率最高的一张，其余可逐张勾选清理。
        </p>
        <div class="dp-switches">
          <label class="dp-switch">
            <input
              type="checkbox"
              :checked="store.includeExact"
              :disabled="store.running"
              @change="store.setIncludeExact(($event.target as HTMLInputElement).checked)"
            />
            完全重复（内容哈希）
          </label>
          <label class="dp-switch">
            <input
              type="checkbox"
              :checked="store.includeSimilar"
              :disabled="store.running"
              @change="store.setIncludeSimilar(($event.target as HTMLInputElement).checked)"
            />
            画面相似（向量阈值）
          </label>
        </div>
      </div>

      <div class="dp-threshold">
        <div class="dp-thr-head">
          <span class="dp-thr-label">相似度阈值</span>
          <span class="dp-thr-value">{{ store.threshold.toFixed(2) }}</span>
        </div>
        <input
          class="dp-range"
          type="range"
          min="0.8"
          max="1"
          step="0.01"
          :value="store.threshold"
          :disabled="store.running || !store.includeSimilar"
          aria-label="相似度阈值"
          @input="store.setThreshold(Number(($event.target as HTMLInputElement).value))"
        />
        <p class="dp-thr-hint">{{ hint }}</p>
      </div>

      <div class="dp-run">
        <button v-if="!store.running" class="btn btn-primary" :disabled="!canScan" @click="start">
          ▶ 开始扫描全库
        </button>
        <button v-else class="btn" :disabled="store.stopping" @click="store.stop()">
          {{ store.stopping ? "正在停止…" : "■ 停止" }}
        </button>
        <button v-if="hasResult && !store.running" class="btn" @click="store.reset()">清空结果</button>
      </div>
    </div>

    <!-- ② 进度 -->
    <div v-if="store.running" class="dp-progress">
      <div class="dp-progress-head">
        <span class="dp-stage">{{ stageLabel }}</span>
        <span class="dp-progress-text">{{ store.progressText }}</span>
        <span v-if="store.progressTotal" class="dp-progress-num">
          {{ store.progressDone }} / {{ store.progressTotal }}
        </span>
      </div>
      <div class="dp-bar"><i :style="{ width: `${progressPercent}%` }" /></div>
    </div>

    <!-- 错误态 -->
    <div v-if="store.error" class="dp-error">
      <span>{{ store.error }}</span>
      <button class="dp-mini" @click="store.error = ''">知道了</button>
    </div>

    <!-- HEIC 能力提示（缺 HEIF 扩展时，HEIC 照片解不出缩略图/向量） -->
    <div v-if="store.heic && !store.heic.available" class="dp-warn">
      HEIC（iPhone 照片）解码不可用：{{ store.heic.reason }}
    </div>

    <!-- 无向量提示：相似检测被跳过时的唯一解释来源 -->
    <div v-if="needVectorHint" class="dp-warn dp-warn-soft">
      <div>
        <template v-if="report?.similar_skipped_reason">{{ report.similar_skipped_reason }}</template>
        <template v-else>
          有 {{ report?.skipped_unencoded }} 张照片没有图像向量，已跳过相似检测（不影响完全重复判定）。
        </template>
      </div>
      <button class="dp-mini" @click="goScan">去扫描入库</button>
    </div>

    <!-- ③ 汇总带 -->
    <div v-if="hasResult && !store.running" class="dp-sum">
      <div class="dp-sum-item">
        <em>{{ report?.total_photos }}</em>
        <span>全库照片</span>
      </div>
      <div class="dp-sum-item">
        <em>{{ groups.length }}</em>
        <span>重复组</span>
      </div>
      <div class="dp-sum-item">
        <em>{{ report?.exact_groups }}</em>
        <span>完全重复</span>
      </div>
      <div class="dp-sum-item">
        <em>{{ report?.similar_groups }}</em>
        <span>画面相似</span>
      </div>
      <div class="dp-sum-item dp-sum-gain">
        <em>{{ formatSize(store.reclaimableBytes) }}</em>
        <span>可释放</span>
      </div>
      <div class="dp-sum-note">
        阈值 {{ report?.threshold.toFixed(2) }} · 耗时 {{ ((report?.elapsed_ms ?? 0) / 1000).toFixed(1) }}s
        <template v-if="report?.embedding_model"> · 向量 {{ report.embedding_model }}</template>
      </div>
    </div>

    <!-- ④ 分组列表 -->
    <div v-if="groups.length" class="dp-groups">
      <DupeGroupCard
        v-for="g in store.pageGroups"
        :key="g.key"
        :group="g"
        :keep-path="store.keepOf(g)"
        :thumbs="thumbs"
        :selected="store.selected"
        :busy="store.deleting"
        @toggle="store.toggle"
        @set-keep="(p: string) => store.setKeep(g, p)"
        @select-group="(on: boolean) => store.selectGroup(g, on)"
      />

      <div v-if="store.pageCount > 1" class="dp-pager">
        <button class="dp-mini" :disabled="store.page <= 0" @click="store.setPage(store.page - 1)">
          上一页
        </button>
        <span>{{ store.page + 1 }} / {{ store.pageCount }}</span>
        <button
          class="dp-mini"
          :disabled="store.page >= store.pageCount - 1"
          @click="store.setPage(store.page + 1)"
        >
          下一页
        </button>
      </div>
    </div>

    <!-- 空态 -->
    <EmptyState
      v-else-if="hasResult && !store.running"
      icon="🧹"
      title="没有发现重复照片"
      :description="
        store.includeSimilar
          ? '可以调低相似度阈值再扫一次；若刚导入大量照片，建议先做一次扫描入库（相似判定依赖图像向量）。'
          : '当前只比较文件内容是否完全相同。勾选『画面相似』可以再找同一张图的不同尺寸/格式副本。'
      "
      action-label="调低阈值？点这里重扫"
      @action="start"
    />

    <!-- 未扫描的空态 -->
    <EmptyState
      v-else-if="!store.running"
      icon="🔍"
      title="全库重复照片检测"
      description="点『开始扫描全库』：先按文件内容找出完全一样的副本，再按图像向量找出画面相同但尺寸/格式不同的照片。"
    />

    <!-- ⑤ 底部常驻操作条 -->
    <div v-if="groups.length && !store.running" class="dp-actions">
      <div class="dp-act-sum">
        已勾选 <b>{{ store.selectedCount }}</b> 张 · 可释放
        <b>{{ formatSize(store.selectedBytes) }}</b>
      </div>
      <div class="dp-act-btns">
        <button class="dp-mini" :disabled="store.deleting" @click="store.selectAll()">全选可删</button>
        <button class="dp-mini" :disabled="store.deleting" @click="store.clearSelection()">全不选</button>
        <button class="btn" :disabled="!store.canDelete" @click="askDelete('records')">
          仅删除记录
        </button>
        <button class="btn dp-danger" :disabled="!store.canDelete" @click="askDelete('trash')">
          移入回收站
        </button>
      </div>
    </div>

    <!-- 删除确认（两档各自一个文案） -->
    <ConfirmDialog
      :visible="pendingMode !== null"
      :title="dialogTitle"
      :message="dialogMessage"
      :confirm-text="pendingMode === 'trash' ? '移入回收站' : '仅删除记录'"
      cancel-text="取消"
      :danger="true"
      @confirm="confirmDelete"
      @cancel="pendingMode = null"
    />
  </section>
</template>

<style scoped>
.dp {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding-bottom: 12px;
}

/* ---- ① 工具栏 ---- */
.dp-toolbar {
  display: flex;
  gap: 18px;
  align-items: flex-start;
  flex-wrap: wrap;
  border: 1px solid var(--panel-glass-edge);
  border-radius: var(--panel-glass-radius);
  background: var(--panel-glass-fill);
  padding: 14px 16px;
}

.dp-criteria {
  flex: 1 1 380px;
  min-width: 260px;
}
.dp-desc {
  margin: 0 0 10px;
  font-size: 12.5px;
  line-height: 1.65;
  color: var(--c-fg-2);
  max-width: 720px;
}
.dp-desc b {
  color: var(--c-fg);
}

.dp-switches {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
}
.dp-switch {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  color: var(--c-fg);
  cursor: pointer;
  min-height: 24px;
}
.dp-switch input {
  width: 15px;
  height: 15px;
  accent-color: var(--color-primary);
  cursor: pointer;
}

.dp-threshold {
  flex: 0 1 250px;
  min-width: 220px;
}
.dp-thr-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}
.dp-thr-label {
  font-size: 12.5px;
  color: var(--c-fg-2);
}
.dp-thr-value {
  font-size: 16px;
  font-weight: var(--fw-em);
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
}
.dp-range {
  width: 100%;
  margin: 6px 0 2px;
  accent-color: var(--color-primary);
  cursor: pointer;
}
.dp-range:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.dp-thr-hint {
  margin: 0;
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--c-fg-2);
}

.dp-run {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}

/* ---- ② 进度 ---- */
.dp-progress {
  border: 1px solid var(--c-rule);
  border-radius: 12px;
  padding: 10px 14px;
  background: color-mix(in srgb, var(--c-fg) 3%, transparent);
}
.dp-progress-head {
  display: flex;
  gap: 10px;
  align-items: baseline;
  font-size: 12.5px;
  color: var(--c-fg-2);
  margin-bottom: 8px;
}
.dp-stage {
  color: var(--c-fg);
  font-weight: var(--fw-em);
}
.dp-progress-text {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.dp-progress-num {
  font-variant-numeric: tabular-nums;
}
.dp-bar {
  height: 6px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--c-fg) 12%, transparent);
  overflow: hidden;
}
.dp-bar i {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--color-primary);
  transition: width var(--dur-2);
}

/* ---- 提示与错误 ---- */
.dp-error {
  display: flex;
  align-items: center;
  gap: 12px;
  border: 1px solid color-mix(in srgb, var(--color-danger) 45%, transparent);
  background: color-mix(in srgb, var(--color-danger) 10%, transparent);
  color: var(--color-danger-text, var(--c-fg));
  border-radius: 10px;
  padding: 9px 12px;
  font-size: 12.5px;
}
.dp-warn {
  display: flex;
  align-items: center;
  gap: 12px;
  border: 1px solid color-mix(in srgb, var(--color-warning, #c07a00) 40%, transparent);
  background: color-mix(in srgb, var(--color-warning, #c07a00) 10%, transparent);
  border-radius: 10px;
  padding: 9px 12px;
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--c-fg);
}
.dp-warn > div {
  flex: 1;
}
.dp-warn-soft {
  border-style: dashed;
  background: color-mix(in srgb, var(--c-fg) 4%, transparent);
  border-color: var(--c-rule);
  color: var(--c-fg-2);
}

/* ---- ③ 汇总带 ---- */
.dp-sum {
  display: flex;
  align-items: baseline;
  gap: 24px;
  flex-wrap: wrap;
  padding: 10px 4px;
  border-bottom: 1px solid var(--c-rule);
}
.dp-sum-item {
  display: flex;
  align-items: baseline;
  gap: 6px;
}
.dp-sum-item em {
  font-style: normal;
  font-size: 18px;
  font-weight: var(--fw-em);
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
}
.dp-sum-item span {
  font-size: 12px;
  color: var(--c-fg-2);
}
.dp-sum-gain em {
  color: var(--color-primary);
}
.dp-sum-note {
  margin-left: auto;
  font-size: 11.5px;
  color: var(--c-fg-2);
}

/* ---- ④ 分组 ---- */
.dp-groups {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.dp-pager {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  font-size: 12.5px;
  color: var(--c-fg-2);
  padding: 4px 0 2px;
}

.dp-mini {
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 7px;
  padding: 4px 10px;
  font-size: 12px;
  cursor: pointer;
  min-height: 28px;
}
.dp-mini:hover:not(:disabled) {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}
.dp-mini:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

/* ---- ⑤ 底部操作条 ---- */
.dp-actions {
  position: sticky;
  bottom: 8px;
  z-index: 5;
  display: flex;
  align-items: center;
  gap: 16px;
  flex-wrap: wrap;
  margin-top: 4px;
  padding: 10px 14px;
  border: 1px solid var(--panel-glass-edge);
  border-radius: var(--panel-glass-radius);
  background: var(--panel-glass-fill);
  -webkit-backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  backdrop-filter: blur(var(--panel-glass-blur)) saturate(var(--panel-glass-saturate));
  box-shadow: var(--panel-glass-shadow);
}
.dp-act-sum {
  font-size: 12.5px;
  color: var(--c-fg-2);
}
.dp-act-sum b {
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
}
.dp-act-btns {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
  margin-left: auto;
}
.dp-danger {
  border-color: color-mix(in srgb, var(--color-danger) 55%, transparent);
  color: var(--color-danger-text, var(--color-danger));
}
.dp-danger:hover:not(:disabled) {
  background: color-mix(in srgb, var(--color-danger) 12%, transparent);
  color: var(--color-danger-text, var(--color-danger));
}

@media (max-width: 900px) {
  .dp-toolbar {
    padding: 12px;
  }
  .dp-act-btns {
    margin-left: 0;
    width: 100%;
  }
}
</style>
