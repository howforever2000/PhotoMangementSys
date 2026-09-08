<script setup lang="ts">
/**
 * 地点画廊（FEAT-049）—— 智慧相册「地点」tab
 *
 * 数据源：photo_content_scan 的 GPS 坐标经 geo_index **离线**反查省/市
 * （万张 <1s，零网络），并回写 location 列作持久缓存（下次 0 反查成本）。
 * 无 GPS / 境外照片归入「未记录地点」组，弱化展示、固定排最后。
 *
 * 两级视图（与 CategoryGallery 同构）：
 *  1. 地点卡片网格：地名 + 张数徽标 + 代表封面
 *  2. 点击地点 → 照片网格 + PhotoLightbox 大图
 */
import { computed, onMounted, ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useThemeStore } from "../stores/theme";
import { useNotify } from "../composables/useNotify";
import PhotoLightbox from "./PhotoLightbox.vue";
import ContextMenu, { type ContextMenuEntry } from "./ContextMenu.vue";
import ConfirmDialog from "./ConfirmDialog.vue";
import type { ContentSearchHit, LocationGroupRow } from "../types/content";

const theme = useThemeStore();
const notify = useNotify();

const UNKNOWN_LABEL = "未记录地点";

/* -------------------- 数据 -------------------- */
const loading = ref(true);
const error = ref("");
const groups = ref<LocationGroupRow[]>([]);
const thumbMap = ref<Record<string, string>>({});

/** 有地名的组（未记录组由后端排最后，前端按 location === null 分流展示） */
const namedGroups = computed(() => groups.value.filter((g) => g.location !== null));
const unknownGroup = computed(() => groups.value.find((g) => g.location === null) ?? null);

async function load() {
  loading.value = true;
  error.value = "";
  try {
    groups.value = await invoke<LocationGroupRow[]>("list_photo_locations");
    await loadCoverThumbs();
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

async function loadCoverThumbs() {
  const byAlbum = new Map<number, string[]>();
  for (const g of groups.value) {
    if (!g.cover_path || thumbMap.value[g.cover_path]) continue;
    const aid = g.cover_album_id ?? 0;
    if (!byAlbum.has(aid)) byAlbum.set(aid, []);
    byAlbum.get(aid)!.push(g.cover_path);
  }
  await Promise.all(
    [...byAlbum.entries()].map(async ([aid, paths]) => {
      try {
        const pairs = await invoke<[string, string][]>("get_photo_thumbs", { albumId: aid, paths });
        for (const [path, thumb] of pairs) if (!thumbMap.value[path]) thumbMap.value[path] = thumb;
      } catch {
        /* 封面缺图不阻塞 */
      }
    }),
  );
}

/* -------------------- 视图 2：地点照片网格 -------------------- */
const activeGroup = ref<LocationGroupRow | null>(null);
const photos = ref<ContentSearchHit[]>([]);
const photosLoading = ref(false);

/** 地点卡片标题（None → 未记录地点） */
function groupLabel(g: LocationGroupRow): string {
  return g.location ?? UNKNOWN_LABEL;
}

async function openGroup(g: LocationGroupRow) {
  activeGroup.value = g;
  photosLoading.value = true;
  try {
    photos.value = await invoke<ContentSearchHit[]>("list_photos_by_location", {
      location: g.location,
    });
    await loadGridThumbs();
  } catch (e) {
    notify.error("加载地点照片失败", String(e));
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
        /* 单相册失败不阻塞 */
      }
    }),
  );
}

function backToCards() {
  activeGroup.value = null;
  photos.value = [];
}

/* -------------------- FEAT-050：删除（记录删除 / 回收站磁盘删除） -------------------- */
type DeleteMode = "records" | "trash";

const selectMode = ref(false);
const selected = ref<Set<string>>(new Set());
const modeDialogPaths = ref<string[] | null>(null);
const confirmVisible = ref(false);
const pendingDelete = ref<{ paths: string[]; mode: DeleteMode } | null>(null);

const ctxVisible = ref(false);
const ctxX = ref(0);
const ctxY = ref(0);
const ctxPath = ref("");
const ctxItems = computed<ContextMenuEntry[]>(() => [
  {
    label: "本地记录删除（保留文件）",
    icon: "📄",
    danger: true,
    onClick: () => askDelete([ctxPath.value], "records"),
  },
  {
    label: "磁盘删除（移入回收站）",
    icon: "🗑",
    danger: true,
    onClick: () => askDelete([ctxPath.value], "trash"),
  },
]);

function onCellContextMenu(e: MouseEvent, path: string) {
  if (selectMode.value) return;
  ctxX.value = e.clientX;
  ctxY.value = e.clientY;
  ctxPath.value = path;
  ctxVisible.value = true;
}

function toggleSelectMode() {
  selectMode.value = !selectMode.value;
  if (!selectMode.value) selected.value = new Set();
}
function toggleSelect(path: string) {
  const next = new Set(selected.value);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  selected.value = next;
}
function selectAll() {
  selected.value = new Set(photos.value.map((p) => p.path));
}
function clearSelection() {
  selected.value = new Set();
}

function openModeDialog(paths: string[]) {
  if (!paths.length) return;
  modeDialogPaths.value = paths;
}
function pickMode(mode: DeleteMode) {
  const paths = modeDialogPaths.value ?? [];
  modeDialogPaths.value = null;
  askDelete(paths, mode);
}

function askDelete(paths: string[], mode: DeleteMode) {
  if (!paths.length) return;
  pendingDelete.value = { paths, mode };
  confirmVisible.value = true;
}

const confirmTitle = computed(() =>
  pendingDelete.value?.mode === "records" ? "本地记录删除" : "磁盘删除（回收站）",
);
const confirmMessage = computed(() => {
  const pd = pendingDelete.value;
  if (!pd) return "";
  const n = pd.paths.length;
  return pd.mode === "records"
    ? `将删除 ${n} 张照片的扫描 / AI 记录与缩略图缓存，本地文件保留（重新扫描可恢复展示）。确定继续吗？`
    : `将把 ${n} 张照片移入系统回收站（可在回收站找回），并同步清除扫描 / AI 记录与缩略图缓存。确定继续吗？`;
});

function cancelDelete() {
  confirmVisible.value = false;
  pendingDelete.value = null;
}

async function executeDelete() {
  const pd = pendingDelete.value;
  if (!pd) return;
  confirmVisible.value = false;
  const cmd = pd.mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths: pd.paths });
    const removed = new Set(pd.paths.filter((p) => !outcome.failed_paths.includes(p)));
    photos.value = photos.value.filter((p) => !removed.has(p.path));
    const tm = { ...thumbMap.value };
    for (const p of removed) delete tm[p];
    thumbMap.value = tm;
    if (lightboxOpen.value) {
      if (!photos.value.length) lightboxOpen.value = false;
      else if (lightboxIndex.value >= photos.value.length)
        lightboxIndex.value = photos.value.length - 1;
    }
    await refreshGroups();
    if (activeGroup.value && !groups.value.some((g) => g.location === activeGroup.value!.location)) {
      backToCards();
    }
    if (outcome.failed > 0) {
      notify.warning(
        `已删除 ${outcome.deleted} / ${outcome.requested} 张`,
        `失败：${outcome.failed_paths.slice(0, 3).join("、")}${outcome.failed_paths.length > 3 ? "…" : ""}`,
      );
    } else {
      notify.success(
        `已删除 ${outcome.deleted} 张`,
        pd.mode === "trash" ? "文件已移入系统回收站" : "本地文件保留，仅清除记录",
      );
    }
  } catch (e) {
    notify.error("删除失败", String(e));
  } finally {
    pendingDelete.value = null;
    selected.value = new Set();
  }
}

async function refreshGroups() {
  try {
    groups.value = await invoke<LocationGroupRow[]>("list_photo_locations");
    await loadCoverThumbs();
  } catch {
    /* 计数刷新失败不阻塞 */
  }
}

/* -------------------- 大图看图器 -------------------- */
const lightboxOpen = ref(false);
const lightboxIndex = ref(0);
const lightboxPhotos = computed(() =>
  photos.value.map((p) => ({ path: p.path, albumId: p.album_id })),
);
function openLightbox(p: ContentSearchHit) {
  const idx = photos.value.findIndex((x) => x.path === p.path);
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
  <div class="lg-wrap" :style="{ color: theme.textColor }">
    <!-- ============ 视图 1：地点卡片 ============ -->
    <template v-if="!activeGroup">
      <div v-if="loading" class="lg-state">
        <div class="sk-grid">
          <div v-for="i in 6" :key="i" class="sk-card"></div>
        </div>
      </div>

      <div v-else-if="error" class="lg-state">
        <div class="lg-state-icon">⚠️</div>
        <p>加载失败：{{ error }}</p>
        <button class="btn" @click="load">重试</button>
      </div>

      <div v-else-if="!groups.length" class="lg-state">
        <div class="lg-state-icon">📍</div>
        <p class="lg-state-title">还没有可聚合的照片</p>
        <p class="lg-state-text">请先在相册详情页执行「综合扫描」，照片的 GPS 定位会离线解析为省 / 市并聚合展示。</p>
      </div>

      <template v-else>
        <p class="lg-hint">
          依据照片 EXIF GPS 离线解析（省 / 市级）；无定位的照片（截图 / 文档类常见）归入「{{ UNKNOWN_LABEL }}」。
        </p>
        <div class="loc-grid">
          <article
            v-for="g in namedGroups"
            :key="g.location ?? 'unknown'"
            class="loc-card"
            :style="theme.cardStyle"
            :title="`${groupLabel(g)} · ${g.count} 张`"
            @click="openGroup(g)"
          >
            <div class="loc-cover">
              <img v-if="g.cover_path && thumbMap[g.cover_path]" :src="fileUrl(thumbMap[g.cover_path])" loading="lazy" alt="" />
              <span v-else class="loc-cover-ph">📍</span>
              <span class="loc-count">{{ g.count }} 张</span>
            </div>
            <div class="loc-body">
              <h3 class="loc-name">{{ groupLabel(g) }}</h3>
            </div>
          </article>
        </div>
        <!-- 未记录地点：弱化卡片，固定排最后 -->
        <div v-if="unknownGroup" class="loc-unknown-row">
          <button class="loc-unknown" :style="theme.cardStyle" @click="openGroup(unknownGroup)">
            <span class="loc-unknown-icon">🗺️</span>
            <span class="loc-unknown-name">{{ UNKNOWN_LABEL }}</span>
            <span class="loc-unknown-count">{{ unknownGroup.count }} 张 · 无定位 / 境外</span>
          </button>
        </div>
      </template>
    </template>

    <!-- ============ 视图 2：地点照片网格 ============ -->
    <template v-else>
      <div class="lg-detail-head">
        <button class="btn" @click="backToCards">← 返回地点</button>
        <h2 class="lg-detail-title">📍 {{ groupLabel(activeGroup) }}</h2>
        <span class="lg-detail-count">{{ activeGroup.count }} 张</span>
        <span class="lg-spacer"></span>
        <button class="btn" :class="{ active: selectMode }" @click="toggleSelectMode">
          {{ selectMode ? "退出批量" : "☑ 批量管理" }}
        </button>
        <template v-if="selectMode">
          <button class="btn" @click="selectAll">全选</button>
          <button class="btn" @click="clearSelection">取消全选</button>
          <button class="btn btn-danger" :disabled="!selected.size" @click="openModeDialog([...selected])">
            🗑 删除选中（{{ selected.size }}）
          </button>
        </template>
      </div>

      <div v-if="photosLoading" class="lg-state">
        <div class="sk-grid">
          <div v-for="i in 8" :key="i" class="sk-card sk-photo"></div>
        </div>
      </div>
      <div v-else-if="!photos.length" class="lg-state">
        <div class="lg-state-icon">🗂</div>
        <p>该地点下暂无照片</p>
      </div>
      <div v-else class="photo-grid">
        <figure
          v-for="p in photos"
          :key="p.id"
          class="photo-cell"
          :class="{ selectable: selectMode, checked: selectMode && selected.has(p.path) }"
          :title="[p.label, p.shoot_time].filter(Boolean).join(' · ')"
          @click="selectMode ? toggleSelect(p.path) : openLightbox(p)"
          @contextmenu.prevent="onCellContextMenu($event, p.path)"
        >
          <img v-if="thumbMap[p.path]" :src="fileUrl(thumbMap[p.path])" loading="lazy" alt="" />
          <div v-else class="photo-ph">🖼</div>
          <span v-if="selectMode" class="cell-check" :class="{ on: selected.has(p.path) }">
            {{ selected.has(p.path) ? "✓" : "" }}
          </span>
        </figure>
      </div>
    </template>

    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      deletable
      @close="lightboxOpen = false"
      @delete="openModeDialog([$event])"
    />

    <!-- 右键菜单（复用 FEAT-043 组件） -->
    <ContextMenu :items="ctxItems" :x="ctxX" :y="ctxY" @close="ctxVisible = false" />

    <!-- 删除方式选择（批量 / 预览删除） -->
    <Teleport to="body">
      <div v-if="modeDialogPaths" class="del-mask" @click.self="modeDialogPaths = null">
        <div class="del-dialog" :style="theme.cardStyle">
          <h4>选择删除方式（{{ modeDialogPaths.length }} 张）</h4>
          <button class="del-opt" @click="pickMode('records')">
            <b>📄 本地记录删除</b>
            <span>清除扫描 / AI 记录与缩略图缓存，本地文件保留</span>
          </button>
          <button class="del-opt" @click="pickMode('trash')">
            <b>🗑 磁盘删除（移入回收站）</b>
            <span>照片移入系统回收站，可找回；记录与缓存同步清除</span>
          </button>
          <button class="btn del-cancel" @click="modeDialogPaths = null">取消</button>
        </div>
      </div>
    </Teleport>

    <!-- 二次确认 -->
    <ConfirmDialog
      :visible="confirmVisible"
      :title="confirmTitle"
      :message="confirmMessage"
      confirm-text="确认删除"
      @confirm="executeDelete"
      @cancel="cancelDelete"
    />
  </div>
</template>

<style scoped>
.lg-wrap {
  min-width: 0;
}

.lg-state {
  padding: 30px 10px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}
.lg-state-icon {
  font-size: 44px;
}
.lg-state-title {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
}
.lg-state-text {
  opacity: 0.7;
  font-size: 13px;
  max-width: 420px;
  line-height: 1.6;
  margin: 0;
}
.lg-hint {
  font-size: 12px;
  opacity: 0.6;
  margin: 0 0 12px;
}

.sk-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
  gap: 14px;
  width: 100%;
}
.sk-card {
  height: 170px;
  border-radius: 14px;
  background: rgba(127, 127, 127, 0.16);
  animation: lgpulse 1.2s infinite;
}
.sk-photo {
  height: 130px;
}
@keyframes lgpulse {
  50% { opacity: 0.4; }
}

.loc-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
  gap: 16px;
}
.loc-card {
  border-radius: 14px;
  overflow: hidden;
  cursor: pointer;
  transition: transform 0.16s ease, box-shadow 0.16s ease;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.1);
}
.loc-card:hover {
  transform: translateY(-3px);
  box-shadow: 0 10px 24px rgba(0, 0, 0, 0.18);
}
.loc-cover {
  position: relative;
  aspect-ratio: 4 / 3;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(135deg, #43cea2 0%, #185a9d 100%);
}
.loc-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.loc-cover-ph {
  font-size: 36px;
  opacity: 0.85;
}
.loc-count {
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
.loc-body {
  padding: 10px 12px 12px;
}
.loc-name {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
}

/* 未记录地点：弱化横条 */
.loc-unknown-row {
  margin-top: 16px;
}
.loc-unknown {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  border-radius: 12px;
  border: 1px dashed rgba(127, 127, 127, 0.45);
  background: transparent;
  color: inherit;
  cursor: pointer;
  text-align: left;
  transition: border-color 0.15s;
}
.loc-unknown:hover {
  border-color: rgba(106, 141, 240, 0.7);
}
.loc-unknown-icon {
  font-size: 20px;
  opacity: 0.7;
}
.loc-unknown-name {
  font-size: 14px;
  font-weight: 600;
}
.loc-unknown-count {
  font-size: 12px;
  opacity: 0.6;
  margin-left: auto;
}

.lg-detail-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
  flex-wrap: wrap;
}
.lg-detail-title {
  margin: 0;
  font-size: 19px;
  font-weight: 700;
}
.lg-detail-count {
  font-size: 12.5px;
  opacity: 0.65;
}

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

@media (max-width: 640px) {
  .loc-grid { grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 10px; }
  .photo-grid { grid-template-columns: repeat(auto-fill, minmax(110px, 1fr)); }
}

/* ---- FEAT-050：批量选择 / 删除 ---- */
.lg-spacer {
  flex: 1;
}
.btn.active {
  border-color: rgba(106, 141, 240, 0.8);
  color: #6a8df0;
}
.btn-danger {
  color: #e03131;
  border-color: rgba(224, 49, 49, 0.5);
}
.btn-danger:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.photo-cell.selectable {
  cursor: pointer;
}
.photo-cell.checked img {
  opacity: 0.55;
}
.photo-cell.checked {
  outline: 3px solid rgba(106, 141, 240, 0.85);
  outline-offset: -3px;
}
.cell-check {
  position: absolute;
  left: 7px;
  top: 7px;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 2px solid rgba(255, 255, 255, 0.9);
  background: rgba(0, 0, 0, 0.35);
  color: #fff;
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
}
.cell-check.on {
  background: #4c8dff;
  border-color: #4c8dff;
}
.del-mask {
  position: fixed;
  inset: 0;
  z-index: 1100;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
}
.del-dialog {
  width: min(420px, 92vw);
  border-radius: 14px;
  padding: 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.28);
}
.del-dialog h4 {
  margin: 0 0 4px;
  font-size: 16px;
}
.del-opt {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  padding: 12px 14px;
  border-radius: 10px;
  border: 1px solid rgba(127, 127, 127, 0.32);
  background: transparent;
  color: inherit;
  cursor: pointer;
  text-align: left;
  transition: border-color 0.15s, background 0.15s;
}
.del-opt:hover {
  border-color: rgba(106, 141, 240, 0.75);
  background: rgba(106, 141, 240, 0.08);
}
.del-opt span {
  font-size: 12px;
  opacity: 0.65;
  line-height: 1.5;
}
.del-cancel {
  align-self: flex-end;
}
</style>
