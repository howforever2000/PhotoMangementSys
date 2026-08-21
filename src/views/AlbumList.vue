<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import type { Album, CreateAlbumInput } from "../types/album";
import type { ContentSearchHit } from "../types/content";
import { groupByTime, seasonName, MONTH_NAMES } from "../utils/timeGroup";
import type { YearGroup } from "../utils/timeGroup";
import ManualSort from "./ManualSort.vue";
import AlbumCard from "../components/AlbumCard.vue";
import ConfirmDialog from "../components/ConfirmDialog.vue";

const router = useRouter();
const route = useRoute();
const store = useAlbumStore();
const contentStore = useContentStore();

// ---------- 回到顶部按钮状态 ----------
const scrollTop = ref(0);
let rafId: number | null = null;

/** 节流监听页面滚动 */
function onScroll() {
  if (rafId != null) return;
  rafId = requestAnimationFrame(() => {
    scrollTop.value = window.scrollY;
    rafId = null;
  });
}

const showBackToTop = computed(() => scrollTop.value > 300);

/** 平滑滚动回顶部 */
function scrollToTop() {
  window.scrollTo({ top: 0, behavior: "smooth" });
}

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

/** 在系统文件管理器中打开文件夹内部（需求：地址可点击进入目录） */
async function openAlbumPath(path: string, event: MouseEvent) {
  event.stopPropagation(); // 阻止卡片点击跳转详情页
  try {
    await invoke("open_folder", { path });
  } catch (e) {
    alert(`无法打开文件夹：${path}\n\n${e}`);
  }
}

// ---------- 批量导入状态 ----------
const isImporting = ref(false);
const importProgress = ref(0); // 0-100
const importStatus = ref(""); // 进度提示文案
let unlistenImport: (() => void) | null = null;

/**
 * 批量导入：选择一个大文件夹，遍历其一级子文件夹创建相册
 * 通过监听后端 import-progress 事件实时更新进度条
 */
async function batchImport() {
  if (isImporting.value) return;
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择要批量导入的根文件夹",
  });
  if (typeof selected !== "string") return;

  isImporting.value = true;
  importProgress.value = 0;
  importStatus.value = "准备中…";

  // 监听后端进度事件
  unlistenImport = await listen<{
    current: number;
    total: number;
    imported: number;
    current_name: string;
  }>("import-progress", (event) => {
    const { current, total, imported, current_name } = event.payload;
    importProgress.value = total > 0 ? Math.round((current / total) * 100) : 100;
    importStatus.value = `正在处理 ${current_name}（${current}/${total}），已导入 ${imported} 个`;
  });

  try {
    const result = await store.importAlbums(selected);
    importProgress.value = 100;
    const parts = [`成功导入 ${result.imported} 个相册`];
    if (result.skipped > 0) parts.push(`跳过 ${result.skipped} 个已存在的`);
    if (result.errors.length > 0) {
      parts.push(`失败 ${result.errors.length} 个`);
      console.error("导入失败的文件夹:", result.errors);
    }
    alert(parts.join("，"));
  } catch (e) {
    alert(`批量导入失败：${e}`);
  } finally {
    if (unlistenImport) {
      unlistenImport();
      unlistenImport = null;
    }
    isImporting.value = false;
    importProgress.value = 0;
    importStatus.value = "";
  }
}

// ---------- 勾选 / 批量删除状态 ----------
const selectedIds = ref<Set<number>>(new Set());
const isSelectMode = ref(false); // 是否处于勾选管理模式
const isDeleting = ref(false);

/** 是否全部选中 */
const allSelected = computed(
  () => store.albums.length > 0 && selectedIds.value.size === store.albums.length,
);

/** 进入勾选管理模式（默认全不选） */
function enterSelectMode() {
  isSelectMode.value = true;
}

/** 切换单个相册的勾选状态 */
function toggleSelect(id: number, event?: MouseEvent) {
  event?.stopPropagation(); // 阻止触发卡片点击跳转
  const next = new Set(selectedIds.value);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  selectedIds.value = next;
}

/** 全选 / 取消全选 */
function toggleSelectAll() {
  if (allSelected.value) {
    selectedIds.value = new Set();
  } else {
    selectedIds.value = new Set(store.albums.map((a) => a.id));
  }
}

/** 退出勾选管理模式，清空所有选择 */
function exitSelectMode() {
  selectedIds.value = new Set();
  isSelectMode.value = false;
}

/** 批量删除：二次确认后仅删数据库记录，不删本地文件 */
const batchDeleteConfirm = ref<{ visible: boolean; message: string }>({ visible: false, message: "" });
async function batchDelete() {
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;
  if (isDeleting.value) return;

  batchDeleteConfirm.value = {
    visible: true,
    message: `确定要删除选中的 ${ids.length} 个相册吗？\n\n此操作仅删除相册记录，不会删除本地照片文件。`,
  };
}
/** 确认后真正执行批量删除 */
async function doBatchDelete() {
  batchDeleteConfirm.value.visible = false;
  const ids = [...selectedIds.value];
  if (ids.length === 0) return;
  if (isDeleting.value) return;

  isDeleting.value = true;
  try {
    const deleted = await store.deleteAlbums(ids);
    alert(`已删除 ${deleted} 个相册`);
    exitSelectMode();
  } catch (e) {
    alert(`删除失败：${e}`);
  } finally {
    isDeleting.value = false;
  }
}

/** 右键菜单状态 */
const contextMenu = ref<{ visible: boolean; x: number; y: number; albumId: number }>({
  visible: false,
  x: 0,
  y: 0,
  albumId: 0,
});
const isDeletingOne = ref(false);

// ---------- 重命名相册 ----------
const showRenameDialog = ref(false);
const renameInput = ref("");
const isRenaming = ref(false);

/** 打开重命名对话框（右键菜单） */
function openRenameDialog() {
  const id = contextMenu.value.albumId;
  closeContextMenu();
  const album = store.albums.find((a) => a.id === id);
  if (!album) return;
  renameInput.value = album.name;
  showRenameDialog.value = true;
}

/** 提交重命名 */
async function submitRename() {
  const id = contextMenu.value.albumId;
  const name = renameInput.value.trim();
  if (!name) {
    alert("相册名称不能为空");
    return;
  }
  if (name.length > 100) {
    alert("相册名称不能超过 100 个字符");
    return;
  }
  if (isRenaming.value) return;
  isRenaming.value = true;
  try {
    await store.renameAlbum(id, name, true);
    showRenameDialog.value = false;
  } catch (e) {
    alert(`重命名失败：${e}`);
  } finally {
    isRenaming.value = false;
  }
}

/** 右键打开自定义菜单 */
function onRightClick(albumId: number, event: MouseEvent) {
  event.preventDefault(); // 阻止浏览器默认右键菜单
  event.stopPropagation(); // 阻止触发卡片点击跳转
  contextMenu.value = {
    visible: true,
    x: event.clientX,
    y: event.clientY,
    albumId,
  };
}

/** 关闭右键菜单 */
function closeContextMenu() {
  contextMenu.value.visible = false;
}

/** 点击菜单「删除」选项 */
/** 待确认删除的相册 ID（确认弹窗回调时使用，contextMenu 已关闭） */
const pendingDeleteId = ref<number | null>(null);
const contextDeleteConfirm = ref<{ visible: boolean; message: string }>({ visible: false, message: "" });
async function contextDelete() {
  const albumId = contextMenu.value.albumId;
  closeContextMenu();
  if (isDeletingOne.value) return;

  const album = store.albums.find((a) => a.id === albumId);
  pendingDeleteId.value = albumId;
  contextDeleteConfirm.value = {
    visible: true,
    message: `确定要删除相册「${album?.name ?? ""}」吗？\n\n此操作仅删除相册记录，不会删除本地照片文件。`,
  };
}
/** 确认后真正执行删除 */
async function doContextDelete() {
  contextDeleteConfirm.value.visible = false;
  const albumId = pendingDeleteId.value;
  pendingDeleteId.value = null;
  if (albumId == null) return;
  if (isDeletingOne.value) return;

  isDeletingOne.value = true;
  try {
    await store.deleteAlbum(albumId);
    // 若正处于勾选模式，从勾选集合移除
    if (selectedIds.value.has(albumId)) {
      const next = new Set(selectedIds.value);
      next.delete(albumId);
      selectedIds.value = next;
    }
  } catch (e) {
    alert(`删除失败：${e}`);
  } finally {
    isDeletingOne.value = false;
  }
}

/** 全局点击关闭右键菜单 */
function onGlobalClick() {
  closeContextMenu();
}

// ---------- 新建相册对话框状态 ----------
const showCreateDialog = ref(false);
const isCreating = ref(false);
const form = ref<{ name: string; path: string; description: string }>({
  name: "",
  path: "",
  description: "",
});
const errorMsg = ref("");

/** 打开系统文件夹选择对话框（需求 §6.1） */
async function chooseFolder() {
  errorMsg.value = "";
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择相册文件夹",
  });
  if (typeof selected === "string") {
    form.value.path = selected;
    // 若名称为空，自动用文件夹名填充（需求 §6.1 结果处理）
    if (!form.value.name.trim()) {
      const parts = selected.split(/[\\/]/);
      form.value.name = parts[parts.length - 1] || "";
    }
  }
}

function openCreateDialog() {
  form.value = { name: "", path: "", description: "" };
  errorMsg.value = "";
  showCreateDialog.value = true;
}

async function submitCreate() {
  errorMsg.value = "";
  // 前端校验（需求 §7.1）
  if (!form.value.name.trim()) {
    errorMsg.value = "请填写相册名称";
    return;
  }
  if (!form.value.path.trim()) {
    errorMsg.value = "请选择文件夹";
    return;
  }

  isCreating.value = true;
  try {
    const input: CreateAlbumInput = {
      name: form.value.name.trim(),
      path: form.value.path,
      description: form.value.description.trim() || null,
    };
    const album = await store.createAlbum(input);
    showCreateDialog.value = false;
    router.push(`/album/${album.id}`);
  } catch (e) {
    // 后端错误（如路径已被占用）直接展示
    errorMsg.value = String(e);
  } finally {
    isCreating.value = false;
  }
}

// ---------- 时间分组 / 显示视图状态 ----------

/** 排序方式：date=按日期，location=按地点，manual=手动排序（localStorage 持久化） */
const sortMode = ref<"date" | "location" | "manual">(
  (localStorage.getItem("album-sort-mode") as "date" | "location" | "manual") ?? "date",
);

/** 按地点排序的相册（手动 location 标签），无地点的排在最后 */
const locationSortedAlbums = computed(() => {
  return [...store.albums].sort((a, b) => {
    const la = a.location ?? "￿"; // 无地点排最后
    const lb = b.location ?? "￿";
    return la.localeCompare(lb);
  });
});

/** 切换排序方式并持久化到 localStorage */
function setSortMode(mode: "date" | "location" | "manual") {
  sortMode.value = mode;
  // 切换排序方式时清空折叠状态（各模式共用 collapsed，避免串扰）
  collapsed.value = new Set();
  localStorage.setItem("album-sort-mode", mode);
}

/** 手动排序组件实例引用（用于调用其全部折叠/展开） */
const manualSortRef = ref<InstanceType<typeof ManualSort> | null>(null);

/** 地点分组：按地点名分组（保持 A-Z 顺序，无地点排最后） */
const locationGroups = computed<Array<{ location: string | null; albums: Album[] }>>(() => {
  const groups: Array<{ location: string | null; albums: Album[] }> = [];
  const map = new Map<string | null, Album[]>();
  for (const a of locationSortedAlbums.value) {
    const loc = a.location ?? null;
    if (!map.has(loc)) map.set(loc, []);
    map.get(loc)!.push(a);
  }
  for (const [loc, albums] of map) {
    groups.push({ location: loc, albums });
  }
  return groups;
});

/** 折叠状态：key 为分组的唯一标识 */
const collapsed = ref<Set<string>>(new Set());

/** 时间分组的相册树 */
const groupedYears = computed<YearGroup[]>(() => groupByTime(store.albums));

/** 是否折叠某分组 */
function isCollapsed(key: string): boolean {
  return collapsed.value.has(key);
}

/** 切换折叠状态 */
function toggleCollapse(key: string) {
  const next = new Set(collapsed.value);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  collapsed.value = next;
}

/** 年份折叠 key */
function yearKey(year: number): string {
  return `y-${year}`;
}
/** 季节折叠 key */
function seasonKey(year: number, season: string): string {
  return `s-${year}-${season}`;
}
/** 月份折叠 key */
function monthKey(year: number, season: string, month: number): string {
  return `m-${year}-${season}-${month}`;
}
/** 地点折叠 key */
function locationKey(location: string | null): string {
  return `loc-${location ?? "__none__"}`;
}

/** 路线图点击：滚动到对应年分组并展开 */
function jumpToYear(year: number) {
  // 展开该年分组
  const key = yearKey(year);
  if (collapsed.value.has(key)) {
    const next = new Set(collapsed.value);
    next.delete(key);
    collapsed.value = next;
  }
  // 滚动到对应分组
  const el = document.getElementById(`year-${year}`);
  if (el) {
    el.scrollIntoView({ behavior: "smooth", block: "start" });
    // 高亮提示
    el.classList.add("year-highlight");
    setTimeout(() => el.classList.remove("year-highlight"), 1500);
  }
}

/** 展开 / 折叠全部（日期 / 地点模式） */
function toggleAllGroups() {
  // 收集当前模式下的所有分组 key
  const keys: string[] = [];
  if (sortMode.value === "location") {
    for (const g of locationGroups.value) {
      keys.push(locationKey(g.location));
    }
  } else {
    for (const yg of groupedYears.value) {
      keys.push(yearKey(yg.year));
      for (const sg of yg.seasons) {
        keys.push(seasonKey(yg.year, sg.season));
        for (const mg of sg.months) {
          keys.push(monthKey(yg.year, sg.season, mg.month));
        }
      }
    }
  }
  if (collapsed.value.size > 0) {
    collapsed.value = new Set(); // 全部展开
  } else {
    collapsed.value = new Set(keys); // 全部折叠
  }
}

/** 当前模式是否处于"全部折叠"状态（用于按钮文字） */
const allCollapsed = computed(() => {
  if (sortMode.value === "manual") {
    return manualSortRef.value?.isAllCollapsed ?? false;
  }
  return collapsed.value.size > 0;
});

/** 工具栏"全部折叠 / 展开"按钮：按当前模式分发 */
function onToggleAll() {
  if (sortMode.value === "manual") {
    manualSortRef.value?.toggleAll();
  } else {
    toggleAllGroups();
  }
}

// ---------- 搜索功能 ----------
/** 搜索结果 */
interface SearchHit {
  album: Album;
  folder_id: number | null;
  folder_path: string;
}
const searchKeyword = ref("");
const searchResults = ref<SearchHit[]>([]);
/** 全局照片内容搜索命中（群相册搜索：跨全部相册的照片内容） */
const contentHits = ref<ContentSearchHit[]>([]);
const isSearching = ref(false);
let searchTimer: ReturnType<typeof setTimeout> | null = null;

/** 搜索框输入（防抖）：同时搜相册 + 全局照片内容 */
function onSearchInput() {
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    const kw = searchKeyword.value.trim();
    if (!kw) {
      searchResults.value = [];
      contentHits.value = [];
      return;
    }
    isSearching.value = true;
    try {
      const [albums, contents] = await Promise.all([
        invoke<Array<{ album: Album; folder_id: number | null; folder_path: string }>>(
          "search_albums",
          { keyword: kw }
        ),
        contentStore.searchPhotoContent(kw), // album_id=null → 全局群相册内容搜索
      ]);
      searchResults.value = albums.map((r) => ({
        album: r.album,
        folder_id: r.folder_id,
        folder_path: r.folder_path,
      }));
      contentHits.value = contents;
    } catch {
      searchResults.value = [];
      contentHits.value = [];
    } finally {
      isSearching.value = false;
    }
  }, 300);
}

/** 是否处于搜索状态（有输入关键词） */
const isSearchingActive = computed(() => searchKeyword.value.trim().length > 0);

/** 清除搜索 */
function clearSearch() {
  searchKeyword.value = "";
  searchResults.value = [];
  contentHits.value = [];
}

/** 点击照片内容命中：跳转到所属相册（无相册则打开所在文件夹） */
async function gotoContentHit(hit: ContentSearchHit) {
  if (hit.album_id != null) {
    router.push(`/album/${hit.album_id}`);
    return;
  }
  if (hit.album_path) {
    try {
      await invoke("open_folder", { path: hit.album_path });
    } catch (e) {
      alert(`无法打开文件夹：${e}`);
    }
  }
}

/** 手动排序模式下待跳转的分组 id（搜索结果点击分组路径触发） */
const manualJumpFolderId = ref<number | null>(null);

/** 搜索结果点击分组路径：切换到手动模式并跳转到对应分组 */
function jumpToFolderInManual(folderId: number) {
  if (folderId == null) return;
  manualJumpFolderId.value = folderId;
  setSortMode("manual");
  // 切换到手动模式后，ManualSort 组件 watch 到 manualJumpFolderId 会滚动
}

// ---------- 列表加载 ----------
onMounted(() => {
  store.fetchAlbums();
  // 全局点击关闭右键菜单
  window.addEventListener("click", onGlobalClick);
  // 滚动监听（用于回到顶部按钮显示）
  window.addEventListener("scroll", onScroll, { passive: true });

  // 从路由 query 处理跳转（如从详情页跳转父相册）
  const q = route.query;
  if (q.sort === "manual") {
    const folderId = q.folder ? Number(q.folder) : null;
    if (folderId) {
      manualJumpFolderId.value = folderId;
    }
    setSortMode("manual");
  }
});
onBeforeUnmount(() => {
  window.removeEventListener("click", onGlobalClick);
  window.removeEventListener("scroll", onScroll);
});
</script>

<template>
  <div class="album-page">
    <!-- 顶部工具栏（需求 §5.3） -->
    <header class="toolbar">
      <div class="toolbar-left">
        <button class="btn btn-back" @click="router.push('/home')">← 主页</button>
        <h1 class="page-title">我的相册</h1>
      </div>
      <div class="toolbar-actions">
        <!-- 非勾选模式：排序控件 + 批量导入 / 管理 / 新建 -->
        <template v-if="!isSelectMode">
          <!-- 排序方式 -->
          <select class="select" :value="sortMode" @change="(e) => setSortMode((e.target as HTMLSelectElement).value as 'date' | 'location' | 'manual')">
            <option value="date">按日期</option>
            <option value="location">按地点</option>
            <option value="manual">手动排序</option>
          </select>
          <button class="btn" @click="onToggleAll">
            {{ allCollapsed ? "全部展开" : "全部折叠" }}
          </button>
          <button class="btn" :disabled="isImporting" @click="batchImport">
            {{ isImporting ? "导入中…" : "批量导入" }}
          </button>
          <button class="btn" @click="enterSelectMode">管理</button>
          <button class="btn btn-primary" @click="openCreateDialog">新建相册</button>
        </template>
        <!-- 勾选模式：删除 / 取消 -->
        <template v-else>
          <button
            class="btn btn-danger"
            :disabled="isDeleting || selectedIds.size === 0"
            @click="batchDelete"
          >
            {{ isDeleting ? "删除中…" : `删除所选 (${selectedIds.size})` }}
          </button>
          <button class="btn" @click="exitSelectMode">取消</button>
        </template>
      </div>
    </header>

    <!-- 搜索栏 -->
    <div class="search-bar">
      <div class="search-input-wrap">
        <span class="search-icon">🔍</span>
        <input
          v-model="searchKeyword"
          class="search-input"
          placeholder="搜索相册名称…"
          @input="onSearchInput"
        />
        <button v-if="searchKeyword" class="search-clear" @click="clearSearch">×</button>
      </div>
    </div>

    <!-- 搜索结果面板：相册 + 照片内容（全局智能搜索） -->
    <div v-if="isSearchingActive" class="search-results">
      <div v-if="isSearching" class="search-status">正在搜索…</div>
      <template v-else>
        <!-- 相册匹配 -->
        <div v-if="searchResults.length" class="search-section">
          <div class="search-section-title">相册</div>
          <div class="search-result-list">
            <div
              v-for="hit in searchResults"
              :key="'a' + hit.album.id"
              class="search-result-item"
              @click="router.push(`/album/${hit.album.id}`)"
            >
              <img v-if="hit.album.cover_path" :src="fileUrl(hit.album.cover_path)" class="result-cover" />
              <div v-else class="result-cover result-placeholder">📷</div>
              <div class="result-info">
                <span class="result-name">{{ hit.album.name }}</span>
                <div class="result-meta">
                  <span v-if="hit.folder_id != null" class="result-path">{{ hit.folder_path || "分组" }}</span>
                  <span v-else class="result-path">未分组</span>
                  <button
                    v-if="hit.folder_id != null"
                    class="result-jump"
                    title="跳转到该分组"
                    @click.stop="jumpToFolderInManual(hit.folder_id)"
                  >📁 跳转分组</button>
                </div>
              </div>
              <span class="result-arrow">→</span>
            </div>
          </div>
        </div>
        <!-- 照片内容匹配（智能搜索，跨全部相册） -->
        <div v-if="contentHits.length" class="search-section">
          <div class="search-section-title">照片内容（{{ contentHits.length }}）</div>
          <div class="search-result-list">
            <div
              v-for="hit in contentHits"
              :key="'c' + hit.id"
              class="search-result-item"
              title="点击跳转到所属相册"
              @click="gotoContentHit(hit)"
            >
              <div class="result-cover result-placeholder">🖼</div>
              <div class="result-info">
                <span class="result-name">{{ hit.label || hit.category || "照片" }}</span>
                <div class="result-meta">
                  <span class="result-path">{{ hit.album_name || hit.parent_dir }}</span>
                  <span v-if="hit.location" class="result-path">{{ hit.location }}</span>
                </div>
                <div class="result-meta" v-if="hit.person_ids.length || hit.shoot_time">
                  <span class="result-path">{{ [hit.person_ids.join(" "), hit.shoot_time].filter(Boolean).join(" · ") }}</span>
                </div>
              </div>
              <span class="result-arrow">→</span>
            </div>
          </div>
        </div>
        <div v-if="!searchResults.length && !contentHits.length" class="search-status">
          未找到匹配的相册或照片内容
        </div>
      </template>
    </div>

    <!-- 日期模式：年 → 季节 → 月 路线图（点击年份可跳转） -->
    <div v-if="!isSelectMode && sortMode === 'date'" class="view-bar">
      <span class="view-bar-label">时间路线图</span>
      <span v-if="groupedYears.length === 0" class="bc-empty">暂无相册</span>
      <template v-for="yg in groupedYears" :key="yg.year">
        <span class="bc-sep">›</span>
        <span
          class="bc-item bc-link"
          :title="`跳转到 ${yg.year === 0 ? '未分类' : yg.year + '年'}`"
          @click="jumpToYear(yg.year)"
        >{{ yg.year === 0 ? "未分类" : `${yg.year}年` }}</span>
      </template>
    </div>

    <!-- 地点模式：提示栏 -->
    <div v-if="!isSelectMode && sortMode === 'location'" class="view-bar">
      <span class="view-bar-label">按地点排序</span>
      <span v-if="store.albums.length === 0" class="bc-empty">暂无相册</span>
      <span v-else class="bc-item">{{ store.albums.filter((a) => a.location).length }} 个相册有地点</span>
    </div>

    <!-- 批量导入进度条 -->
    <div v-if="isImporting" class="import-progress-bar">
      <div class="import-track">
        <div class="import-fill" :style="{ width: `${importProgress}%` }"></div>
      </div>
      <p class="import-status">{{ importStatus }}</p>
    </div>

    <!-- 批量选择提示栏（勾选模式下且有勾选时显示） -->
    <div v-if="isSelectMode && selectedIds.size > 0" class="select-bar">
      <span>
        已选 {{ selectedIds.size }} 个相册
        <button class="link-btn" @click="toggleSelectAll">
          {{ allSelected ? "取消全选" : "全选" }}
        </button>
      </span>
      <span class="select-hint">勾选后删除仅移除相册记录，不影响本地照片</span>
    </div>

    <!-- 日期模式：时间分组视图（年 → 季节 → 月，可折叠） -->
    <main v-if="sortMode === 'date'" class="timeline-view">
      <template v-for="yg in groupedYears" :key="yg.year">
        <!-- 年分组 -->
        <section
          :id="`year-${yg.year}`"
          class="year-group"
          :class="{ collapsed: isCollapsed(yearKey(yg.year)) }"
        >
          <h2 class="group-head group-year" @click="toggleCollapse(yearKey(yg.year))">
            <span class="fold-arrow">{{ isCollapsed(yearKey(yg.year)) ? "▸" : "▾" }}</span>
            <span class="group-title">{{ yg.year === 0 ? "未分类" : `${yg.year} 年` }}</span>
            <span class="group-count">
              {{ yg.seasons.reduce((n, s) => n + s.months.reduce((m, mo) => m + mo.albums.length, 0), 0) + yg.uncategorized.length }} 个相册
            </span>
          </h2>
          <div v-show="!isCollapsed(yearKey(yg.year))" class="group-body">
            <!-- 未分类相册 -->
            <div v-if="yg.uncategorized.length > 0" class="season-group">
              <h3 class="group-head group-season" @click="toggleCollapse(`${yearKey(yg.year)}-uc`)">
                <span class="fold-arrow">{{ isCollapsed(`${yearKey(yg.year)}-uc`) ? "▸" : "▾" }}</span>
                <span class="group-title">未分类</span>
              </h3>
              <div v-show="!isCollapsed(`${yearKey(yg.year)}-uc`)" class="group-body">
                <div class="album-grid">
                  <AlbumCard
                    v-for="album in yg.uncategorized"
                    :key="album.id"
                    :album="album"
                    :select-mode="isSelectMode"
                    :selected="selectedIds.has(album.id)"
                    @click="isSelectMode ? toggleSelect(album.id) : router.push(`/album/${album.id}`)"
                    @contextmenu="onRightClick(album.id, $event)"
                    @toggle-select="toggleSelect(album.id, $event)"
                    @open-path="openAlbumPath(album.path, $event)"
                  />
                </div>
              </div>
            </div>

            <!-- 季节分组 -->
            <section
              v-for="sg in yg.seasons"
              :key="seasonKey(yg.year, sg.season)"
              class="season-group"
              :class="{ collapsed: isCollapsed(seasonKey(yg.year, sg.season)) }"
            >
              <h3 class="group-head group-season" @click="toggleCollapse(seasonKey(yg.year, sg.season))">
                <span class="fold-arrow">{{ isCollapsed(seasonKey(yg.year, sg.season)) ? "▸" : "▾" }}</span>
                <span class="group-title">{{ seasonName(sg.season) }}</span>
                <span class="group-bc">{{ yg.year }} 年 › {{ seasonName(sg.season) }}</span>
                <span class="group-count">{{ sg.months.reduce((n, m) => n + m.albums.length, 0) }} 个相册</span>
              </h3>
              <div v-show="!isCollapsed(seasonKey(yg.year, sg.season))" class="group-body">
                <!-- 月分组 -->
                <section
                  v-for="mg in sg.months"
                  :key="monthKey(yg.year, sg.season, mg.month)"
                  class="month-group"
                  :class="{ collapsed: isCollapsed(monthKey(yg.year, sg.season, mg.month)) }"
                >
                  <h4 class="group-head group-month" @click="toggleCollapse(monthKey(yg.year, sg.season, mg.month))">
                    <span class="fold-arrow">{{ isCollapsed(monthKey(yg.year, sg.season, mg.month)) ? "▸" : "▾" }}</span>
                    <span class="group-title">{{ MONTH_NAMES[mg.month - 1] }}</span>
                    <span class="group-bc">{{ yg.year }} 年 › {{ seasonName(sg.season) }} › {{ MONTH_NAMES[mg.month - 1] }}</span>
                    <span class="group-count">{{ mg.albums.length }} 个相册</span>
                  </h4>
                  <div v-show="!isCollapsed(monthKey(yg.year, sg.season, mg.month))" class="group-body">
                    <div class="album-grid">
                      <AlbumCard
                        v-for="album in mg.albums"
                        :key="album.id"
                        :album="album"
                        :select-mode="isSelectMode"
                        :selected="selectedIds.has(album.id)"
                        @click="isSelectMode ? toggleSelect(album.id) : router.push(`/album/${album.id}`)"
                        @contextmenu="onRightClick(album.id, $event)"
                        @toggle-select="toggleSelect(album.id, $event)"
                        @open-path="openAlbumPath(album.path, $event)"
                      />
                    </div>
                  </div>
                </section>
              </div>
            </section>
          </div>
        </section>
      </template>
    </main>

    <!-- 地点模式：按地点分组（A-Z 顺序，可折叠） -->
    <main v-else-if="sortMode === 'location'" class="location-view">
      <template v-for="g in locationGroups" :key="g.location ?? '__none__'">
        <section class="location-group" :class="{ collapsed: isCollapsed(locationKey(g.location)) }">
          <h3 class="group-head group-location" @click="toggleCollapse(locationKey(g.location))">
            <span class="fold-arrow">{{ isCollapsed(locationKey(g.location)) ? "▸" : "▾" }}</span>
            <span class="group-title">📍 {{ g.location || "未知地点" }}</span>
            <span class="group-count">{{ g.albums.length }} 个相册</span>
          </h3>
          <div v-show="!isCollapsed(locationKey(g.location))" class="group-body">
            <div class="album-grid">
              <div v-for="album in g.albums" :key="album.id" class="location-card-wrap">
                <AlbumCard
                  :album="album"
                  :select-mode="isSelectMode"
                  :selected="selectedIds.has(album.id)"
                  show-location
                  @click="isSelectMode ? toggleSelect(album.id) : router.push(`/album/${album.id}`)"
                  @contextmenu="onRightClick(album.id, $event)"
                  @toggle-select="toggleSelect(album.id, $event)"
                  @open-path="openAlbumPath(album.path, $event)"
                />
              </div>
            </div>
          </div>
        </section>
      </template>
    </main>

    <!-- 手动排序模式：文件夹树 + 拖拽 -->
    <main v-else class="manual-view">
      <ManualSort
        ref="manualSortRef"
        :jump-folder-id="manualJumpFolderId"
        :select-mode="isSelectMode"
        :selected-ids="selectedIds"
        @toggle-select="toggleSelect"
      />
    </main>

    <!-- 空状态引导（需求 §2.1，手动模式不显示） -->
    <div v-if="!store.isLoading && store.albums.length === 0 && sortMode !== 'manual'" class="empty-state">
      <div class="empty-icon">📁</div>
      <p>还没有相册</p>
      <button class="btn btn-primary" @click="openCreateDialog">创建第一个相册</button>
    </div>

    <!-- 右键菜单 -->
    <div
      v-if="contextMenu.visible"
      class="context-menu"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @click.stop
    >
      <div class="context-menu-item" @click="router.push(`/album/${contextMenu.albumId}`)">
        <span class="ctx-icon">📂</span> 打开
      </div>
      <div class="context-menu-item" @click="openRenameDialog">
        <span class="ctx-icon">✏️</span> 重命名
      </div>
      <div class="context-menu-item context-menu-danger" @click="contextDelete">
        <span class="ctx-icon">🗑️</span> 删除
      </div>
    </div>

    <!-- 新建相册对话框（模态） -->
    <div v-if="showCreateDialog" class="dialog-mask" @click.self="showCreateDialog = false">
      <div class="dialog">
        <h2 class="dialog-title">新建相册</h2>

        <!-- 文件夹选择 -->
        <div class="form-field">
          <label class="form-label">相册文件夹</label>
          <div class="path-row">
            <input v-model="form.path" class="input" placeholder="选择相册绑定的文件夹" readonly />
            <button class="btn" @click="chooseFolder">选择</button>
          </div>
        </div>

        <!-- 相册名称 -->
        <div class="form-field">
          <label class="form-label">相册名称</label>
          <input
            v-model="form.name"
            class="input"
            placeholder="请输入相册名称"
            maxlength="100"
          />
        </div>

        <!-- 相册说明（需求：添加说明模块） -->
        <div class="form-field">
          <label class="form-label">相册说明</label>
          <textarea
            v-model="form.description"
            class="textarea"
            placeholder="选填，介绍一下这个相册的内容…"
            maxlength="500"
          ></textarea>
          <span class="char-count">{{ form.description.length }}/500</span>
        </div>

        <!-- 错误提示 -->
        <p v-if="errorMsg" class="error-msg">{{ errorMsg }}</p>

        <!-- 操作按钮 -->
        <div class="dialog-actions">
          <button class="btn" @click="showCreateDialog = false">取消</button>
          <button class="btn btn-primary" :disabled="isCreating" @click="submitCreate">
            {{ isCreating ? "创建中…" : "创建" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 重命名相册对话框（右键菜单） -->
    <div v-if="showRenameDialog" class="dialog-mask" @click.self="showRenameDialog = false">
      <div class="dialog">
        <h2 class="dialog-title">重命名相册</h2>
        <div class="form-field">
          <label class="form-label">相册名称</label>
          <input
            v-model="renameInput"
            class="input"
            maxlength="100"
            placeholder="相册名称"
            @keydown.enter="submitRename"
            @keydown.esc="showRenameDialog = false"
          />
        </div>
        <div class="dialog-actions">
          <button class="btn" @click="showRenameDialog = false">取消</button>
          <button class="btn btn-primary" :disabled="isRenaming" @click="submitRename">
            {{ isRenaming ? "保存中…" : "保存" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 删除相册二次确认（右键菜单） -->
    <ConfirmDialog
      :visible="contextDeleteConfirm.visible"
      title="删除相册"
      :message="contextDeleteConfirm.message"
      @confirm="doContextDelete"
      @cancel="contextDeleteConfirm.visible = false"
    />

    <!-- 批量删除二次确认 -->
    <ConfirmDialog
      :visible="batchDeleteConfirm.visible"
      title="批量删除相册"
      :message="batchDeleteConfirm.message"
      confirm-text="删除所选"
      @confirm="doBatchDelete"
      @cancel="batchDeleteConfirm.visible = false"
    />

    <!-- 回到顶部按钮 -->
    <button
      v-show="showBackToTop"
      class="back-to-top"
      @click="scrollToTop"
      title="回到顶部"
    >
      ↑
    </button>
  </div>
</template>

<style scoped>
.album-page {
  max-width: 1200px;
  margin: 0 auto;
  padding: 24px;
  min-height: 100vh;
}

.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 24px;
}

.toolbar-left {
  display: flex;
  align-items: center;
  gap: 16px;
}

.toolbar-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.select {
  padding: 8px 12px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 14px;
  background: #fff;
  outline: none;
  cursor: pointer;
}

.select:focus {
  border-color: #396cd8;
}

.btn-icon {
  padding: 8px 12px;
}

/* 时间路线图提示栏 */
.view-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  background: #f7f9fc;
  border: 1px solid #eef0f4;
  border-radius: 8px;
  padding: 8px 16px;
  margin-bottom: 20px;
  font-size: 13px;
  color: #555;
  overflow-x: auto;
  white-space: nowrap;
}

.view-bar-label {
  color: #396cd8;
  font-weight: 600;
  margin-right: 4px;
}

.bc-sep {
  color: #bbb;
}

.bc-item {
  color: #396cd8;
}

.bc-link {
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  transition: background 0.2s, color 0.2s;
}

.bc-link:hover {
  background: #396cd8;
  color: #fff;
}

/* 搜索栏 */
.search-bar {
  margin-bottom: 16px;
}

.search-input-wrap {
  position: relative;
  display: flex;
  align-items: center;
}

.search-icon {
  position: absolute;
  left: 12px;
  font-size: 14px;
  color: #999;
  pointer-events: none;
}

.search-input {
  width: 100%;
  padding: 10px 36px 10px 36px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  box-sizing: border-box;
  transition: border-color 0.2s;
}

.search-input:focus {
  border-color: #396cd8;
}

.search-clear {
  position: absolute;
  right: 10px;
  border: none;
  background: none;
  color: #999;
  font-size: 18px;
  cursor: pointer;
  line-height: 1;
}

.search-clear:hover {
  color: #333;
}

/* 搜索结果面板 */
.search-results {
  background: #fff;
  border: 1px solid #eef0f4;
  border-radius: 10px;
  padding: 12px;
  margin-bottom: 16px;
  max-height: 400px;
  overflow-y: auto;
}

.search-status {
  color: #999;
  font-size: 14px;
  text-align: center;
  padding: 20px 0;
}

/* 搜索结果分组（相册 / 照片内容） */
.search-section + .search-section {
  margin-top: 6px;
  border-top: 1px solid #eef0f4;
  padding-top: 8px;
}

.search-section-title {
  font-size: 12px;
  color: #999;
  padding: 2px 10px 6px;
  letter-spacing: 0.5px;
}

.search-result-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.2s;
}

.search-result-item:hover {
  background: #f0f5ff;
}

.result-cover {
  width: 48px;
  height: 36px;
  object-fit: cover;
  border-radius: 6px;
  flex-shrink: 0;
}

.result-placeholder {
  background: #f0f0f0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
}

.result-info {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.result-name {
  font-size: 14px;
  color: #2c3e50;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.result-path {
  font-size: 12px;
  color: #888;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.result-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.result-jump {
  flex-shrink: 0;
  border: 1px solid #396cd8;
  background: #eef3ff;
  color: #396cd8;
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 10px;
  cursor: pointer;
  transition: all 0.2s;
}

.result-jump:hover {
  background: #396cd8;
  color: #fff;
}

.result-arrow {
  color: #999;
  flex-shrink: 0;
}

.btn-back {
  border: none;
  background: transparent;
  color: #396cd8;
  padding: 4px 8px;
  font-size: 14px;
}

.btn-back:hover {
  background: rgba(57, 108, 216, 0.08);
  border-color: transparent;
  color: #396cd8;
}

.page-title {
  font-size: 24px;
  margin: 0;
}

/* 时间分组视图 */
.timeline-view {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.year-group {
  border: 1px solid #eef0f4;
  border-radius: 12px;
  overflow: hidden;
  background: #fff;
}

/* 路线图跳转后的高亮动画 */
.year-highlight {
  animation: yearFlash 1.5s ease;
}

@keyframes yearFlash {
  0% {
    box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.4);
    background: #f0f5ff;
  }
  100% {
    box-shadow: none;
    background: #fff;
  }
}

.group-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  margin: 0;
  cursor: pointer;
  user-select: none;
  transition: background 0.2s;
}

.group-head:hover {
  background: #f5f8ff;
}

.group-year {
  background: #eef3ff;
  border-bottom: 1px solid #e0e9fa;
}

.group-season {
  background: #f7f9fc;
  border-bottom: 1px solid #f0f0f0;
}

.group-month {
  background: #fafbfd;
  border-bottom: 1px solid #f5f5f5;
  padding: 8px 16px 8px 32px;
}

.group-title {
  font-weight: 600;
  font-size: 15px;
  color: #2c3e50;
}

.group-count {
  font-size: 12px;
  color: #999;
  font-weight: normal;
}

.group-bc {
  font-size: 12px;
  color: #396cd8;
  background: rgba(57, 108, 216, 0.08);
  border-radius: 4px;
  padding: 1px 8px;
}

.fold-arrow {
  color: #999;
  font-size: 13px;
  width: 14px;
  text-align: center;
}

.group-body {
  padding: 12px 16px;
}

.month-group .group-body {
  padding: 12px 16px 12px 48px;
}

.collapsed .group-body {
  display: none;
}

.album-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 20px;
}

.empty-state {
  text-align: center;
  padding: 80px 0;
  color: #888;
}

.empty-icon {
  font-size: 56px;
  margin-bottom: 12px;
}

/* 右键菜单 */
.context-menu {
  position: fixed;
  z-index: 200;
  min-width: 140px;
  background: #fff;
  border: 1px solid #eee;
  border-radius: 8px;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.15);
  padding: 6px;
}

.context-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 14px;
  color: #2c3e50;
  cursor: pointer;
  transition: background 0.15s;
}

.context-menu-item:hover {
  background: #f0f4ff;
}

.context-menu-danger {
  color: #e5484d;
}

.context-menu-danger:hover {
  background: #fdf0f0;
}

.ctx-icon {
  font-size: 14px;
}

/* 通用按钮 */
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid #ddd;
  background: #fff;
  cursor: pointer;
  font-size: 14px;
  transition: all 0.2s;
}

.btn:hover {
  border-color: #396cd8;
  color: #396cd8;
}

.btn-primary {
  background: #396cd8;
  color: #fff;
  border-color: #396cd8;
}

.btn-primary:hover {
  background: #2f5cc2;
  color: #fff;
}

.btn:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.btn-danger {
  background: #e5484d;
  color: #fff;
  border-color: #e5484d;
}

.btn-danger:hover {
  background: #d13438;
  color: #fff;
}

/* 批量导入进度条 */
.import-progress-bar {
  margin-bottom: 16px;
}

.import-track {
  width: 100%;
  height: 8px;
  background: #eee;
  border-radius: 4px;
  overflow: hidden;
}

.import-fill {
  height: 100%;
  background: #396cd8;
  border-radius: 4px;
  transition: width 0.3s ease;
}

.import-status {
  margin: 6px 0 0;
  font-size: 13px;
  color: #666;
}

/* 批量选择提示栏 */
.select-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  background: #eef3ff;
  border: 1px solid #d0dcf8;
  border-radius: 8px;
  padding: 8px 16px;
  margin-bottom: 16px;
  font-size: 14px;
  color: #2c3e50;
}

.select-hint {
  font-size: 12px;
  color: #888;
}

.link-btn {
  border: none;
  background: none;
  color: #396cd8;
  cursor: pointer;
  font-size: 14px;
  padding: 0 4px;
}

.link-btn:hover {
  text-decoration: underline;
}

/* 地点模式 */
.location-view {
  padding-bottom: 20px;
}

.location-group {
  border: 1px solid #eef0f4;
  border-radius: 12px;
  overflow: hidden;
  background: #fff;
  margin-bottom: 12px;
}

.group-location {
  background: #eef3ff;
  border-bottom: 1px solid #e0e9fa;
}

.location-card-wrap {
  break-inside: avoid;
}

/* 对话框 */
.dialog-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.dialog {
  width: 520px;
  max-width: 90vw;
  background: #fff;
  border-radius: 12px;
  padding: 24px;
  box-shadow: 0 12px 32px rgba(0, 0, 0, 0.2);
}

.dialog-title {
  margin: 0 0 20px;
  font-size: 20px;
}

.form-field {
  margin-bottom: 16px;
}

.form-label {
  display: block;
  margin-bottom: 6px;
  font-size: 14px;
  color: #333;
}

.path-row {
  display: flex;
  gap: 8px;
}

.input {
  flex: 1;
  padding: 8px 12px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 14px;
  outline: none;
}

.input:focus {
  border-color: #396cd8;
}

.textarea {
  width: 100%;
  min-height: 90px;
  padding: 8px 12px;
  border: 1px solid #ddd;
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  resize: vertical;
  box-sizing: border-box;
}

.char-count {
  display: block;
  text-align: right;
  font-size: 12px;
  color: #999;
  margin-top: 4px;
}

.error-msg {
  color: #e5484d;
  font-size: 14px;
  margin: 0 0 12px;
}

.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

/* 回到顶部按钮 */
.back-to-top {
  position: fixed;
  right: 24px;
  bottom: 24px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  border: 1px solid #ddd;
  background: #fff;
  box-shadow: 0 2px 12px rgba(0, 0, 0, 0.12);
  font-size: 20px;
  color: #396cd8;
  cursor: pointer;
  transition: opacity 0.3s ease, transform 0.3s ease, background 0.2s;
  z-index: 150;
  display: flex;
  align-items: center;
  justify-content: center;
  user-select: none;
}

.back-to-top:hover {
  background: #eef3ff;
  transform: scale(1.08);
}

.back-to-top:active {
  transform: scale(0.95);
}
</style>
