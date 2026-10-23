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
import PhotoActionMenu from "./PhotoActionMenu.vue";
import PhotoDeleteDialog, { type DeleteMode } from "./PhotoDeleteDialog.vue";
import SelectAllCheckbox from "./SelectAllCheckbox.vue";
import LocationMap from "./ui/LocationMap.vue";
import StatBand, { type StatItem } from "./ui/StatBand.vue";
import { usePhotoActions } from "../composables/usePhotoActions";
import { trace } from "../utils/trace";
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

/**
 * FEAT-103：地点页视图切换（地图 / 卡片）
 *
 * 为什么默认地图：地点数据的价值在于「空间分布」，而卡片网格只能回答
 * "有多少个城市"，回答不了"它们在哪、有多密集"。地图一眼能看出
 * "我的照片集中在一两个城市" 或 "遍布全国" —— 这是卡片做不到的。
 * 但卡片视图保留（切换）：找具体某个城市的照片时，列表比在地图上瞄准更快。
 */
type LocView = "map" | "list";
const view = ref<LocView>("map");
/** 地图上被点击的城市名（用于同步高亮卡片、以及打开详情） */
const mapActive = ref<string | null>(null);

/** 有 GPS 坐标的城市组（地图能用得上的那些） */
const mappedGroups = computed(() => namedGroups.value.filter((g) => g.lat != null && g.lon != null));

/** 无坐标的城市数（有地名但没存坐标 —— 老数据/手工填的地点） */
const noCoordCount = computed(() => namedGroups.value.length - mappedGroups.value.length);

/**
 * 顶部统计数据带（FEAT-103：统计不删，只去壳）
 * 这几项是"地点功能是否在工作"的唯一可读证据：
 *   有地点城市 / GPS 覆盖 / 未记录 / 最大聚集
 * 尤其是 **GPS 覆盖** —— UX 评审指出用户看到"0 个城市"时分不清
 * 是"照片没 GPS"还是"功能坏了"。给出百分比一眼可判。
 */
const statItems = computed<StatItem[]>(() => {
  const total = groups.value.reduce((n, g) => n + g.count, 0);
  const unknown = unknownGroup.value?.count ?? 0;
  const covered = total - unknown;
  const pct = total > 0 ? (covered / total) * 100 : 0;
  const max = mappedGroups.value.reduce<(typeof mappedGroups.value)[number] | null>(
    (best, g) => (best === null || g.count > best.count ? g : best),
    null,
  );
  return [
    { label: "有地点城市", value: mappedGroups.value.length, delta: mappedGroups.value.length ? "已点亮" : "无坐标" },
    {
      label: "GPS 覆盖",
      value: pct.toFixed(1),
      unit: "%",
      delta: `${covered} / ${total}`,
      tone: pct > 0 ? "up" : "mute",
    },
    { label: "未记录", value: unknown, delta: "无定位", tone: "mute" },
    ...(max
      ? [
          {
            label: "最大聚集",
            value: max.count,
            unit: "张",
            delta: (max.location ?? "").replace(/[省市]$/, ""),
          },
        ]
      : []),
  ];
});

/** 地图点位点击：直接打开该城市的照片网格（与卡片点击同一路径） */
function onMapPick(g: { location: string | null }) {
  mapActive.value = g.location;
  const hit = namedGroups.value.find((x) => x.location === g.location);
  if (hit) openGroup(hit);
}

async function load() {
  loading.value = true;
  error.value = "";
  // CS2：关键数据入口带出入口日志（含耗时与错误）
  const done = trace("LocationGallery.load", async () => {
    groups.value = await invoke<LocationGroupRow[]>("list_photo_locations");
    await loadCoverThumbs();
  });
  try {
    await done();
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
// FEAT-095：DeleteMode 由 PhotoDeleteDialog 统一导出，本组件复用不再重复定义

const selectMode = ref(false);
const selected = ref<Set<string>>(new Set());
/** 删除方式选择弹窗（两种选择即最终确认，点击方式立即执行） */
const modeDialogPaths = ref<string[] | null>(null);
/** FEAT-095：归档 / 隐藏（与全站共用同一套语义） */
const photoActions = usePhotoActions();

const ctxTarget = ref<{ x: number; y: number; paths: string[] } | null>(null);

function onCellContextMenu(e: MouseEvent, path: string) {
  if (selectMode.value) return;
  ctxTarget.value = { x: e.clientX, y: e.clientY, paths: [path] };
}

/** 右键动作完成后的刷新：归档 / 隐藏会改变聚合计数，需重拉分组 */
async function onCtxRefresh() {
  await refreshGroups();
  if (activeGroup.value && !groups.value.some((g) => g.location === activeGroup.value!.location)) {
    backToCards();
  }
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
/**
 * 全选 / 取消全选（FEAT-096：参数为「本次点击意图」，由三态复选框算好传进来）
 *
 * @param selectAll `true` = 全选；`false` = 取消全选
 */
function selectAll(selectAll = true) {
  selected.value = selectAll ? new Set(photos.value.map((p) => p.path)) : new Set();
}

function openModeDialog(paths: string[]) {
  if (!paths.length) return;
  modeDialogPaths.value = paths;
}
function pickMode(mode: DeleteMode) {
  const paths = modeDialogPaths.value ?? [];
  void executeDelete(paths, mode);
}

/** 删除执行中（禁用共享弹窗按钮，防重复提交） */
const deleting = ref(false);

async function executeDelete(paths: string[], mode: DeleteMode) {
  if (!paths.length || deleting.value) return;
  deleting.value = true;
  const cmd = mode === "records" ? "delete_photo_records_by_paths" : "delete_photos_to_trash";
  try {
    const outcome = await invoke<{
      requested: number;
      deleted: number;
      failed: number;
      failed_paths: string[];
    }>(cmd, { paths });
    const removed = new Set(paths.filter((p) => !outcome.failed_paths.includes(p)));
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
        mode === "trash" ? "文件已移入系统回收站" : "本地文件保留，仅清除记录",
      );
    }
  } catch (e) {
    notify.error("删除失败", String(e));
  } finally {
    modeDialogPaths.value = null;
    deleting.value = false;
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

/** FEAT-095：大图内归档 / 隐藏后刷新聚合计数 */
async function onLightboxMark(kind: "archived" | "hidden", path: string, marked: boolean) {
  const ok = await photoActions.apply(kind, [path], marked);
  if (!ok) return;
  if (marked) {
    // 照片已移出当前地点视图 → 关闭大图并刷新
    photos.value = photos.value.filter((x) => x.path !== path);
    if (!photos.value.length) lightboxOpen.value = false;
    else lightboxIndex.value = Math.min(lightboxIndex.value, photos.value.length - 1);
  }
  await onCtxRefresh();
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
        <!-- FEAT-103：统计带（统计不删，只去壳）—— 含 GPS 覆盖率，
             让"0 个城市"能区分开"照片没定位"与"功能坏了" -->
        <StatBand :items="statItems" class="lg-stats" />

        <!-- 视图切换：地图 / 卡片。
             地图回答"在哪、多密"，卡片回答"有哪些" —— 两种问题都要能回答 -->
        <div class="lg-viewbar">
          <div class="lg-seg" role="group" aria-label="地点视图">
            <button
              class="lg-seg-btn"
              :class="{ on: view === 'map' }"
              type="button"
              :aria-pressed="view === 'map'"
              @click="view = 'map'"
            >
              地图
            </button>
            <button
              class="lg-seg-btn"
              :class="{ on: view === 'list' }"
              type="button"
              :aria-pressed="view === 'list'"
              @click="view = 'list'"
            >
              卡片
            </button>
          </div>
          <p class="lg-hint lg-hint-inline">
            依据照片 EXIF GPS 离线解析（省 / 市级）
            <template v-if="noCoordCount > 0">；{{ noCoordCount }} 个地点无坐标，无法上图</template>
          </p>
        </div>

        <!-- —— 地图视图 ——
             空即信息：没有点的地方就留空，不画占位、不画假点。
             这正是让"未覆盖"可见的方式（UX 评审：空转必须有解释）。 -->
        <div v-if="view === 'map'" class="lg-map-wrap">
          <LocationMap
            :groups="groups"
            :width="820"
            :active-name="mapActive"
            @pick="onMapPick"
          />
        </div>

        <!-- —— 卡片视图 —— -->
        <div v-else class="loc-grid">
          <article
            v-for="g in namedGroups"
            :key="g.location ?? 'unknown'"
            class="loc-card glass-card"
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

        <!-- 未记录地点：两种视图都展示（它是真实存在的照片，不是错误） -->
        <div v-if="unknownGroup" class="loc-unknown-row">
          <button class="loc-unknown glass-card" :style="theme.cardStyle" @click="openGroup(unknownGroup)">
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
          <!-- FEAT-096：三态全选复选框（空圈 → 打勾；全选后再点即取消全选，取代原「全选 / 取消全选」两按钮） -->
          <SelectAllCheckbox
            :selected-count="selected.size"
            :total-count="photos.length"
            @toggle="selectAll"
          />
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

    <!-- FEAT-095：原图预览同样启用归档 / 隐藏（与缩略图右键菜单功能对齐） -->
    <PhotoLightbox
      v-if="lightboxOpen"
      :photos="lightboxPhotos"
      :index="lightboxIndex"
      deletable
      markable
      @close="lightboxOpen = false"
      @delete="openModeDialog([$event])"
      @archive="(p, m) => onLightboxMark('archived', p, m)"
      @hide="(p, m) => onLightboxMark('hidden', p, m)"
    />

    <!-- FEAT-095（R8）：右键菜单统一走 PhotoActionMenu（打分/标签/编辑/归档/隐藏/删除） -->
    <PhotoActionMenu
      :target="ctxTarget"
      :on-refresh="onCtxRefresh"
      :on-close="() => (ctxTarget = null)"
    />

    <!-- FEAT-095（R7 / D1）：批量删除方式选择，复用共享弹窗 -->
    <PhotoDeleteDialog
      :paths="modeDialogPaths"
      :busy="deleting"
      @pick="pickMode"
      @cancel="modeDialogPaths = null"
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
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
  margin: 0 0 12px;
  letter-spacing: var(--ls-meta);
}

/* ===== FEAT-103：地点页新骨架（数据带 + 视图切换 + 地图） ===== */

/* 统计带：贴齐页面左右留白（--sp-8），但负 margin 抵消父级 padding。
   这里不用 .lg-wrap 直接改 padding —— 它被 CategoryGallery 等其他视图复用同构样式，
   改父级会连带影响它们。 */
.lg-stats {
  margin: 0 calc(var(--sp-8) * -1) 4px;
}

.lg-viewbar {
  display: flex;
  align-items: center;
  gap: var(--sp-4);
  margin: var(--sp-3) 0 var(--sp-4);
  flex-wrap: wrap;
}

/* 分段控件：两选项。用 1px 内描边 + 选中金底，
   不做"滑块跟随"动画 —— 两段的位移暗示在这里没必要，反而显得吵。 */
.lg-seg {
  display: inline-flex;
  padding: 2px;
  border-radius: var(--r-pill);
  background: rgba(255, 255, 255, 0.05);
  box-shadow: inset 0 0 0 1px var(--c-rule);
}
.lg-seg-btn {
  min-height: 26px;
  padding: 0 14px;
  border-radius: var(--r-pill);
  font-size: var(--fs-body);
  color: var(--c-fg-2);
  background: transparent;
  cursor: pointer;
  transition:
    background var(--dur-2) var(--ease-out),
    color var(--dur-2) var(--ease-out);
}
.lg-seg-btn.on {
  background: var(--c-accent);
  color: #1a1206;
  font-weight: var(--fw-em);
}

.lg-hint-inline {
  margin: 0;
}

.lg-map-wrap {
  display: flex;
  justify-content: center;
  padding: var(--sp-2) 0 var(--sp-5);
}

body:not(.theme-dark) .lg-seg {
  background: rgba(23, 22, 26, 0.04);
}
body:not(.theme-dark) .lg-seg-btn.on {
  color: #fff;
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
  transition: transform var(--dur-2) ease, box-shadow var(--dur-2) ease;
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
  background: linear-gradient(135deg, #12332a 0%, #050f0c 100%);
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
  transition: border-color var(--dur-2);
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
  transition: transform var(--dur-1) ease;
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
  border-color: var(--color-link);
  color: var(--color-link);
}
.btn-danger {
  color: var(--color-danger-text);
  border-color: color-mix(in srgb, var(--color-danger-text) 50%, transparent);
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
</style>
