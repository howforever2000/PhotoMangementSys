<script setup lang="ts">
import { ref } from "vue";
import { openPath } from "@tauri-apps/plugin-opener";
import { useContentStore } from "../stores/content";
import type { AlbumContentRow, ContentScanFilters, ContentSearchHit } from "../types/content";

const props = defineProps<{ albumId: number }>();
const contentStore = useContentStore();

const contentKeyword = ref("");
const contentHits = ref<ContentSearchHit[]>([]);
const contentSearching = ref(false);
let contentSearchTimer: ReturnType<typeof setTimeout> | null = null;

/** 防抖搜索 */
function onContentSearchInput() {
  if (contentSearchTimer) clearTimeout(contentSearchTimer);
  contentSearchTimer = setTimeout(async () => {
    const kw = contentKeyword.value.trim();
    if (!kw) {
      contentHits.value = [];
      return;
    }
    contentSearching.value = true;
    try {
      contentHits.value = await contentStore.searchPhotoContent(kw, props.albumId);
    } catch {
      contentHits.value = [];
    } finally {
      contentSearching.value = false;
    }
  }, 300);
}

function clearContentSearch() {
  contentKeyword.value = "";
  contentHits.value = [];
}

// ---- 过滤条件 ----
const contentFilter = ref<ContentScanFilters>({
  iso_min: null,
  iso_max: null,
  shutter_min: null,
  shutter_max: null,
  aperture_min: null,
  aperture_max: null,
  focal_min: null,
  focal_max: null,
  tone_type: null,
});
const contentFilterHits = ref<AlbumContentRow[]>([]);
const contentFilterSearching = ref(false);

async function searchPhotoContentWithFilters() {
  contentFilterSearching.value = true;
  try {
    contentFilterHits.value = await contentStore.searchPhotoContentWithFilters(
      contentKeyword.value,
      props.albumId,
      contentFilter.value,
    );
    contentHits.value = [];
  } catch (e) {
    alert(`过滤搜索失败：${e}`);
  } finally {
    contentFilterSearching.value = false;
  }
}

function clearContentFilters() {
  contentFilter.value = {
    iso_min: null,
    iso_max: null,
    shutter_min: null,
    shutter_max: null,
    aperture_min: null,
    aperture_max: null,
    focal_min: null,
    focal_max: null,
    tone_type: null,
  };
  contentFilterHits.value = [];
}

async function openContentHit(path: string) {
  try {
    await openPath(path);
  } catch (e) {
    alert(`无法打开图片：${path}\n\n${e}`);
  }
}

function contentHitName(hit: ContentSearchHit): string {
  if (hit.label) return hit.label;
  if (hit.category) return hit.category;
  const i = Math.max(hit.path.lastIndexOf("/"), hit.path.lastIndexOf("\\"));
  return i >= 0 ? hit.path.slice(i + 1) : hit.path;
}

/** 影调映射 */
const toneLabelMap: Record<string, string> = {
  "low-key": "低调",
  "mid-key": "中间调",
  "high-key": "高调",
  LowKey: "低调",
  MidKey: "中间调",
  HighKey: "高调",
};
</script>

<template>
  <div class="content-search-area">
    <div class="content-search-input-wrap">
      <input
        v-model="contentKeyword"
        class="content-search-input"
        placeholder="在本相册内按内容搜索照片，如：狗 / 人物 / 建筑 / P001…"
        @input="onContentSearchInput"
      />
      <button v-if="contentKeyword" class="search-clear" @click="clearContentSearch">×</button>
    </div>

    <!-- 过滤条 -->
    <div class="content-search-filters">
      <div class="filter-group">
        <label class="filter-label">ISO</label>
        <select v-model="contentFilter.iso_min" class="filter-select" title="ISO 下限">
          <option :value="null">不限</option>
          <option value="80">80+</option>
          <option value="200">200+</option>
          <option value="400">400+</option>
          <option value="800">800+</option>
          <option value="1600">1600+</option>
        </select>
        <select v-model="contentFilter.iso_max" class="filter-select" title="ISO 上限">
          <option :value="null">不限</option>
          <option value="200">≤200</option>
          <option value="400">≤400</option>
          <option value="800">≤800</option>
          <option value="1600">≤1600</option>
          <option value="3200">≤3200</option>
        </select>
      </div>
      <div class="filter-group">
        <label class="filter-label">快门</label>
        <select v-model="contentFilter.shutter_min" class="filter-select" title="快门下限">
          <option :value="null">不限</option>
          <option value="0.001">≥1/1000</option>
          <option value="0.005">≥1/200</option>
          <option value="0.01">≥1/100</option>
          <option value="0.02">≥1/50</option>
          <option value="0.1">≥1/10</option>
        </select>
        <select v-model="contentFilter.shutter_max" class="filter-select" title="快门上限">
          <option :value="null">不限</option>
          <option value="0.01">≤1/100</option>
          <option value="0.05">≤1/20</option>
          <option value="0.1">≤1/10</option>
          <option value="1">≤1s</option>
          <option value="10">≤10s</option>
        </select>
      </div>
      <div class="filter-group">
        <label class="filter-label">光圈</label>
        <select v-model="contentFilter.aperture_min" class="filter-select" title="光圈下限">
          <option :value="null">不限</option>
          <option value="1.4">≥f/1.4</option>
          <option value="2">≥f/2</option>
          <option value="2.8">≥f/2.8</option>
          <option value="4">≥f/4</option>
          <option value="5.6">≥f/5.6</option>
        </select>
        <select v-model="contentFilter.aperture_max" class="filter-select" title="光圈上限">
          <option :value="null">不限</option>
          <option value="2.8">≤f/2.8</option>
          <option value="4">≤f/4</option>
          <option value="5.6">≤f/5.6</option>
          <option value="8">≤f/8</option>
          <option value="16">≤f/16</option>
        </select>
      </div>
      <div class="filter-group">
        <label class="filter-label">焦段</label>
        <select v-model="contentFilter.focal_min" class="filter-select" title="焦段下限">
          <option :value="null">不限</option>
          <option value="24">≥24mm</option>
          <option value="35">≥35mm</option>
          <option value="50">≥50mm</option>
          <option value="85">≥85mm</option>
          <option value="135">≥135mm</option>
        </select>
        <select v-model="contentFilter.focal_max" class="filter-select" title="焦段上限">
          <option :value="null">不限</option>
          <option value="35">≤35mm</option>
          <option value="50">≤50mm</option>
          <option value="85">≤85mm</option>
          <option value="135">≤135mm</option>
          <option value="200">≤200mm</option>
        </select>
      </div>
      <div class="filter-group">
        <label class="filter-label">影调</label>
        <select v-model="contentFilter.tone_type" class="filter-select">
          <option :value="null">不限</option>
          <option value="low-key">低调</option>
          <option value="mid-key">中间调</option>
          <option value="high-key">高调</option>
        </select>
      </div>
      <div class="filter-actions">
        <button class="btn btn-primary btn-sm" @click="searchPhotoContentWithFilters">
          🔍 过滤搜索
        </button>
        <button class="btn btn-ghost btn-sm" @click="clearContentFilters">重置</button>
      </div>
    </div>

    <div v-if="contentKeyword.trim()" class="content-search-results">
      <div v-if="contentSearching" class="scan-empty">正在搜索照片内容…</div>
      <div v-else-if="contentHits.length === 0" class="scan-empty">
        未在本相册中找到匹配的照片（可先点击上方「内容识别」写入内容库）
      </div>
      <div v-else class="content-hit-list">
        <div
          v-for="hit in contentHits"
          :key="hit.id"
          class="content-hit-item"
          :title="hit.path"
          @click="openContentHit(hit.path)"
        >
          <span class="content-hit-name">{{ contentHitName(hit) }}</span>
          <span class="content-hit-tags">
            <span v-if="hit.label" class="top3-chip">{{ hit.label }}</span>
            <span v-for="pid in hit.person_ids" :key="pid" class="person-chip">{{ pid }}</span>
            <span v-if="hit.location" class="top3-chip">{{ hit.location }}</span>
            <span v-if="hit.shoot_time" class="top3-chip">{{ hit.shoot_time }}</span>
            <span v-if="hit.iso" class="top3-chip">ISO {{ hit.iso }}</span>
            <span v-if="hit.aperture" class="top3-chip">{{ hit.aperture }}</span>
          </span>
        </div>
      </div>
    </div>

    <!-- 过滤搜索结果 -->
    <div v-if="contentFilterHits.length > 0 || contentFilterSearching" class="content-search-results">
      <div v-if="contentFilterSearching" class="scan-empty">正在按条件过滤…</div>
      <div v-else-if="contentFilterHits.length === 0" class="scan-empty">
        未在当前相册中找到匹配过滤条件的照片
      </div>
      <div v-else class="content-hit-list">
        <div
          v-for="hit in contentFilterHits"
          :key="hit.id"
          class="content-hit-item"
          :title="hit.path"
          @click="openContentHit(hit.path)"
        >
          <span class="content-hit-name">{{ hit.path.split('/').pop() || hit.path.split('\\').pop() }}</span>
          <span class="content-hit-tags">
            <span v-if="hit.label" class="top3-chip">{{ hit.label }}</span>
            <span v-if="hit.category" class="top3-chip">{{ hit.category }}</span>
            <span v-if="hit.tone_type" class="tone-badge tone-" :class="hit.tone_type">{{ toneLabelMap[hit.tone_type] || hit.tone_type }}</span>
            <span v-if="hit.iso" class="top3-chip">ISO {{ hit.iso }}</span>
            <span v-if="hit.shoot_time" class="top3-chip">{{ hit.shoot_time }}</span>
          </span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.content-search-area {
  margin-top: 20px;
  border-top: 1px solid #e5e7eb;
  padding-top: 16px;
}

.content-search-input-wrap {
  display: flex;
  align-items: center;
  gap: 4px;
}

.content-search-input {
  flex: 1;
  padding: 8px 12px;
  border: 1px solid #d0d5dd;
  border-radius: 6px;
  font-size: 13px;
  outline: none;
  transition: border-color 0.15s;
}

.content-search-input:focus {
  border-color: #396cd8;
}

.search-clear {
  background: transparent;
  border: none;
  font-size: 18px;
  cursor: pointer;
  color: #667085;
  padding: 4px 8px;
}

.search-clear:hover {
  color: #e5484d;
}

.content-search-results {
  margin-top: 8px;
}

.content-hit-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.content-hit-item {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px 10px;
  padding: 6px 10px;
  border-radius: 4px;
  cursor: pointer;
  transition: background 0.12s;
}

.content-hit-item:hover {
  background: #f0f4ff;
}

.content-hit-name {
  font-weight: 500;
  font-size: 13px;
  min-width: 80px;
}

.content-hit-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

/* 过滤条 */
.content-search-filters {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  padding: 10px 12px;
  background: #f8f9fa;
  border-radius: 6px;
  margin-bottom: 8px;
  align-items: flex-end;
  margin-top: 8px;
}

.filter-group {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.filter-label {
  font-size: 10px;
  font-weight: 600;
  color: #667085;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.filter-select {
  font-size: 12px;
  padding: 2px 4px;
  border: 1px solid #d0d5dd;
  border-radius: 4px;
  background: #fff;
  min-width: 68px;
}

.filter-actions {
  display: flex;
  gap: 4px;
  padding-left: 8px;
  margin-left: auto;
}

/* 复用标签 */
.scan-empty {
  padding: 12px;
  color: #667085;
  font-size: 13px;
}

.top3-chip {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  background: #eef3fb;
  color: #396cd8;
  font-size: 11px;
}

.person-chip {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  background: #fef3c7;
  color: #9a6b00;
  font-size: 11px;
}

.tone-badge {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: 11px;
}

.tone-badge.tone-low-key,
.tone-badge.tone-low_key {
  background: #2a2a2a;
  color: #fff;
}

.tone-badge.tone-mid-key,
.tone-badge.tone-mid_key {
  background: #667085;
  color: #fff;
}

.tone-badge.tone-high-key,
.tone-badge.tone-high_key {
  background: #e5e7eb;
  color: #1f2328;
}
</style>