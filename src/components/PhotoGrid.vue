<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { invoke } from "@tauri-apps/api/core";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import type { AlbumContentRow } from "../types/content";
import type { PersonInfo } from "../types/photo";
import ConfirmDialog from "./ConfirmDialog.vue";
import PhotoLightbox from "./PhotoLightbox.vue";

/**
 * 照片网格浏览组件
 *
 * 数据源：`list_album_photos`（walkdir 列出相册内全部图片路径，无需先扫描）
 * 展示：256px JPEG 网格缩略图（`get_photo_thumbs` 指纹缓存，分批懒加载），
 *       避免网格直接加载原图造成的内存/IO 压力；
 *       大图查看器（PhotoLightbox）按需加载单张原图。
 * 元数据叠加：`read_album_content`（AI 分类/人脸/EXIF），扫描后支持按分类/人物筛选。
 */

const props = defineProps<{ albumId: number }>();
const emit = defineEmits<{
  (e: "count", n: number): void;
}>();

const albumStore = useAlbumStore();
const contentStore = useContentStore();

/** 跨挂载的缩略图路径缓存（模块级，组件重建不丢失）。
 *  后端本身已持久化在 app_data/thumbs/grid/，这里避免前端每次进入都回到占位条重新查询/加载。 */
const thumbCache = new Map<string, string>();

/** 网格照片项：路径 + 可选的扫描元数据 */
interface GridPhoto {
  path: string;
  meta?: AlbumContentRow;
}

const photos = ref<GridPhoto[]>([]);
const loading = ref(true);
const loadError = ref("");

/** path → 缩略图缓存路径（生成后由 get_photo_thumbs 填充） */
const thumbMap = ref<Record<string, string>>({});

/** 当前筛选：null=全部，或指定大类（人物筛选已移至主页「智慧相册」板块） */
const activeCategory = ref<string | null>(null);

/** 大图查看器状态 */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);

/* ---- 人物注册表：编号 → 自定义命名 + 头像（代表脸裁剪）---- */
const personsMap = ref<Record<string, string>>({});
/** pid → 头像缓存文件的 convertFileSrc URL；获取失败（服务未运行等）无键，回退首字占位 */
const avatarMap = ref<Record<string, string>>({});

async function loadPersons() {
  try {
    const list = await invoke<PersonInfo[]>("list_persons");
    const map: Record<string, string> = {};
    for (const p of list) map[p.id] = p.name || p.id;
    personsMap.value = map;
    // 并发拉取头像（后端有本地缓存，命中直接返回）
    for (const p of list) {
      if (avatarMap.value[p.id]) continue;
      try {
        const cachePath = await albumStore.getPersonAvatar(p.id);
        avatarMap.value = { ...avatarMap.value, [p.id]: fileUrl(cachePath) };
      } catch {
        /* 服务未运行/无登记脸：保持无键，渲染回退占位 */
      }
    }
  } catch {
    /* 人物服务不可用时网格仍可正常浏览 */
  }
}

function personName(pid: string): string {
  return personsMap.value[pid] || pid;
}

/** 缩略图角标最多展示的人物数，超出折叠为 +N */
const MAX_BADGE_PERSONS = 3;

/** 缩略图按需生成：逐格观察视口，进入视口（含提前量）即入队后台异步生成 */
const BATCH = 60;
const thumbsLoading = ref(false);
let cellObserver: IntersectionObserver | null = null;
const pendingThumbs = new Set<string>();
let flushing = false;

function fileUrl(p: string) {
  return p ? convertFileSrc(p) : "";
}

/** 过滤后用于网格展示的照片（仅按大类筛选） */
const visiblePhotos = computed<GridPhoto[]>(() => {
  if (!activeCategory.value) return photos.value;
  return photos.value.filter((ph) => ph.meta?.category === activeCategory.value);
});

/** 从已扫描元数据推导出的可选大类（用于筛选 chips） */
const categoryOptions = computed<string[]>(() => {
  const s = new Set<string>();
  for (const ph of photos.value) {
    if (ph.meta?.category) s.add(ph.meta.category);
  }
  return [...s].sort();
});

/** 是否已有扫描元数据（大类）可用于筛选 */
function hasAnyMeta() {
  return categoryOptions.value.length > 0;
}

/** 内部滚动容器（照片多时只在子框内滚动，整页不跟着滚） */
const scrollEl = ref<HTMLElement | null>(null);
const showTopBtn = ref(false);

async function load() {
  loading.value = true;
  loadError.value = "";
  const cached: Record<string, string> = {};
  try {
    // 1. 文件夹图片列表（始终可用，无需扫描）
    const paths = await albumStore.listAlbumPhotos(props.albumId);
    // 2. 已扫描元数据：一次性拉取全量（page_size 足够大）
    let rows: AlbumContentRow[] = [];
    try {
      const res = await contentStore.readAlbumContent(props.albumId, 1, 5000);
      rows = res.rows;
    } catch {
      rows = []; // 扫描数据不可用时仍可浏览照片，仅无标签
    }
    const metaMap = new Map<string, AlbumContentRow>();
    for (const r of rows) metaMap.set(r.path, r);
    photos.value = paths.map((p) => ({ path: p, meta: metaMap.get(p) }));
    emit("count", paths.length);
    // 命中模块级缓存的缩略图直接回填，重进页面不再闪占位符
    for (const p of paths) {
      const t = thumbCache.get(p);
      if (t) cached[p] = t;
    }
    thumbMap.value = cached;
    // 首屏缩略图由格子注册后的 IntersectionObserver 初始回调自动触发，无需额外预取
  } catch (e) {
    loadError.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 惰性创建格子观察器：以子框为滚动根，向上/向下都提前 600px 触发 */
function getCellObserver(): IntersectionObserver | null {
  if (cellObserver) return cellObserver;
  if (typeof IntersectionObserver === "undefined") return null;
  cellObserver = new IntersectionObserver(onCellsIntersect, {
    root: scrollEl.value,
    rootMargin: "600px 0px",
  });
  return cellObserver;
}

/** 模板函数 ref：注册每个照片格子，缺缩略图的才纳入观察 */
function registerCell(el: unknown, path: string) {
  const node = el as HTMLElement | null;
  if (!node) return; // 卸载
  node.dataset.path = path;
  if (!thumbMap.value[path] && !thumbCache.has(path)) {
    getCellObserver()?.observe(node);
  }
}

/** 格子进入视口：把缺失项加入待生成队列并启动后台冲刷 */
function onCellsIntersect(entries: IntersectionObserverEntry[]) {
  let added = false;
  for (const en of entries) {
    if (!en.isIntersecting) continue;
    const p = (en.target as HTMLElement).dataset.path;
    if (p && !thumbMap.value[p] && !pendingThumbs.has(p)) {
      pendingThumbs.add(p);
      added = true;
    }
  }
  if (added) void flushPending();
}

/** 后台队列冲刷：每批 BATCH 张依次请求，不阻塞滚动；进行中新增的入队下一轮继续 */
async function flushPending() {
  if (flushing) return;
  flushing = true;
  try {
    while (pendingThumbs.size > 0) {
      const batch = [...pendingThumbs].slice(0, BATCH);
      for (const p of batch) pendingThumbs.delete(p);
      thumbsLoading.value = true;
      try {
        const pairs = await albumStore.getPhotoThumbs(props.albumId, batch);
        for (const [path, thumb] of pairs) {
          thumbMap.value[path] = thumb;
          thumbCache.set(path, thumb);
        }
      } catch {
        // 单批失败不阻塞：滚回该区域时会因仍缺图而重新触发
      }
    }
  } finally {
    flushing = false;
    thumbsLoading.value = false;
  }
}

function onScroll() {
  showTopBtn.value = (scrollEl.value?.scrollTop ?? 0) > 300;
}

function backToTop() {
  scrollEl.value?.scrollTo({ top: 0, behavior: "smooth" });
}

function openLightbox(index: number) {
  // 多选模式下点击格子是选中/取消选中，不打开大图
  if (selectMode.value) {
    toggleSelect(visiblePhotos.value[index].path);
    return;
  }
  lightboxIndex.value = index;
  lightboxOpen.value = true;
}

function closeLightbox() {
  lightboxOpen.value = false;
}

/* ---- 多选删除：选择模式 + 二次确认（记录删除 / 文件删除两种）---- */
const selectMode = ref(false);
const selectedPaths = ref<Set<string>>(new Set());

/** 二次确认弹窗状态：null=关闭；否则记录待删模式与路径 */
const confirmState = ref<null | { mode: "record" | "file"; paths: string[] }>(null);
const deleting = ref(false);
const deleteMsg = ref("");

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  selectedPaths.value = new Set();
}

function exitSelectMode() {
  selectMode.value = false;
  selectedPaths.value = new Set();
}

function toggleSelect(path: string) {
  const next = new Set(selectedPaths.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  selectedPaths.value = next;
}

function isSelected(path: string): boolean {
  return selectedPaths.value.has(path);
}

const allSelected = computed(
  () => visiblePhotos.value.length > 0 && visiblePhotos.value.every((p) => selectedPaths.value.has(p.path))
);

function toggleSelectAll() {
  selectedPaths.value = allSelected.value
    ? new Set()
    : new Set(visiblePhotos.value.map((p) => p.path));
}

/** 打开二次确认弹窗（当前勾选的照片，两种删除模式） */
function requestDelete(mode: "record" | "file") {
  const paths = [...selectedPaths.value];
  if (!paths.length) return;
  confirmState.value = { mode, paths };
}

const confirmTitle = computed(() =>
  confirmState.value?.mode === "file" ? "删除本地文件" : "删除相册记录"
);

const confirmMessage = computed(() => {
  if (!confirmState.value) return "";
  const n = confirmState.value.paths.length;
  return confirmState.value.mode === "file"
    ? `将永久删除 ${n} 张照片的本地文件，此操作不可恢复！同时清除对应的扫描/AI 记录与缩略图缓存。确定继续吗？`
    : `将从本相册移除 ${n} 张照片并清除其扫描/AI 记录，本地文件保留、可重新扫描找回。确定继续吗？`;
});

/** 确认后执行删除：调后端命令 → 刷新网格 → 报告结果 */
async function doConfirmedDelete() {
  if (!confirmState.value || deleting.value) return;
  const { mode, paths } = confirmState.value;
  deleting.value = true;
  deleteMsg.value = "";
  try {
    const outcome =
      mode === "file"
        ? await albumStore.deletePhotoFiles(props.albumId, paths)
        : await albumStore.deletePhotoRecords(props.albumId, paths);
    deleteMsg.value =
      `已处理 ${outcome.deleted} / ${outcome.requested} 张` +
      (outcome.failed ? `，${outcome.failed} 张失败：${outcome.failed_paths.slice(0, 3).join("、")}` : "");
    exitSelectMode();
    await load(); // 重新拉取列表（后端已过滤被移除项）并回传新计数
  } catch (e) {
    deleteMsg.value = `删除失败：${String(e)}`;
  } finally {
    confirmState.value = null;
    deleting.value = false;
    // 结果提示停留几秒后自动消失
    if (deleteMsg.value) setTimeout(() => (deleteMsg.value = ""), 5000);
  }
}

function resetFilter() {
  activeCategory.value = null;
}

function refresh() {
  resetFilter();
  load();
}

defineExpose({ refresh });

// 筛选变化导致网格重建时，重置观察器与队列（重新渲染后由 registerCell 重新挂观察）；切筛选后回到顶部
watch(visiblePhotos, () => {
  nextTick(() => {
    cellObserver?.disconnect();
    cellObserver = null;
    pendingThumbs.clear();
    scrollEl.value?.scrollTo({ top: 0 });
  });
});

onMounted(() => {
  load();
  loadPersons();
});

onBeforeUnmount(() => {
  cellObserver?.disconnect();
});
</script>

<template>
  <section class="photo-grid-panel">
    <header class="pg-header">
      <h3 class="pg-title">照片浏览</h3>
      <span v-if="!loading" class="pg-count">{{ visiblePhotos.length }} / {{ photos.length }} 张</span>
      <button v-if="photos.length" class="btn pg-refresh" :class="{ 'pg-selecting': selectMode }" @click="toggleSelectMode">
        {{ selectMode ? "退出选择" : "选择" }}
      </button>
      <button v-if="photos.length && !selectMode" class="btn pg-refresh" title="重新加载" @click="refresh">刷新</button>
    </header>

    <!-- 多选模式操作条 -->
    <div v-if="selectMode" class="pg-select-bar">
      <label class="pg-select-all">
        <input type="checkbox" :checked="allSelected" @change="toggleSelectAll" /> 全选
      </label>
      <span class="pg-selected-count">已选 {{ selectedPaths.size }} 张</span>
      <span class="pg-select-tip">点击照片进行勾选；记录删除可恢复，文件删除不可恢复</span>
      <div class="pg-select-actions">
        <button class="btn" :disabled="!selectedPaths.size || deleting" @click="requestDelete('record')">删除相册记录</button>
        <button class="btn btn-danger-pg" :disabled="!selectedPaths.size || deleting" @click="requestDelete('file')">删除本地文件…</button>
      </div>
    </div>
    <div v-if="deleteMsg" class="pg-delete-msg">{{ deleteMsg }}</div>

    <!-- 扫描提示（未扫描时提示可叠加标签） -->
    <div v-if="!loading && photos.length && !hasAnyMeta()" class="pg-hint">
      <p>💡 照片已可浏览。运行「综合扫描」（EXIF/影调/AI）后，网格将叠加分类与人物标签，并支持智能筛选。</p>
    </div>

    <!-- 筛选条：仅大类 chips -->
    <div v-if="hasAnyMeta()" class="pg-filters">
      <div class="pg-filter-row">
        <span class="pg-filter-label">分类</span>
        <button
          class="chip"
          :class="{ active: activeCategory === null }"
          @click="activeCategory = null"
        >全部</button>
        <button
          v-for="c in categoryOptions"
          :key="c"
          class="chip"
          :class="{ active: activeCategory === c }"
          @click="activeCategory = activeCategory === c ? null : c"
        >{{ c }}</button>
      </div>
    </div>

    <!-- 加载中 -->
    <div v-if="loading" class="pg-loading">正在加载照片…</div>

    <!-- 加载失败 -->
    <div v-else-if="loadError" class="pg-error">加载失败：{{ loadError }}</div>

    <!-- 空相册 -->
    <div v-else-if="!photos.length" class="pg-empty">
      <p>该相册内暂无图片。</p>
    </div>

    <!-- 筛选后无结果 -->
    <div v-else-if="!visiblePhotos.length" class="pg-empty">
      <p>当前筛选条件下没有匹配的照片。</p>
      <button class="btn btn-primary" @click="resetFilter">清除筛选</button>
    </div>

    <!-- 照片子框：照片多时只在框内滚动，整页不动；右下角提供回到顶部 -->
    <div v-else class="pg-scroll" ref="scrollEl" @scroll.passive="onScroll">
      <div class="pg-grid">
      <figure
        v-for="(ph, i) in visiblePhotos"
        :key="ph.path"
        class="pg-cell"
        :class="{ 'pg-cell-selected': selectMode && isSelected(ph.path) }"
        :title="ph.path"
        :data-path="ph.path"
        :ref="(el) => registerCell(el, ph.path)"
        @click="openLightbox(i)"
      >
        <!-- 多选勾选框（仅选择模式显示） -->
        <span v-if="selectMode" class="pg-check" :class="{ on: isSelected(ph.path) }">
          {{ isSelected(ph.path) ? "✓" : "" }}
        </span>
        <div class="pg-thumb-wrap">
          <img
            v-if="thumbMap[ph.path]"
            :src="fileUrl(thumbMap[ph.path])"
            loading="lazy"
            decoding="async"
            alt=""
          />
          <div v-else class="pg-thumb-placeholder"></div>
        </div>
        <figcaption v-if="ph.meta?.category" class="pg-badge">{{ ph.meta.category }}</figcaption>
        <!-- 人物角标：头像小框 + 自定义命名（最多 3 人，超出 +N） -->
        <div v-if="(ph.meta?.person_ids ?? []).length" class="pg-person">
          <span v-for="pid in ph.meta!.person_ids.slice(0, MAX_BADGE_PERSONS)" :key="pid" class="pg-person-chip" :title="`${personName(pid)}（${pid}）`">
            <img v-if="avatarMap[pid]" :src="avatarMap[pid]" class="pg-person-avatar" alt="" />
            <span v-else class="pg-person-avatar pg-person-fallback">{{ personName(pid).slice(0, 1) }}</span>
            <span class="pg-person-name">{{ personName(pid) }}</span>
          </span>
          <span v-if="ph.meta!.person_ids.length > MAX_BADGE_PERSONS" class="pg-person-more">+{{ ph.meta!.person_ids.length - MAX_BADGE_PERSONS }}</span>
        </div>
      </figure>
    </div>

    <!-- 缩略图生成进度（后台异步，不阻塞滚动） -->
      <div v-if="thumbsLoading" class="pg-thumbs-loading">正在生成可视区域缩略图…</div>

      <!-- 回到顶部箭头 -->
      <transition name="pg-top">
        <button v-if="showTopBtn" class="pg-top-btn" title="回到顶部" @click="backToTop">↑</button>
      </transition>
    </div>

    <!-- 大图查看器：点击照片打开，按需加载单张原图（含详细信息面板与像素分布图） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="visiblePhotos"
      :index="lightboxIndex"
      :persons="personsMap"
      @close="closeLightbox"
    />

    <!-- 删除二次确认：明确区分两种模式及后果 -->
    <ConfirmDialog
      :visible="confirmState !== null"
      :title="confirmTitle"
      :message="confirmMessage"
      :confirm-text="confirmState?.mode === 'file' ? '永久删除文件' : '确认移除'"
      :danger="confirmState?.mode === 'file'"
      @confirm="doConfirmedDelete"
      @cancel="confirmState = null"
    />
  </section>
</template>

<style scoped>
.photo-grid-panel {
  margin-top: 24px;
  position: relative;
}

/* 子框内滚动：高度自适应视口，照片多时整页不再被拖长 */
.pg-scroll {
  position: relative;
  max-height: calc(100vh - 300px);
  min-height: 240px;
  overflow-y: auto;
  border: 1px solid #eceef2;
  border-radius: 10px;
  padding: 12px;
  background: #fafbfc;
  scrollbar-width: thin;
}

.pg-top-btn {
  position: sticky;
  bottom: 16px;
  left: calc(100% - 48px);
  display: block;
  width: 38px;
  height: 38px;
  margin-left: auto;
  border: none;
  border-radius: 50%;
  background: #396cd8;
  color: #fff;
  font-size: 17px;
  cursor: pointer;
  box-shadow: 0 4px 14px rgba(57, 108, 216, 0.45);
  transition: transform 0.15s, background 0.15s;
}
.pg-top-btn:hover {
  background: #2f5bc0;
  transform: translateY(-2px);
}

.pg-top-enter-active,
.pg-top-leave-active {
  transition: opacity 0.2s ease;
}
.pg-top-enter-from,
.pg-top-leave-to {
  opacity: 0;
}

.pg-header {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}

.pg-title {
  font-size: 16px;
  margin: 0;
  font-weight: 600;
}

.pg-count {
  color: #667085;
  font-size: 13px;
}

.pg-refresh {
  margin-left: auto;
  padding: 4px 12px;
  font-size: 13px;
}

.pg-hint {
  background: #f5f7ff;
  border: 1px solid #dbe3ff;
  color: #396cd8;
  border-radius: 8px;
  padding: 10px 14px;
  font-size: 13px;
  margin-bottom: 12px;
}
.pg-hint p { margin: 0; }

.pg-filters {
  margin-bottom: 14px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.pg-filter-row {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.pg-filter-label {
  font-size: 13px;
  color: #667085;
  margin-right: 2px;
}

.chip {
  border: 1px solid #ddd;
  background: #fff;
  border-radius: 999px;
  padding: 3px 12px;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
}
.chip:hover { border-color: #396cd8; color: #396cd8; }
.chip.active { background: #396cd8; border-color: #396cd8; color: #fff; }

.pg-loading, .pg-error, .pg-empty {
  text-align: center;
  padding: 40px 20px;
  color: #667085;
}
.pg-error { color: #e5484d; }

.pg-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 12px;
}

.pg-cell {
  position: relative;
  margin: 0;
  border-radius: 8px;
  overflow: hidden;
  cursor: pointer;
  background: #f2f4f7;
  border: 1px solid #eceef2;
  transition: transform 0.12s, box-shadow 0.12s;
}
.pg-cell:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.12);
}

.pg-thumb-wrap {
  aspect-ratio: 1 / 1;
  overflow: hidden;
}
.pg-thumb-wrap img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.pg-thumb-placeholder {
  width: 100%;
  height: 100%;
  background: repeating-linear-gradient(
    45deg,
    #eef1f5 0px,
    #eef1f5 8px,
    #e6eaf0 8px,
    #e6eaf0 16px
  );
}

.pg-badge {
  position: absolute;
  left: 6px;
  bottom: 6px;
  background: rgba(0, 0, 0, 0.6);
  color: #fff;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 999px;
  backdrop-filter: blur(2px);
}

.pg-person {
  position: absolute;
  right: 6px;
  bottom: 6px;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 3px;
  max-width: calc(100% - 12px);
}

.pg-person-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  max-width: 100%;
  background: rgba(0, 0, 0, 0.6);
  border-radius: 999px;
  padding: 2px 8px 2px 3px;
  backdrop-filter: blur(2px);
}

.pg-person-avatar {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  object-fit: cover;
  flex-shrink: 0;
  display: block;
}

.pg-person-fallback {
  background: #396cd8;
  color: #fff;
  font-size: 11px;
  line-height: 18px;
  text-align: center;
}

.pg-person-name {
  color: #fff;
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 90px;
}

.pg-person-more {
  background: rgba(0, 0, 0, 0.6);
  color: #fff;
  font-size: 11px;
  padding: 1px 7px;
  border-radius: 999px;
  backdrop-filter: blur(2px);
}

.pg-thumbs-loading {
  text-align: center;
  color: #667085;
  font-size: 12px;
  padding: 10px 0 4px;
}

/* ---- 多选删除 ---- */
.pg-refresh.pg-selecting {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
}

.pg-select-bar {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  background: #f5f7ff;
  border: 1px solid #dbe3ff;
  border-radius: 8px;
  padding: 8px 12px;
  margin-bottom: 10px;
  font-size: 13px;
}
.pg-select-all {
  display: flex;
  align-items: center;
  gap: 5px;
  cursor: pointer;
  user-select: none;
}
.pg-selected-count {
  font-weight: 600;
  color: #396cd8;
}
.pg-select-tip {
  color: #8a92a3;
  font-size: 12px;
}
.pg-select-actions {
  margin-left: auto;
  display: flex;
  gap: 8px;
}
.btn-danger-pg {
  background: #e5484d;
  border-color: #e5484d;
  color: #fff;
}
.btn-danger-pg:hover { background: #d03a3f; }
.btn-danger-pg:disabled,
.btn:disabled { opacity: 0.5; cursor: not-allowed; }

.pg-delete-msg {
  background: #fff8e6;
  border: 1px solid #ffe1a8;
  color: #8a6100;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 13px;
  margin-bottom: 10px;
}

.pg-cell-selected {
  outline: 3px solid #396cd8;
  outline-offset: -3px;
}
.pg-check {
  position: absolute;
  left: 6px;
  top: 6px;
  z-index: 2;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid #fff;
  background: rgba(0, 0, 0, 0.35);
  color: #fff;
  font-size: 13px;
  line-height: 18px;
  text-align: center;
  pointer-events: none;
}
.pg-check.on {
  background: #396cd8;
  border-color: #fff;
}
</style>
