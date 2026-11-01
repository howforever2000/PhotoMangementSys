<script setup lang="ts">
/**
 * 入库体检（FEAT-152）—— 替换掉未开发的「按年·地点浏览」
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 为什么这块属于「扫描入库」，而不是智慧相册
 * ──────────────────────────────────────────────────────────────────────────
 * 扫描入库往库里写三类东西：扫描记录、缩略图缓存、图像向量。写完就完了吗？
 * 没有 —— 相册目录会在系统里被改名 / 搬走 / 删掉，缩略图要首次访问才生成，
 * 向量只在勾选语义时才写。于是"库里的数据"与"盘上的照片"必然出现偏差。
 *
 * 「按年浏览」「按地点浏览」已经分别由时间线（/timeline）与智慧相册的地点分区
 * 承担，在扫描模块里再做一遍是第三个重复入口；而这套偏差检查在别处没有任何人做。
 *
 * ══════════════════════════════════════════════════════════════════════════
 * 本版边界（用户确认）：**只读 + 跳转**
 * ──────────────────────────────────────────────────────────────────────────
 * 不在体检页里做删除 / 重扫 / 重建这类写操作 —— 诊断与治疗分开，避免"点一下就
 * 动了用户的照片库"。每条问题都给出**去哪修**的入口（去扫描 / 打开相册）。
 *
 * 判定口径在 `utils/albumHealth.ts`（纯函数 + 单测），本组件只管取数、分组、呈现。
 */
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import type { AlbumHealthRow } from "../types/health";
import {
  albumCoverage,
  albumIssues,
  albumLevel,
  healthSummary,
  sortBySeverity,
  type HealthLevel,
} from "../utils/albumHealth";
import { formatSize } from "../types/album";
import { useAlbumHealthStore } from "../stores/albumHealth";
import { useNotify } from "../composables/useNotify";
import EmptyState from "./EmptyState.vue";
import HealthAlbumCard from "./health/HealthAlbumCard.vue";

const router = useRouter();
const notify = useNotify();
const store = useAlbumHealthStore();

/** 体检报告（存 store：切分区回来不重跑；要新数据点「重新体检」） */
const report = computed(() => store.report);
const loading = computed(() => store.loading);
const error = computed(() => store.error);
/** 只看有问题的（默认开：体检页的价值就是先看红的地方） */
const onlyIssues = ref(true);
/** 严重度筛选：null = 全部 */
const levelFilter = ref<HealthLevel | null>(null);
/** 列表一次渲染多少册（相册可能上百，先给一屏，其余点开） */
const PAGE = 40;
const limit = ref(PAGE);

/** 重新体检（只读命令；失败也要如实报给用户） */
async function refresh() {
  await store.load(true);
  if (store.error) notify.error("体检失败", store.error);
}

onMounted(() => {
  void store.load();
  if (store.error) notify.error("体检失败", store.error);
});

/* ---------------- 汇总与分组 ---------------- */
const summary = computed(() => healthSummary(report.value?.albums ?? []));

/** 排序后的全量行（严重在前） */
const sortedRows = computed(() => sortBySeverity(report.value?.albums ?? []));

/** 叠加两层筛选（只看问题 / 只看某一档） */
const filteredRows = computed<AlbumHealthRow[]>(() =>
  sortedRows.value.filter((row) => {
    if (levelFilter.value && albumLevel(row) !== levelFilter.value) return false;
    if (onlyIssues.value && albumLevel(row) === "ok") return false;
    return true;
  }),
);

const visibleRows = computed(() => filteredRows.value.slice(0, limit.value));
const hiddenCount = computed(() => Math.max(0, filteredRows.value.length - visibleRows.value.length));

/** 三档筛选按钮（数字就是数量，点它就是筛选） */
const levelPills = computed(() => [
  { key: "bad" as HealthLevel, label: "需处理", n: summary.value.bad },
  { key: "warn" as HealthLevel, label: "有缺口", n: summary.value.warn },
  { key: "ok" as HealthLevel, label: "正常", n: summary.value.ok },
]);

function pickLevel(v: HealthLevel) {
  levelFilter.value = levelFilter.value === v ? null : v;
  limit.value = PAGE;
}

/** 覆盖率读数（空库时显示 — 而不是 0%，0% 会被读成"全错"） */
const coverageText = computed(() => {
  if (!summary.value.totalPhotos) return "—";
  return `${Math.round(summary.value.scanCoverage * 100)}%`;
});

/** 体检口径的一句话说明 */
const scopeNote = computed(() => {
  const s = report.value;
  if (!s) return "";
  return `磁盘读数取自相册统计缓存（可能滞后于刚拖入的照片）· 每册抽样上限 ${s.sampled_per_album} 条验证文件是否存在 · 耗时 ${(s.elapsed_ms / 1000).toFixed(1)}s`;
});

const ranAtText = computed(() => {
  const ts = store.ranAt;
  if (!ts) return "";
  const d = new Date(ts);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${hh}:${mm} 体检`;
});

function openAlbum(row: AlbumHealthRow) {
  router.push(`/album/${row.album_id}`);
}

function goScan() {
  router.push("/scan");
}

function toggleOnlyIssues() {
  onlyIssues.value = !onlyIssues.value;
  limit.value = PAGE;
}

function showAll() {
  onlyIssues.value = false;
  levelFilter.value = null;
  limit.value = PAGE;
}
</script>

<template>
  <section class="ah">
    <!-- ① 口径与动作：先讲清"这份结论是什么口径"，再给刷新 -->
    <div class="ah-toolbar">
      <p class="ah-desc">
        体检「扫描入库」写下的三类数据是否还和磁盘对得上：
        <b>入库覆盖</b>（磁盘有、库里没有的漏扫）、<b>缩略图与向量覆盖</b>、
        <b>记录是否还指向存在的文件</b>，以及缩略图缓存占了多大。
        <span class="ah-readonly">全程只读</span>，不会改写你的照片库。
      </p>
      <div class="ah-actions">
        <span v-if="ranAtText" class="ah-stamp">{{ ranAtText }}</span>
        <button class="btn btn-primary" :disabled="loading" @click="refresh">
          {{ loading ? "体检中…" : "重新体检" }}
        </button>
      </div>
    </div>

    <!-- ② 加载态：骨架条，别用空屏（相册多时这一步要几百毫秒） -->
    <div v-if="loading" class="ah-skeleton" aria-hidden="true">
      <div v-for="i in 4" :key="i" class="sk-row" />
      <p class="sk-tip">正在统计入库 / 缩略图 / 向量覆盖，并抽样核对文件是否还在…</p>
    </div>

    <!-- ③ 错误态 -->
    <EmptyState
      v-else-if="error"
      tone="error"
      icon="🩺"
      title="体检没跑起来"
      :description="error"
      action-label="重试"
      @action="refresh"
    />

    <template v-else-if="report">
      <!-- ④ 结论带：三档计数（可点筛选）+ 总量读数 -->
      <div class="ah-sum">
        <div class="ah-pills">
          <button
            v-for="p in levelPills"
            :key="p.key"
            type="button"
            class="ah-pill"
            :class="[`pill-${p.key}`, { on: levelFilter === p.key }]"
            :aria-pressed="levelFilter === p.key"
            @click="pickLevel(p.key)"
          >
            <b>{{ p.n }}</b>
            <span>{{ p.label }}</span>
          </button>
        </div>
        <div class="ah-figures">
          <span>磁盘 <b>{{ summary.totalPhotos }}</b> 张</span>
          <span>已入库 <b>{{ summary.totalScanned }}</b> 张</span>
          <span>入库覆盖 <b>{{ coverageText }}</b></span>
          <span>缓存 <b>{{ formatSize(report.thumb_bytes) }}</b></span>
        </div>
        <p class="ah-scope">{{ scopeNote }}</p>
      </div>

      <!-- ⑤ 列表工具条 -->
      <div class="ah-listbar">
        <label class="ah-check">
          <input type="checkbox" :checked="onlyIssues" @change="toggleOnlyIssues" />
          只看有问题的
        </label>
        <span class="ah-count">
          共 {{ filteredRows.length }} 册{{ levelFilter ? `（已按「${levelPills.find((p) => p.key === levelFilter)?.label}」筛选）` : "" }}
        </span>
        <button v-if="onlyIssues || levelFilter" class="ah-clear" type="button" @click="showAll">
          显示全部
        </button>
      </div>

      <!-- ⑥ 逐册卡片 -->
      <div v-if="visibleRows.length" class="ah-list">
        <HealthAlbumCard
          v-for="row in visibleRows"
          :key="row.album_id"
          :row="row"
          :level="albumLevel(row)"
          :issues="albumIssues(row)"
          :coverage="albumCoverage(row)"
          @open="openAlbum(row)"
          @scan="goScan"
        />
        <button v-if="hiddenCount" class="ah-more" type="button" @click="limit += PAGE">
          还有 {{ hiddenCount }} 册，继续显示
        </button>
      </div>

      <!-- ⑦ 空态：分两种——"有相册但都健康" / "确实没问题可看" -->
      <EmptyState
        v-else-if="report.album_count === 0"
        icon="🩺"
        title="还没有可体检的相册"
        description="导入相册并做一次扫描入库后，这里会显示每个相册的入库覆盖、缩略图与向量缺口、以及记录是否还指向存在的文件。"
        action-label="去扫描入库"
        @action="goScan"
      />
      <EmptyState
        v-else
        icon="✅"
        title="这一屏没有需要处理的相册"
        :description="
          onlyIssues || levelFilter
            ? '当前筛选下没有命中：可以点「显示全部」看完整清单。'
            : '所有相册的入库、缩略图、向量都对得上，且记录指向的文件都在。'
        "
        action-label="显示全部"
        @action="showAll"
      />
    </template>

    <!-- ⑧ 初次进入（理论上有 onMounted，这里兜住极短的空白帧） -->
    <EmptyState
      v-else
      icon="🩺"
      title="入库体检"
      description="检查「扫描入库」写下的数据是否还和磁盘对得上：哪些相册没入库、哪些照片缺缩略图 / 缺向量、哪些记录已指向不存在的文件。"
      action-label="开始体检"
      @action="refresh"
    />
  </section>
</template>

<style scoped>
.ah {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding-bottom: 12px;
}

/* ---- ① 工具栏 ---- */
.ah-toolbar {
  display: flex;
  gap: 18px;
  align-items: flex-start;
  flex-wrap: wrap;
  border: 1px solid var(--panel-glass-edge);
  border-radius: var(--panel-glass-radius);
  background: var(--panel-glass-fill);
  padding: 14px 16px;
}
.ah-desc {
  flex: 1 1 420px;
  margin: 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--c-fg-2);
  max-width: 760px;
}
.ah-desc b {
  color: var(--c-fg);
}
.ah-readonly {
  margin-left: 6px;
  padding: 1px 6px;
  border-radius: 5px;
  border: 1px solid var(--c-rule);
  color: var(--c-fg-2);
  font-size: 11px;
  white-space: nowrap;
}
.ah-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}
.ah-stamp {
  font-size: 11.5px;
  color: var(--c-fg-2);
  font-variant-numeric: tabular-nums;
}

/* ---- ② 骨架 ---- */
.ah-skeleton {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.sk-row {
  height: 74px;
  border-radius: 12px;
  border: 1px solid var(--c-rule);
  background: linear-gradient(
    90deg,
    color-mix(in srgb, var(--c-fg) 4%, transparent),
    color-mix(in srgb, var(--c-fg) 8%, transparent),
    color-mix(in srgb, var(--c-fg) 4%, transparent)
  );
  background-size: 200% 100%;
  animation: sk 1.4s ease-in-out infinite;
}
.sk-tip {
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--c-fg-2);
}
@keyframes sk {
  0% {
    background-position: 100% 0;
  }
  100% {
    background-position: -100% 0;
  }
}
@media (prefers-reduced-motion: reduce) {
  .sk-row {
    animation: none;
  }
}

/* ---- ④ 结论带 ---- */
.ah-sum {
  display: flex;
  align-items: center;
  gap: 22px;
  flex-wrap: wrap;
  padding: 10px 2px 12px;
  border-bottom: 1px solid var(--c-rule);
}
.ah-pills {
  display: flex;
  gap: 8px;
}
.ah-pill {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  padding: 5px 11px;
  border-radius: 9px;
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  font-size: 12px;
  cursor: pointer;
  min-height: 30px;
}
.ah-pill b {
  font-size: 16px;
  font-variant-numeric: tabular-nums;
  color: var(--c-fg);
}
.ah-pill:hover {
  background: color-mix(in srgb, var(--c-fg) 7%, transparent);
}
.pill-bad.on {
  border-color: color-mix(in srgb, var(--color-danger) 60%, transparent);
  background: color-mix(in srgb, var(--color-danger) 12%, transparent);
}
.pill-warn.on {
  border-color: color-mix(in srgb, var(--color-warning, #c07a00) 60%, transparent);
  background: color-mix(in srgb, var(--color-warning, #c07a00) 12%, transparent);
}
.pill-ok.on {
  border-color: color-mix(in srgb, var(--c-fg-2) 45%, transparent);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}
.pill-bad.on b {
  color: var(--color-danger);
}
.pill-warn.on b {
  color: var(--color-warning-text, #c07a00);
}

.ah-figures {
  display: flex;
  gap: 18px;
  flex-wrap: wrap;
  font-size: 12px;
  color: var(--c-fg-2);
}
.ah-figures b {
  color: var(--c-fg);
  font-variant-numeric: tabular-nums;
  font-weight: var(--fw-em);
}
.ah-scope {
  margin: 0;
  margin-left: auto;
  font-size: 11.5px;
  color: var(--c-fg-2);
  max-width: 100%;
}

/* ---- ⑤ 列表工具条 ---- */
.ah-listbar {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  font-size: 12.5px;
  color: var(--c-fg-2);
}
.ah-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  min-height: 26px;
  color: var(--c-fg);
}
.ah-check input {
  width: 14px;
  height: 14px;
  accent-color: var(--color-primary);
  cursor: pointer;
}
.ah-clear {
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 7px;
  padding: 3px 10px;
  font-size: 11.5px;
  cursor: pointer;
  min-height: 26px;
}
.ah-clear:hover {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}

/* ---- ⑥ 列表 ---- */
.ah-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.ah-more {
  align-self: center;
  border: 1px solid var(--c-rule);
  background: transparent;
  color: var(--c-fg-2);
  border-radius: 8px;
  padding: 7px 16px;
  font-size: 12.5px;
  cursor: pointer;
  min-height: 32px;
}
.ah-more:hover {
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}

@media (max-width: 900px) {
  .ah-toolbar {
    padding: 12px;
  }
  .ah-scope {
    margin-left: 0;
  }
}
</style>
