<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import { useNotify } from "../composables/useNotify";
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

const props = defineProps<{ albumId: number; /** FEAT-034-B：需要滚动定位并高亮的照片路径（来自外部路由 query），加载完成后 1.8s 高亮 */
  focusPath?: string;
}>();
const emit = defineEmits<{
  (e: "count", n: number): void;
}>();

const albumStore = useAlbumStore();
const contentStore = useContentStore();
const notify = useNotify();

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

/** FEAT-034-B：当前高亮（focus）的照片 path，加载完成后 scrollIntoView + 1.8s 边框高亮 */
const highlightedPath = ref<string | null>(null);

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

/** FEAT-D：传给大图查看器的照片列表，每项带 albumId 以便 Lightbox 触发 ensure_photo_scanned */
const lightboxPhotos = computed(() =>
  visiblePhotos.value.map((p) => ({ path: p.path, meta: p.meta, albumId: props.albumId })),
);

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
    // 打分：一次性拉取当前相册照片的打分（不依赖扫描记录）
    try {
      const ratingRows = await albumStore.getPhotoRatings(paths);
      const rm: Record<string, number> = {};
      for (const [p, r] of ratingRows) rm[p] = r;
      ratingMap.value = rm;
    } catch {
      /* 打分服务不可用不影响浏览 */
    }
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

/* ---- 打分（星标） ---- */
/** path → 打分（0/缺失视为未打分） */
const ratingMap = ref<Record<string, number>>({});
const ratingOpen = ref(false);
const ratingPick = ref(5);
const busyRating = ref(false);

/** 打分弹窗状态统计：选中照片中已有分数的分布 */
const ratingStats = computed(() => {
  const paths = [...selectedPaths.value];
  const scores: number[] = [];
  for (const p of paths) {
    const r = ratingMap.value[p];
    if (r && r > 0) scores.push(r);
  }
  if (!scores.length) return { count: 0, max: 0, min: 0, avg: 0, hasRated: false };
  return {
    count: scores.length,
    max: Math.max(...scores),
    min: Math.min(...scores),
    avg: scores.reduce((a, b) => a + b, 0) / scores.length,
    hasRated: true,
  };
});

/** 打开打分：默认使用现有平均分（取整）以免突变 */
function openRating() {
  const avg = ratingStats.value.avg;
  ratingPick.value = avg >= 1 ? Math.round(avg) : 5;
  ratingOpen.value = true;
}

/* ---- 合并到其他相册 ---- */
const mergeOpen = ref(false);
const mergeTargetId = ref<number | null>(null);
const merging = ref(false);

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

/* ---- 批量导出：选文件夹 → 复制原图（可选生成信息清单）---- */
const exporting = ref(false);
async function doExport() {
  const paths = [...selectedPaths.value];
  if (!paths.length || exporting.value) return;
  try {
    const dir = await open({ directory: true, title: "选择导出目录" });
    if (!dir || typeof dir !== "string") return; // 用户取消
    exporting.value = true;
    const outcome = await albumStore.exportPhotos(paths, dir, true);
    notify.success(
      "导出完成",
      `已导出 ${outcome.copied} 张到 ${dir}` +
        (outcome.skipped ? `（跳过已存在 ${outcome.skipped} 张）` : "") +
        (outcome.failed ? `（${outcome.failed} 张失败：${outcome.failed_paths.slice(0, 3).join("、")}）` : ""),
    );
    exitSelectMode();
  } catch (e) {
    notify.error("导出失败", String(e));
  } finally {
    exporting.value = false;
  }
}

/* ---- 合并到其他相册 ---- */
/** 可作目标的相册（排除当前相册） */
const mergeCandidates = computed(() => albumStore.albums.filter((a) => a.id !== props.albumId));

function openMerge() {
  mergeTargetId.value = null;
  mergeOpen.value = true;
}

async function doMerge() {
  const target = mergeTargetId.value;
  const paths = [...selectedPaths.value];
  if (!target || !paths.length || merging.value) return;
  merging.value = true;
  try {
    const out = await albumStore.movePhotosToAlbum(props.albumId, paths, target);
    notify.success(
      "合并完成",
      `已移动 ${out.moved} / ${out.requested} 张到目标相册` +
        (out.failed ? `（${out.failed} 张失败）` : ""),
    );
    mergeOpen.value = false;
    exitSelectMode();
    await load();
    await albumStore.fetchAlbums();
  } catch (e) {
    notify.error("合并失败", String(e));
  } finally {
    merging.value = false;
  }
}

/* ---- 打分（星标）----
 * openRating / applyRating 已在上面以「打分统计」感知版定义。
 * 此处仅保留 applyRating。 */
async function applyRating() {
  const paths = [...selectedPaths.value];
  if (!paths.length || busyRating.value) return;
  busyRating.value = true;
  try {
    await albumStore.setPhotoRatings(paths, ratingPick.value);
    const next = { ...ratingMap.value };
    for (const p of paths) next[p] = ratingPick.value;
    ratingMap.value = next;
    notify.success("打分完成", `已为 ${paths.length} 张照片设置 ${ratingPick.value} 星`);
    ratingOpen.value = false;
    exitSelectMode();
  } catch (e) {
    notify.error("打分失败", String(e));
  } finally {
    busyRating.value = false;
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

/** FEAT-034-B：根据 focusPath 滚动定位并高亮 1.8s
 *  - wait 一下以确保格子已渲染（visiblePhotos 更新后）
 *  - 多次调用安全：后续以最后一次 focusPath 为准；高亮计时器会重置
 */
function applyFocus(focusPath: string | undefined) {
  if (!focusPath) return;
  const doScroll = () => {
    const root = scrollEl.value ?? document;
    const cell = root.querySelector<HTMLElement>(`[data-path="${CSS.escape(focusPath)}"]`);
    if (!cell) return false;
    cell.scrollIntoView({ behavior: "smooth", block: "center" });
    highlightedPath.value = focusPath;
    window.setTimeout(() => {
      // 避免覆盖后一次高亮
      if (highlightedPath.value === focusPath) highlightedPath.value = null;
    }, 1800);
    return true;
  };
  // 首轮重试：缩略图懒加载，DOM 可能刚挂载；最多重试 10 次 × 200ms（共 2s）。
  let tries = 0;
  const tryApply = () => {
    if (doScroll()) return;
    if (++tries > 10) return;
    window.setTimeout(tryApply, 200);
  };
  nextTick(tryApply);
}

/** FEAT-034-B：监听 focusPath 变化（路由 query 在同一相册内点击不同照片时） */
watch(
  () => props.focusPath,
  (p) => {
    if (p) applyFocus(p);
  },
);

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
  load().then(() => {
    // FEAT-034-B：首次加载完成后如有 focusPath（路由 query 携带）则滚动定位
    if (props.focusPath) applyFocus(props.focusPath);
  });
  loadPersons();
  // 预取相册列表（供「合并到相册」选择目标）
  albumStore.fetchAlbums().catch(() => {});
  window.addEventListener("keydown", onKey);
});

onBeforeUnmount(() => {
  cellObserver?.disconnect();
  window.removeEventListener("keydown", onKey);
});

/* ---------------- 选模式快捷键 ---------------- */
/**
 * 选模式下提供键盘交互，减少鼠标点按：
 *  - Esc 退出选模式
 *  - Ctrl/Cmd + A 全选/取消全选（仅当前可见区）
 *  - Delete 删除记录（默认「安全」语义，本地文件保留）
 *  输入框/弹窗/按 Enter 键时不拦截（避免误退/误删）。
 */
/* ---------------- 键盘交互 ---------------- */
/**
 * 选模式下提供快捷键：
 *  - Esc：优先关闭打开的弹窗；无弹窗则退出选模式
 *  - Ctrl/Cmd + A：全选/取消全选（仅当前可见区）
 *  - Delete：删除记录（默认「安全」语义，本地文件保留）
 * 输入框中不响应。
 */
function onKey(e: KeyboardEvent) {
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName;
  const editable = tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable;
  if (editable) return;

  // 打开的对话框优先处理 Esc：依次关闭评分 → 合并 → 删除确认
  if (e.key === "Escape") {
    if (ratingOpen.value) { e.preventDefault(); ratingOpen.value = false; return; }
    if (mergeOpen.value) { e.preventDefault(); mergeOpen.value = false; return; }
    if (confirmState.value) { e.preventDefault(); confirmState.value = null; return; }
  }

  if (!selectMode.value) return;
  if (mergeOpen.value || ratingOpen.value || confirmState.value) return;

  if (e.key === "Escape") {
    e.preventDefault();
    exitSelectMode();
  } else if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.key === "A")) {
    e.preventDefault();
    toggleSelectAll();
  } else if (e.key === "Delete" && selectedPaths.value.size > 0) {
    e.preventDefault();
    requestDelete("record");
  }
}
</script>

<template>
  <section class="photo-grid-panel">
    <header class="pg-header">
      <h3 class="pg-title">照片浏览</h3>
      <span v-if="!loading" class="pg-count">{{ visiblePhotos.length }} / {{ photos.length }} 张</span>
      <button
        v-if="photos.length"
        class="btn pg-action pg-action-select"
        :class="{ 'pg-selecting': selectMode }"
        @click="toggleSelectMode"
      >
        {{ selectMode ? "退出选择" : "选择" }}
      </button>
      <button
        v-if="photos.length && !selectMode"
        class="btn pg-action pg-action-refresh"
        title="重新加载"
        @click="refresh"
      >刷新</button>
    </header>

    <!-- 多选模式操作条 -->
    <div v-if="selectMode" class="pg-select-bar">
      <label class="pg-select-all">
        <input type="checkbox" :checked="allSelected" @change="toggleSelectAll" /> 全选
      </label>
      <span class="pg-selected-count">已选 {{ selectedPaths.size }} 张</span>
      <span class="pg-select-tip">
        点击照片勾选 · <kbd>Esc</kbd> 退出 · <kbd>Ctrl+A</kbd> 全选 · <kbd>Delete</kbd> 删记录
      </span>
      <div class="pg-select-actions">
        <button class="btn" :disabled="!selectedPaths.size || merging" @click="openMerge">合并到相册…</button>
        <button class="btn" :disabled="!selectedPaths.size || busyRating" @click="openRating">打分…</button>
        <button class="btn" :disabled="!selectedPaths.size || exporting" @click="doExport">导出选中…</button>
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
        :class="{ 'pg-cell-selected': selectMode && isSelected(ph.path), 'pg-cell-focus': highlightedPath === ph.path }"
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
        <!-- 打分星标（>0 才显示，左上角） -->
        <span v-if="ratingMap[ph.path] > 0" class="pg-rating" :title="`打分 ${ratingMap[ph.path]} 星`">{{ "★".repeat(ratingMap[ph.path]) }}</span>
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
      :photos="lightboxPhotos"
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

    <!-- 合并到其他相册：选择目标 -->
    <Teleport to="body">
      <div v-if="mergeOpen" class="pg-mask" @click.self="mergeOpen = false">
        <div class="pg-dialog">
          <h4>合并 {{ selectedPaths.size }} 张照片到…</h4>
          <p class="pg-dialog-tip">选择目标相册后，照片将物理移入该相册文件夹，并同步内容/打分记录。</p>
          <div class="pg-target-list">
            <button
              v-for="a in mergeCandidates"
              :key="a.id"
              class="pg-target-item"
              :class="{ active: mergeTargetId === a.id }"
              @click="mergeTargetId = a.id"
            >
              <span class="pg-target-name">{{ a.name }}</span>
              <span class="pg-target-meta">{{ a.photo_count }} 张·{{ a.path }}</span>
            </button>
            <p v-if="!mergeCandidates.length" class="pg-dialog-empty">暂无其他相册可合并。</p>
          </div>
          <div class="pg-dialog-actions">
            <button class="btn" @click="mergeOpen = false">取消</button>
            <button class="btn btn-primary" :disabled="!mergeTargetId || merging" @click="doMerge">
              {{ merging ? "合并中…" : "确认合并" }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- 打分：星标选择 -->
    <Teleport to="body">
      <div v-if="ratingOpen" class="pg-mask" @click.self="ratingOpen = false">
        <div class="pg-dialog">
          <h4>为 {{ selectedPaths.size }} 张照片打分</h4>
          <!-- 已打分数提示：避免用户不知会覆盖原分 -->
          <p v-if="ratingStats.hasRated" class="pg-rating-warn">
            其中 {{ ratingStats.count }} 张已打过 <b>{{ ratingStats.min }}–{{ ratingStats.max }} 星</b>
            （平均 {{ ratingStats.avg.toFixed(1) }}），本次评分将<strong>覆盖</strong>原有分数。
          </p>
          <p v-else class="pg-rating-hint">
            选中照片均未打分（平均取整默认 {{ ratingPick }} 星）。
          </p>
          <div class="pg-rating-picker">
            <button
              v-for="s in 5"
              :key="s"
              class="pg-star"
              :class="{ on: s <= ratingPick }"
              @click="ratingPick = s"
            >★</button>
            <span class="pg-rating-value">{{ ratingPick }} 星</span>
          </div>
          <div class="pg-dialog-actions">
            <button class="btn" @click="ratingOpen = false">取消</button>
            <button class="btn btn-primary" :disabled="busyRating" @click="applyRating">
              {{ busyRating ? "保存中…" : "确定打分" }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>
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

/* FEAT-C：选择/刷新按钮加重边框 + 高亮背景，让两个动作按钮一眼可见 */
.pg-action {
  margin-left: auto;
  padding: 6px 16px;
  font-size: 13px;
  font-weight: 600;
  border-radius: 8px;
  border: 1.5px solid #396cd8;
  background: #f0f5ff;
  color: #2f5cc2;
  box-shadow: 0 1px 3px rgba(57, 108, 216, 0.18);
  transition: background 0.15s, transform 0.1s, box-shadow 0.15s, border-color 0.15s;
}
.pg-action:hover:not(:disabled) {
  background: #e0ebff;
  border-color: #2f5cc2;
  color: #1f4caa;
  transform: translateY(-1px);
  box-shadow: 0 2px 6px rgba(57, 108, 216, 0.28);
}
.pg-action:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
/* 同一个类多实例时仅靠 margin-left:auto 可能会重叠，用 :not(:first-of-type) 给后续按钮加点左间距 */
.pg-action + .pg-action {
  margin-left: 8px;
}
/* 选择按钮与刷新按钮轻微区分：选择多一个微微高亮的虚线轮廓 */
.pg-action-select {
  border-style: solid;
}
.pg-action-refresh {
  border-style: dashed;
  background: #fafbff;
  color: #396cd8;
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
.pg-action.pg-selecting {
  background: #396cd8;
  border-color: #396cd8;
  color: #fff;
  border-style: solid;
  box-shadow: 0 2px 8px rgba(57, 108, 216, 0.35);
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
  position: sticky;
  top: 0;
  z-index: 5;
  backdrop-filter: blur(8px);
  box-shadow: 0 2px 10px rgba(57, 108, 216, 0.08);
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
  color: #5f6b7a;
  font-size: 12px;
}
.pg-select-tip kbd {
  display: inline-block;
  min-width: 22px;
  padding: 1px 6px;
  margin: 0 1px;
  font-size: 11px;
  font-family: inherit;
  line-height: 1.4;
  color: #4a5568;
  background: rgba(127, 127, 127, 0.12);
  border: 1px solid rgba(127, 127, 127, 0.3);
  border-radius: 4px;
  vertical-align: 1px;
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

/* FEAT-034-B：来自外部跳转的高亮（外部路由 query.focus） */
.pg-cell-focus {
  outline: 3px solid #ff9f1a;
  outline-offset: -3px;
  box-shadow: 0 0 0 6px rgba(255, 159, 26, 0.18);
  transition: box-shadow 0.6s ease-out;
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

/* 打分星标（左上方覆盖） */
.pg-rating {
  position: absolute;
  left: 6px;
  top: 6px;
  z-index: 2;
  background: rgba(0, 0, 0, 0.55);
  color: #ffd43b;
  font-size: 12px;
  line-height: 1;
  padding: 3px 7px;
  border-radius: 999px;
  backdrop-filter: blur(2px);
  letter-spacing: 1px;
  pointer-events: none;
}

/* 弹窗遮罩与对话框（合并/打分） */
.pg-mask {
  position: fixed;
  inset: 0;
  z-index: 1200;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
}
.pg-dialog {
  background: #fff;
  border-radius: 14px;
  padding: 18px 20px;
  width: min(460px, 92vw);
  max-height: 78vh;
  display: flex;
  flex-direction: column;
}
.pg-dialog h4 { margin: 0 0 6px; }
.pg-dialog-tip { font-size: 12px; color: #667085; margin: 0 0 12px; }
.pg-dialog-empty { text-align: center; opacity: 0.6; font-size: 13px; padding: 20px 0; }
.pg-target-list { overflow-y: auto; display: flex; flex-direction: column; gap: 8px; }
.pg-target-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  background: transparent;
  box-shadow: inset 0 0 0 1px rgba(128, 138, 158, 0.4);
  border-radius: 10px;
  padding: 8px 12px;
  cursor: pointer;
  text-align: left;
  transition: all 0.15s;
}
.pg-target-item:hover { border-color: #396cd8; }
.pg-target-item.active { border-color: #396cd8; background: rgba(57, 108, 216, 0.08); }
.pg-target-name { font-weight: 600; font-size: 14px; }
.pg-target-meta { font-size: 12px; opacity: 0.6; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.pg-dialog-actions { margin-top: 14px; display: flex; justify-content: flex-end; gap: 8px; }

/* 打分星标选择 */
.pg-rating-picker { display: flex; align-items: center; gap: 4px; padding: 6px 0 2px; }
.pg-star {
  background: transparent;
  border: none;
  font-size: 30px;
  color: #d3d7df;
  cursor: pointer;
  transition: color 0.12s, transform 0.12s;
}
.pg-star.on { color: #ffd43b; }
.pg-star:hover { transform: scale(1.12); }
.pg-rating-value { margin-left: 10px; font-size: 14px; color: #396cd8; font-weight: 600; }
.pg-rating-warn,
.pg-rating-hint {
  margin: 0 0 10px;
  font-size: 12.5px;
  line-height: 1.5;
  padding: 6px 10px;
  border-radius: 8px;
}
.pg-rating-warn {
  background: #fff8e6;
  border: 1px solid #ffe2a0;
  color: #6a4f00;
}
.pg-rating-warn strong { color: #d95a00; }
.pg-rating-hint {
  background: #eef3ff;
  border: 1px solid #dbe3ff;
  color: #3a4a6a;
}

</style>
