<script setup lang="ts">
/**
 * 内容分类画廊（FEAT-048）—— 智慧相册「内容分类」tab
 *
 * 两级视图（页内切换，不弹窗，层级清晰）：
 *  1. 大类卡片网格：中文类名（categoryLabel 映射）+ 张数徽标 + 细类数提示，
 *     封面取该类置信度最高的照片（get_photo_thumbs 缓存管线，FEAT-044 命中 0 IO）
 *  2. 点击大类 → 照片网格 + 细类 chips 即时过滤（含计数，中文映射 subCategoryLabel）
 *
 * 复用底层：list_content_categories / list_photos_by_category 命令、
 * PhotoLightbox 大图（左右切换）、theme 暗色适配、骨架/空/错误三态。
 */
import { computed, onMounted, ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "./PhotoLightbox.vue";
import type { CategoryGroupRow, ContentSearchHit } from "../types/content";
import { categoryLabel, subCategoryLabel, categoryTone } from "../utils/categoryLabel";

const theme = useThemeStore();
const notify = useNotify();

/* -------------------- 数据 -------------------- */
const loading = ref(true);
const error = ref("");
/** (category, sub_category) 分组行（后端一次聚合返回） */
const groups = ref<CategoryGroupRow[]>([]);
/** path → 缩略图缓存路径（卡片封面与照片网格共用） */
const thumbMap = ref<Record<string, string>>({});

/* -------------------- 视图 1：大类卡片 -------------------- */
interface TopCategory {
  category: string;
  label: string;
  count: number;
  cover: string | null;
  coverAlbumId: number | null;
  subCount: number;
}
const topCategories = computed<TopCategory[]>(() => {
  const map = new Map<string, TopCategory>();
  for (const g of groups.value) {
    let t = map.get(g.category);
    if (!t) {
      t = {
        category: g.category,
        label: categoryLabel(g.category),
        count: 0,
        cover: g.cover_path,
        coverAlbumId: g.cover_album_id,
        subCount: 0,
      };
      map.set(g.category, t);
    }
    t.count += g.count;
    if (g.sub_category && g.sub_category !== g.category) t.subCount += 1;
  }
  return [...map.values()].sort((a, b) => b.count - a.count);
});

/** 卡片无封面时的类目色底（复用 categoryTone 色卡） */
function toneBg(category: string): string {
  return categoryTone(category).bg;
}

async function load() {
  loading.value = true;
  error.value = "";
  try {
    groups.value = await invoke<CategoryGroupRow[]>("list_content_categories");
    await loadCoverThumbs();
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 封面缩略图批量懒加载（按相册分组，复用真实相册缓存命名） */
async function loadCoverThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const t of topCategories.value) {
    if (!t.cover || thumbMap.value[t.cover]) continue;
    const aid = t.coverAlbumId ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(t.cover);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 封面缺图不阻塞：回退类目色底 */
      }
    }),
  );
}

/* -------------------- 视图 2：大类照片网格 + 细类 chips -------------------- */
const activeCategory = ref<TopCategory | null>(null);
const activeSub = ref<string | null>(null); // null / "" = 全部
const photos = ref<ContentSearchHit[]>([]);
const photosLoading = ref(false);

/** 细类 chips（含「全部」）；该大类无细类时为空 → 不渲染过滤条 */
const subChips = computed(() => {
  if (!activeCategory.value) return [];
  const rows = groups.value.filter((g) => g.category === activeCategory.value!.category);
  const chips = rows
    .filter((r) => r.sub_category && r.sub_category !== r.category)
    .map((r) => ({ key: r.sub_category as string, label: subCategoryLabel(r.sub_category), count: r.count }));
  if (!chips.length) return [];
  chips.sort((a, b) => b.count - a.count);
  const total = rows.reduce((n, r) => n + r.count, 0);
  return [{ key: "", label: "全部", count: total }, ...chips];
});

/** chips 过滤纯前端内存过滤：点击即时切换，无请求等待 */
const filteredPhotos = computed(() => {
  if (!activeSub.value) return photos.value;
  return photos.value.filter((p) => p.sub_category === activeSub.value);
});

async function openCategory(t: TopCategory) {
  activeCategory.value = t;
  activeSub.value = null;
  photosLoading.value = true;
  try {
    photos.value = await invoke<ContentSearchHit[]>("list_photos_by_category", {
      category: t.category,
    });
    await loadGridThumbs();
  } catch (e) {
    notify.error("加载分类照片失败", String(e));
    photos.value = [];
  } finally {
    photosLoading.value = false;
  }
}

async function loadGridThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const r of photos.value) {
    if (thumbMap.value[r.path]) continue;
    const aid = r.album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(r.path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 单相册失败不阻塞：缺图卡片回退占位 */
      }
    }),
  );
}

function backToCards() {
  activeCategory.value = null;
  activeSub.value = null;
  photos.value = [];
}

function selectSub(key: string) {
  activeSub.value = key || null;
}

/* -------------------- 大图看图器 -------------------- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
const lightboxPhotos = computed(() =>
  filteredPhotos.value.map((p) => ({ path: p.path, albumId: p.album_id })),
);
function openLightbox(p: ContentSearchHit) {
  const idx = filteredPhotos.value.findIndex((x) => x.path === p.path);
  if (idx < 0) return;
  lightboxIndex.value = idx;
  lightboxOpen.value = true;
}

function fileUrl(p: string): string {
  return p ? convertFileSrc(p) : "";
}

onMounted(load);
</script>

<template>
  <div class="cg-wrap" :style="{ color: theme.textColor }">
    <!-- ============ 视图 1：大类卡片 ============ -->
    <template v-if="!activeCategory">
      <!-- 加载骨架 -->
      <div v-if="loading" class="cg-state">
        <div class="sk-grid">
          <div v-for="i in 6" :key="i" class="sk-card"></div>
        </div>
      </div>

      <!-- 错误态 -->
      <div v-else-if="error" class="cg-state">
        <div class="cg-state-icon">⚠️</div>
        <p>加载失败：{{ error }}</p>
        <button class="btn" @click="load">重试</button>
      </div>

      <!-- 空态引导 -->
      <div v-else-if="!topCategories.length" class="cg-state">
        <div class="cg-state-icon">🏞️</div>
        <p class="cg-state-title">还没有内容分类数据</p>
        <p class="cg-state-text">请先在相册详情页执行「内容扫描 / 综合扫描」，AI 识别的分类会聚合展示在这里。</p>
      </div>

      <!-- 大类卡片网格 -->
      <div v-else class="cat-grid">
        <article
          v-for="t in topCategories"
          :key="t.category"
          class="cat-card"
          :style="theme.cardStyle"
          :title="`${t.label} · ${t.count} 张`"
          @click="openCategory(t)"
        >
          <div class="cat-cover" :style="{ background: toneBg(t.category) }">
            <img v-if="t.cover && thumbMap[t.cover]" :src="fileUrl(thumbMap[t.cover])" loading="lazy" alt="" />
            <span v-else class="cat-cover-ph">{{ t.label.slice(0, 1) }}</span>
            <span class="cat-count">{{ t.count }} 张</span>
          </div>
          <div class="cat-body">
            <h3 class="cat-name">{{ t.label }}</h3>
            <span v-if="t.subCount" class="cat-sub-hint">{{ t.subCount }} 个细类</span>
          </div>
        </article>
      </div>
    </template>

    <!-- ============ 视图 2：大类照片网格 ============ -->
    <template v-else>
      <div class="cg-detail-head">
        <button class="btn" @click="backToCards">← 返回分类</button>
        <h2 class="cg-detail-title">{{ activeCategory.label }}</h2>
        <span class="cg-detail-count">{{ activeCategory.count }} 张</span>
      </div>

      <!-- 细类 chips 即时过滤 -->
      <div v-if="subChips.length" class="chip-row">
        <button
          v-for="c in subChips"
          :key="c.key"
          class="chip"
          :class="{ on: (activeSub ?? '') === c.key }"
          @click="selectSub(c.key)"
        >
          {{ c.label }} <span class="chip-n">{{ c.count }}</span>
        </button>
      </div>

      <div v-if="photosLoading" class="cg-state">
        <div class="sk-grid">
          <div v-for="i in 8" :key="i" class="sk-card sk-photo"></div>
        </div>
      </div>
      <div v-else-if="!filteredPhotos.length" class="cg-state">
        <div class="cg-state-icon">🗂</div>
        <p>该细类下暂无照片</p>
      </div>
      <div v-else class="photo-grid">
        <figure
          v-for="p in filteredPhotos"
          :key="p.id"
          class="photo-cell"
          :title="[p.label, p.shoot_time].filter(Boolean).join(' · ')"
          @click="openLightbox(p)"
        >
          <img v-if="thumbMap[p.path]" :src="fileUrl(thumbMap[p.path])" loading="lazy" alt="" />
          <div v-else class="photo-ph">🖼</div>
          <figcaption v-if="p.label" class="photo-cap">{{ subCategoryLabel(p.label) }}</figcaption>
        </figure>
      </div>
    </template>

    <!-- 大图看图器（复用） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      @close="lightboxOpen = false"
    />
  </div>
</template>

<style scoped>
.cg-wrap {
  min-width: 0;
}

/* ---- 三态 ---- */
.cg-state {
  padding: 30px 10px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.cg-state-icon {
  font-size: 44px;
}
.cg-state-title {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
}
.cg-state-text {
  opacity: 0.7;
  font-size: 13px;
  max-width: 420px;
  line-height: 1.6;
  margin: 0;
}

/* 骨架 */
.sk-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 14px;
  width: 100%;
}
.sk-card {
  height: 190px;
  border-radius: 14px;
  background: rgba(127, 127, 127, 0.16);
  animation: cgpulse 1.2s infinite;
}
.sk-photo {
  height: 130px;
}
@keyframes cgpulse {
  50% { opacity: 0.4; }
}

/* ---- 大类卡片网格 ---- */
.cat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
}
.cat-card {
  border-radius: 14px;
  overflow: hidden;
  cursor: pointer;
  transition: transform 0.16s ease, box-shadow 0.16s ease;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.1);
}
.cat-card:hover {
  transform: translateY(-3px);
  box-shadow: 0 10px 24px rgba(0, 0, 0, 0.18);
}
.cat-cover {
  position: relative;
  aspect-ratio: 4 / 3;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
}
.cat-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.cat-cover-ph {
  font-size: 40px;
  font-weight: 700;
  opacity: 0.75;
}
.cat-count {
  position: absolute;
  right: 8px;
  top: 8px;
  padding: 2px 9px;
  font-size: 11.5px;
  color: #fff;
  background: rgba(0, 0, 0, 0.55);
  border-radius: 999px;
  backdrop-filter: blur(3px);
}
.cat-body {
  padding: 10px 12px 12px;
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}
.cat-name {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
}
.cat-sub-hint {
  font-size: 11.5px;
  opacity: 0.6;
  white-space: nowrap;
}

/* ---- 二级头部 + chips ---- */
.cg-detail-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
  flex-wrap: wrap;
}
.cg-detail-title {
  margin: 0;
  font-size: 19px;
  font-weight: 700;
}
.cg-detail-count {
  font-size: 12.5px;
  opacity: 0.65;
}
.chip-row {
  display: flex;
  gap: 8px;
  overflow-x: auto;
  padding: 2px 2px 10px;
}
.chip {
  flex: 0 0 auto;
  padding: 5px 12px;
  font-size: 12.5px;
  border-radius: 999px;
  border: 1px solid rgba(127, 127, 127, 0.35);
  background: transparent;
  color: inherit;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
}
.chip:hover {
  border-color: rgba(106, 141, 240, 0.7);
}
.chip.on {
  background: rgba(106, 141, 240, 0.16);
  border-color: rgba(106, 141, 240, 0.8);
  font-weight: 600;
}
.chip-n {
  opacity: 0.65;
  font-size: 11px;
  margin-left: 2px;
}

/* ---- 照片网格 ---- */
.photo-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 10px;
}
.photo-cell {
  margin: 0;
  position: relative;
  aspect-ratio: 1;
  border-radius: 10px;
  overflow: hidden;
  cursor: zoom-in;
  background: rgba(127, 127, 127, 0.12);
  transition: transform 0.12s ease;
}
.photo-cell:hover {
  transform: translateY(-2px);
}
.photo-cell img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.photo-ph {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  opacity: 0.6;
}
.photo-cap {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 14px 8px 6px;
  font-size: 11px;
  color: #fff;
  background: linear-gradient(transparent, rgba(0, 0, 0, 0.6));
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

@media (max-width: 640px) {
  .cat-grid { grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 10px; }
  .photo-grid { grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
}
</style>
