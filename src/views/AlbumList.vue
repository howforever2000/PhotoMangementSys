<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import { useThemeStore } from "../stores/theme";
import type { Album, CreateAlbumInput } from "../types/album";
import type { Folder, ManualTree } from "../types/folder";
import type { ContentSearchHit } from "../types/content";
import { groupByTime, seasonName, MONTH_NAMES } from "../utils/timeGroup";
import type { YearGroup } from "../utils/timeGroup";
import ManualSort from "./ManualSort.vue";
import AlbumCard from "../components/AlbumCard.vue";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import { useNotify } from "../composables/useNotify";

const router = useRouter();
const route = useRoute();
const store = useAlbumStore();
const contentStore = useContentStore();
const theme = useThemeStore();
const notify = useNotify();

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
    notify.error("无法打开文件夹", `${path}\n${e}`);
  }
}

/** FEAT-A：合并来源路径点击 —— 直接调 open_folder，事件已由子组件 stopPropagation */
async function openSourcePath(payload: { id: number; path: string }) {
  try {
    await invoke("open_folder", { path: payload.path });
  } catch (e) {
    notify.error("无法打开源文件夹", `${payload.path}\n${e}`);
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
    notify.success("批量导入完成", parts.join("，"));
  } catch (e) {
    notify.error("批量导入失败", String(e));
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

/** 是否全部选中（当前可见的） */
const allSelected = computed(
  () => currentVisibleAlbumIds.value.length > 0 && selectedIds.value.size === currentVisibleAlbumIds.value.length,
);

/** 合并按钮的动态 title：解释「为什么禁用」 */
const mergeTitle = computed(() => {
  if (selectedIds.value.size === 0) return "请先勾选至少 1 个相册（合并是 N → 1）";
  if (store.albums.length - selectedIds.value.size === 0)
    return "需要至少 1 个未被勾选的目标相册（合并目标不能是源）";
  if (selectedIds.value.size === 1)
    return `将所选 1 个相册合并到目标相册（单个也支持，仅「仅删记录」模式生效；物理移动到自身路径会被跳过）`;
  return `把 ${selectedIds.value.size} 个源相册合并到目标相册（可选「物理移动文件」或「仅删记录」）`;
});

/** 进入勾选管理模式（默认全不选） */
function enterSelectMode() {
  if (!store.albums.length) return;
  isSelectMode.value = true;
  // 首次进入显示教学 toast：告知快捷键 + 勾选说明（一次）
  if (!sessionStorage.getItem("pm-album-manage-hint")) {
    sessionStorage.setItem("pm-album-manage-hint", "1");
    notify.info(
      "已进入批量管理",
      "点击卡片勾选 · Ctrl+A 全选当前可见 · Esc 退出 · Del 删除（仅除记录）",
      5000,
    );
  }
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
  // 仅在选模式下操作
  if (allSelected.value) {
    selectedIds.value = new Set();
    return;
  }
  // 全选当前模式下的可见相册（折叠中的不计入）
  const visible = currentVisibleAlbumIds.value;
  selectedIds.value = new Set(visible);
  // 提示用户：折叠中的不会勾选
  const total = store.albums.length;
  if (visible.length < total) {
    notify.info(
      "已选当前可见",
      `勾选 ${visible.length} / ${total} 个相册，折叠中的不会自动勾选。`,
      3500,
    );
  }
}

/** 当前模式（日期 / 地点 / 手动）下，展开中的相册 id 集合。
 *  用于全选只勾选可见 —避免误全选折叠区域。 */
const currentVisibleAlbumIds = computed<number[]>(() => {
  if (sortMode.value === "manual") {
    // 手动模式：所有相册都视为可见（拖拽时也要能全选）
    return store.albums.map((a) => a.id);
  }
  if (sortMode.value === "location") {
    const ids: number[] = [];
    for (const g of locationGroups.value) {
      if (!isCollapsed(locationKey(g.location))) {
        for (const a of g.albums) ids.push(a.id);
      }
    }
    return ids;
  }
  // date mode
  const ids: number[] = [];
  for (const yg of groupedYears.value) {
    if (isCollapsed(yearKey(yg.year))) continue;
    for (const a of yg.uncategorized) ids.push(a.id);
    for (const sg of yg.seasons) {
      if (isCollapsed(seasonKey(yg.year, sg.season))) continue;
      for (const mg of sg.months) {
        if (isCollapsed(monthKey(yg.year, sg.season, mg.month))) continue;
        for (const a of mg.albums) ids.push(a.id);
      }
    }
  }
  return ids;
});

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
    notify.success("批量删除完成", `已删除 ${deleted} 个相册`);
    exitSelectMode();
  } catch (e) {
    notify.error("删除失败", String(e));
  } finally {
    isDeleting.value = false;
  }
}

// ---------- 批量整理：移动分组 / 打标签 / 改地点 / 合并相册 ----------
const batchRunning = ref(false);

/* 移动分组 */
const moveFolderOpen = ref(false);
const moveFolders = ref<Folder[]>([]);
const moveTarget = ref<number | null>(null);

/** 选中相册当前所在分组（用于提示） */
const moveSelectedFolders = computed(() => {
  const ids = [...selectedIds.value];
  const set = new Set<number | null>();
  const names: string[] = [];
  for (const id of ids) {
    const album = store.albums.find((a) => a.id === id);
    if (!album) continue;
    const fid = album.folder_id ?? null;
    if (!set.has(fid)) {
      set.add(fid);
      const f = moveFolders.value.find((x) => x.id === fid);
      names.push(f ? f.name : "未分组");
    }
  }
  return names;
});
async function openMoveFolder() {
  try {
    const tree = await invoke<ManualTree>("get_manual_tree");
    moveFolders.value = tree.folders;
    moveTarget.value = null;
    moveFolderOpen.value = true;
  } catch (e) {
    notify.error("加载分组失败", String(e));
  }
}
async function doMoveFolder() {
  const ids = [...selectedIds.value];
  if (!ids.length || batchRunning.value) return;
  batchRunning.value = true;
  moveFolderOpen.value = false;
  try {
    const r = await store.batchMoveAlbumToFolder(ids, moveTarget.value);
    const parts = [`成功 ${r.ok} / ${r.requested} 个相册`];
    if (r.failed) parts.push(`失败 ${r.failed}`);
    notify.success("移动分组完成", parts.join("；"));
    await store.fetchAlbums();
  } catch (e) {
    notify.error("移动分组失败", String(e));
  } finally {
    batchRunning.value = false;
  }
}

/* 打标签 */
const tagDialogOpen = ref(false);
const tagInput = ref("");
const tagMode = ref<"add" | "remove">("add");
function openTagDialog(mode: "add" | "remove") {
  tagMode.value = mode;
  tagInput.value = "";
  tagDialogOpen.value = true;
}
async function doTagDialog() {
  const ids = [...selectedIds.value];
  const tag = tagInput.value.trim();
  if (!tag || !ids.length || batchRunning.value) return;
  batchRunning.value = true;
  tagDialogOpen.value = false;
  try {
    const r = await store.batchSetAlbumTag(ids, [tag], tagMode.value);
    const parts = [`成功 ${r.ok} / ${r.requested} 个相册`];
    if (r.failed) parts.push(`失败 ${r.failed}`);
    notify.success(tagMode.value === "add" ? "批量打标签完成" : "批量移除标签完成", parts.join("；"));
    await store.fetchAlbums();
  } catch (e) {
    notify.error("批量打标签失败", String(e));
  } finally {
    batchRunning.value = false;
  }
}

/* 改地点 */
const locDialogOpen = ref(false);
const locInput = ref("");

/** 选中相册当前地点（用于覆盖警告） */
const locCurrent = computed(() => {
  const ids = [...selectedIds.value];
  const set = new Set<string>();
  let unlocated = 0;
  for (const id of ids) {
    const a = store.albums.find((x) => x.id === id);
    if (!a) continue;
    if (a.location) set.add(a.location);
    else unlocated++;
  }
  const list = [...set];
  if (unlocated) list.push(`未设置 ×${unlocated}`);
  return list;
});
/** 覆盖警告：若选中相册中已有同名地点，输入框新值会覆盖 */
const locWillOverwrite = computed(() => {
  const v = locInput.value.trim();
  if (!v) return false;
  return locCurrent.value.some((c) => c === v);
});
function openLocDialog() {
  locInput.value = "";
  locDialogOpen.value = true;
}
async function doLocDialog() {
  const ids = [...selectedIds.value];
  if (!ids.length || batchRunning.value) return;
  batchRunning.value = true;
  locDialogOpen.value = false;
  try {
    const r = await store.batchSetAlbumLocation(ids, locInput.value.trim());
    const parts = [`成功 ${r.ok} / ${r.requested} 个相册`];
    if (r.failed) parts.push(`失败 ${r.failed}`);
    notify.success("批量改地点完成", parts.join("；"));
    await store.fetchAlbums();
  } catch (e) {
    notify.error("批量改地点失败", String(e));
  } finally {
    batchRunning.value = false;
  }
}

/* 合并相册：先选目标相册 + 合并模式，再二次确认 */
const mergeOpen = ref(false);
const mergeTarget = ref<number | null>(null);
const mergeMode = ref<"move" | "record">("move");
const mergeConfirm = ref<{ visible: boolean; message: string }>({ visible: false, message: "" });
const mergeTargets = computed(() => store.albums.filter((a) => !selectedIds.value.has(a.id)));
function openMerge() {
  mergeTarget.value = null;
  // 单选场景默认走“仅删记录”（物理移动对于单选仅在“合并到其他路径”时有意义；为防误判默认 record）
  mergeMode.value = selectedIds.value.size === 1 ? "record" : "move";
  mergeOpen.value = true;
}
async function requestMerge() {
  const sourceIds = [...selectedIds.value];
  const targetId = mergeTarget.value;
  if (!targetId || !sourceIds.length) return;
  const target = store.albums.find((a) => a.id === targetId);
  const targetName = target?.name ?? "";
  mergeOpen.value = false;
  // 单选场景特殊提示：物理移动模式下若目标路径与源路径不同时才会真正移动文件；
  // 仅“仅删记录”模式下仅删除源记录，文件保留原地。
  const n = sourceIds.length;
  const moveHint = n === 1
    ? `将「物理移动」所选 1 个源相册「${store.albums.find((a) => a.id === sourceIds[0])?.name ?? ""}」内的照片到「${targetName}」相册，\n合并后源相册记录与文件会被移除；该操作不可恢复，确定继续吗？`
    : `将把所选 ${n} 个源相册的照片「物理移动」到「${targetName}」相册，\n合并后源相册记录与文件会被移除；该操作不可恢复，确定继续吗？`;
  const recordHint = n === 1
    ? `将「仅删除」所选 1 个源相册的记录，照片文件保留在磁盘原处，\n不会移动任何文件；该操作不可恢复，确定继续吗？`
    : `将「仅删除」所选 ${n} 个源相册的记录，照片文件保留在磁盘原处，\n不会移动任何文件；该操作不可恢复，确定继续吗？`;
  mergeConfirm.value = {
    visible: true,
    message: mergeMode.value === "move" ? moveHint : recordHint,
  };
}
async function doMerge() {
  const sourceIds = [...selectedIds.value];
  const targetId = mergeTarget.value;
  if (!targetId || batchRunning.value) return;
  batchRunning.value = true;
  mergeConfirm.value.visible = false;
  try {
    const r = await store.mergeAlbums(sourceIds, targetId, mergeMode.value);
    const parts =
      mergeMode.value === "move"
        ? [`已合并 ${r.merged} 个源相册，移动 ${r.files_moved} 张照片`]
        : [`已合并 ${r.merged} 个源相册（仅删除记录，文件保留）`];
    if (r.files_failed) parts.push(`${r.files_failed} 张移动失败`);
    notify.success("合并相册完成", parts.join("；"));
    exitSelectMode();
    await store.fetchAlbums();
  } catch (e) {
    notify.error("合并相册失败", String(e));
  } finally {
    batchRunning.value = false;
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
    notify.warning("相册名称不能为空");
    return;
  }
  if (name.length > 100) {
    notify.warning("相册名称不能超过 100 个字符");
    return;
  }
  if (isRenaming.value) return;
  isRenaming.value = true;
  try {
    await store.renameAlbum(id, name, true);
    showRenameDialog.value = false;
    notify.success("重命名成功");
  } catch (e) {
    notify.error("重命名失败", String(e));
  } finally {
    isRenaming.value = false;
  }
}

/** 右键打开自定义菜单 */
function onRightClick(albumId: number, event: MouseEvent) {
  event.preventDefault(); // 阻止浏览器默认右键菜单
  event.stopPropagation(); // 阻止触发卡片点击跳转
  // 选模式下禁用右键菜单（避免和勾选交互冲突）
  if (isSelectMode.value) return;
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
    notify.error("删除失败", String(e));
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
      notify.error("无法打开文件夹", String(e));
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
  // 选模式快捷键
  window.addEventListener("keydown", onKey);

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
  window.removeEventListener("keydown", onKey);
});

/* ---------------- 选模式快捷键 ---------------- */
/**
 * 选模式下提供键盘交互：
 *  - Esc 退出选模式
 *  - Ctrl/Cmd + A 全选/取消全选
 *  - Delete 打开删除确认（仅相册记录，文件未删）
 * 输入框中、任一对话框打开时不响应。
 */
/* ---------------- 键盘交互 ---------------- */
/**
 * 选模式下提供快捷键：
 *  - Esc：优先关闭打开的弹窗；无弹窗则退出选模式
 *  - Ctrl/Cmd + A：全选/取消全选
 *  - Delete：打开删除确认
 * 输入框中不响应。
 */
function onKey(e: KeyboardEvent) {
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) return;

  // 打开的对话框优先处理 Esc：依次关闭即可
  if (e.key === "Escape") {
    if (showCreateDialog.value) { e.preventDefault(); showCreateDialog.value = false; return; }
    if (showRenameDialog.value) { e.preventDefault(); showRenameDialog.value = false; return; }
    if (mergeOpen.value) { e.preventDefault(); mergeOpen.value = false; return; }
    if (moveFolderOpen.value) { e.preventDefault(); moveFolderOpen.value = false; return; }
    if (tagDialogOpen.value) { e.preventDefault(); tagDialogOpen.value = false; return; }
    if (locDialogOpen.value) { e.preventDefault(); locDialogOpen.value = false; return; }
    if (batchDeleteConfirm.value.visible) { e.preventDefault(); batchDeleteConfirm.value.visible = false; return; }
  }

  if (!isSelectMode.value) return;
  if (
    showCreateDialog.value ||
    showRenameDialog.value ||
    mergeOpen.value ||
    moveFolderOpen.value ||
    tagDialogOpen.value ||
    locDialogOpen.value ||
    batchDeleteConfirm.value.visible
  ) return;

  if (e.key === "Escape") {
    e.preventDefault();
    exitSelectMode();
  } else if ((e.ctrlKey || e.metaKey) && (e.key === "a" || e.key === "A")) {
    e.preventDefault();
    toggleSelectAll();
  } else if (e.key === "Delete" && selectedIds.value.size > 0 && !isDeleting.value) {
    e.preventDefault();
    batchDelete();
  }
}
</script>

<template>
  <div
    class="album-page"
    :style="{
      color: theme.textColor,
      '--text': theme.textColor,
      '--sub-text': theme.subTextColor,
      '--muted': theme.isDark ? 'rgba(214,221,240,.55)' : 'rgba(60,70,90,.55)',
      '--card-bg': theme.isDark ? 'rgba(30,34,46,.92)' : 'rgba(255,255,255,.94)',
      '--card-border': theme.isDark ? 'rgba(255,255,255,.09)' : 'rgba(0,0,0,.07)',
      '--panel-bg': theme.isDark ? 'rgba(255,255,255,.06)' : '#f7f9fc',
      '--input-bg': theme.isDark ? 'rgba(20,22,30,.7)' : '#fff',
      '--input-border': theme.isDark ? 'rgba(255,255,255,.14)' : '#ddd',
      '--tint-bg': theme.isDark ? 'rgba(57,108,216,.18)' : '#eef3ff',
      '--tint-border': theme.isDark ? 'rgba(57,108,216,.35)' : '#e0e9fa',
      '--hover-bg': theme.isDark ? 'rgba(255,255,255,.07)' : 'rgba(57,108,216,.08)',
      '--danger-bg': theme.isDark ? 'rgba(229,72,77,.15)' : '#fdf0f0',
    }"
  >
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
          <button class="btn" :disabled="!store.albums.length" :title="!store.albums.length ? '请先创建相册后才能进入管理' : '进入批量管理模式（支持选择多个相册进行整理）'" @click="enterSelectMode">
            📋 批量管理
          </button>
          <button class="btn btn-primary" @click="openCreateDialog">新建相册</button>
        </template>
        <!-- 勾选模式：批量整理 / 删除 / 取消 -->
        <template v-else>
          <div class="tb-group tb-state">
            <span class="tb-selected-pill" :title="`已选 ${selectedIds.size} / ${store.albums.length} 个相册`">
              ✓ 已选 <b>{{ selectedIds.size }}</b>
            </span>
            <button class="link-btn" @click="toggleSelectAll" :title="allSelected ? '取消全选当前可见' : '全选当前可见（折叠中的不会勾选）'">
              {{ allSelected ? "取消全选" : "全选" }}
            </button>
            <button class="btn" @click="exitSelectMode" title="退出批量管理（Esc）">取消</button>
          </div>

          <div class="tb-divider"></div>

          <div class="tb-group tb-organize">
            <span class="tb-group-label">整理</span>
            <button
              class="btn"
              :disabled="selectedIds.size < 1 || batchRunning"
              :title="mergeTitle"
              @click="openMerge"
            >🪢 合并…</button>
            <button
              class="btn"
              :disabled="selectedIds.size === 0 || batchRunning"
              :title="selectedIds.size === 0 ? '请先勾选至少 1 个相册' : '把选中相册移动到其他分组（顶级/二级/三级）'"
              @click="openMoveFolder"
            >📁 移动分组…</button>
          </div>

          <div class="tb-divider"></div>

          <div class="tb-group tb-label">
            <span class="tb-group-label">标签</span>
            <button
              class="btn"
              :disabled="selectedIds.size === 0 || batchRunning"
              :title="selectedIds.size === 0 ? '请先勾选至少 1 个相册' : '批量添加 / 移除标签（弹窗内选择模式）'"
              @click="openTagDialog('add')"
            >🏷️ 标签…</button>
            <button
              class="btn"
              :disabled="selectedIds.size === 0 || batchRunning"
              :title="selectedIds.size === 0 ? '请先勾选至少 1 个相册' : '批量修改选中相册的地点（留空清除）'"
              @click="openLocDialog"
            >📍 改地点…</button>
          </div>

          <div class="tb-divider"></div>

          <div class="tb-group tb-danger">
            <span class="tb-group-label">危险</span>
            <button
              class="btn btn-danger"
              :disabled="isDeleting || selectedIds.size === 0"
              :title="selectedIds.size === 0 ? '请先勾选至少 1 个相册' : `仅删除选中相册的数据库记录，照片文件保留在磁盘原处`"
              @click="batchDelete"
            >
              {{ isDeleting ? "删除中…" : `🗑️ 删除 (${selectedIds.size})` }}
            </button>
          </div>
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

    <!-- 批量选择提示栏（勾选模式下提示快捷键；计数与全选在工具栏中） -->
    <div v-if="isSelectMode" class="select-bar">
      <span class="select-hint">
        点击卡片勾选 · <kbd>Esc</kbd> 退出 · <kbd>Ctrl+A</kbd> 全选当前可见 · <kbd>Del</kbd> 删除（仅除记录）
      </span>
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
                    @open-source-path="openSourcePath($event)"
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
                        @open-source-path="openSourcePath($event)"
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
                  @open-source-path="openSourcePath($event)"
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

    <!-- 批量整理：移动分组 -->
    <div v-if="moveFolderOpen" class="dialog-mask" @click.self="moveFolderOpen = false">
      <div class="dialog">
        <h2 class="dialog-title">移动分组</h2>
        <p v-if="moveSelectedFolders.length" class="batch-current-tip">
          当前所在：<b>{{ moveSelectedFolders.join(" · ") }}</b>
        </p>
        <p class="batch-select-tip">选择目标分组（缩进表示层级），或选「不分组」移到顶级</p>
        <div class="folder-list">
          <button class="folder-item" :class="{ active: moveTarget === null }" @click="moveTarget = null">
            <span>— 不分组（顶级）—</span>
          </button>
          <button
            v-for="f in moveFolders"
            :key="f.id"
            class="folder-item"
            :class="{ active: moveTarget === f.id }"
            :style="{ paddingLeft: (f.level - 1) * 16 + 12 + 'px' }"
            @click="moveTarget = f.id"
          >
            <span>📁 {{ f.name }}</span>
          </button>
        </div>
        <div class="dialog-actions">
          <button class="btn" @click="moveFolderOpen = false">取消</button>
          <button class="btn btn-primary" :disabled="batchRunning" @click="doMoveFolder">移动所选</button>
        </div>
      </div>
    </div>

    <!-- 批量整理：打标签 -->
    <div v-if="tagDialogOpen" class="dialog-mask" @click.self="tagDialogOpen = false">
      <div class="dialog">
        <h2 class="dialog-title">批量标签</h2>
        <p class="batch-select-tip">模式：</p>
        <div class="tag-mode">
          <button class="bmode" :class="{ active: tagMode === 'add' }" @click="tagMode = 'add'">
            ➕ 添加标签
          </button>
          <button class="bmode" :class="{ active: tagMode === 'remove' }" @click="tagMode = 'remove'">
            ➖ 移除标签
          </button>
        </div>
        <div class="form-field">
          <label class="form-label">标签名（单个）</label>
          <input
            v-model="tagInput"
            class="input"
            :placeholder="tagMode === 'add' ? '如：精选 / 旅行 / 家人' : '要移除的标签名（必须完全匹配）'"
            @keydown.enter="doTagDialog"
            @keydown.esc="tagDialogOpen = false"
          />
        </div>
        <div class="dialog-actions">
          <button class="btn" @click="tagDialogOpen = false">取消</button>
          <button class="btn btn-primary" :disabled="batchRunning || !tagInput.trim()" @click="doTagDialog">
            {{ tagMode === "add" ? "添加" : "移除" }}
          </button>
        </div>
      </div>
    </div>

    <!-- 批量整理：改地点 -->
    <div v-if="locDialogOpen" class="dialog-mask" @click.self="locDialogOpen = false">
      <div class="dialog">
        <h2 class="dialog-title">批量改地点</h2>
        <p v-if="locCurrent.length" class="batch-current-tip">
          当前地点：<b>{{ locCurrent.join(" · ") }}</b>
        </p>
        <div class="form-field">
          <label class="form-label">新地点标签（留空清除）</label>
          <input
            v-model="locInput"
            class="input"
            placeholder="如：成都"
            @keydown.enter="doLocDialog"
            @keydown.esc="locDialogOpen = false"
          />
        </div>
        <p v-if="locWillOverwrite" class="batch-overwrite-warn">
          ⚠️ 部分选中相册已为「{{ locInput.trim() }}」，本次保存会<strong>覆盖</strong>原有值。
        </p>
        <div class="dialog-actions">
          <button class="btn" @click="locDialogOpen = false">取消</button>
          <button class="btn btn-primary" :disabled="batchRunning" @click="doLocDialog">保存</button>
        </div>
      </div>
    </div>

    <!-- 批量整理：合并相册目标选择 -->
    <div v-if="mergeOpen" class="dialog-mask" @click.self="mergeOpen = false">
      <div class="dialog">
        <h2 class="dialog-title">合并到目标相册</h2>
        <div class="merge-mode">
          <button class="bmode" :class="{ active: mergeMode === 'move' }" @click="mergeMode = 'move'">物理移动</button>
          <button class="bmode" :class="{ active: mergeMode === 'record' }" @click="mergeMode = 'record'">仅删记录</button>
        </div>
        <p class="batch-select-tip">
          {{ mergeMode === "move"
            ? "把照片文件移入目标相册文件夹（源相册记录随之删除）。"
            : "仅删除源相册记录，照片文件留在磁盘原处（不再归属任何相册）。" }}
        </p>
        <div class="folder-list">
          <button
            v-for="t in mergeTargets"
            :key="t.id"
            class="folder-item"
            :class="{ active: mergeTarget === t.id }"
            @click="mergeTarget = t.id"
          >
            <span>📁 {{ t.name }}（{{ t.photo_count }} 张）</span>
          </button>
        </div>
        <div class="dialog-actions">
          <button class="btn" @click="mergeOpen = false">取消</button>
          <button class="btn btn-primary" :disabled="batchRunning || mergeTarget === null" @click="requestMerge">下一步：确认合并</button>
        </div>
      </div>
    </div>

    <!-- 合并且二次确认（物理移动文件，不可恢复） -->
    <ConfirmDialog
      :visible="mergeConfirm.visible"
      title="合并相册"
      :message="mergeConfirm.message"
      confirm-text="确认合并"
      @confirm="doMerge"
      @cancel="mergeConfirm.visible = false"
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
  flex-wrap: wrap;
  justify-content: flex-end;
}

/* ---- 选模式工具栏：按用途分组 ---- */
.tb-group {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px;
  border-radius: 10px;
}
.tb-group-label {
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.5px;
  opacity: 0.7;
  margin-right: 2px;
  user-select: none;
}
.tb-divider {
  width: 1px;
  height: 24px;
  background: var(--card-border);
  flex-shrink: 0;
}
.tb-state {
  background: var(--tint-bg);
  border: 1px solid var(--tint-border);
}
.tb-state .btn {
  background: transparent;
  border: 1px solid var(--tint-border);
  color: inherit;
}
.tb-state .btn:hover {
  background: var(--hover-bg);
}
.tb-organize {
  background: rgba(57, 108, 216, 0.06);
  border: 1px solid rgba(57, 108, 216, 0.18);
}
.tb-label {
  background: rgba(34, 159, 110, 0.06);
  border: 1px solid rgba(34, 159, 110, 0.18);
}
.tb-danger {
  background: rgba(229, 72, 77, 0.08);
  border: 1px solid rgba(229, 72, 77, 0.22);
}
.tb-selected-pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  font-size: 13px;
  font-weight: 600;
  color: #2f5de0;
  background: rgba(57, 108, 216, 0.15);
  border: 1px solid rgba(57, 108, 216, 0.3);
  border-radius: 999px;
}
.tb-selected-pill b {
  font-size: 14px;
  color: #2f5de0;
}

.select {
  padding: 8px 12px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  font-size: 14px;
  background: var(--input-bg);
  color: var(--text);
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
  background: var(--panel-bg);
  border: 1px solid var(--card-border);
  border-radius: 8px;
  padding: 8px 16px;
  margin-bottom: 20px;
  font-size: 13px;
  color: var(--sub-text);
  overflow-x: auto;
  white-space: nowrap;
}

.view-bar-label {
  color: #396cd8;
  font-weight: 600;
  margin-right: 4px;
}

.bc-sep {
  color: var(--muted);
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
  color: var(--muted);
  pointer-events: none;
}

.search-input {
  width: 100%;
  padding: 10px 36px 10px 36px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  box-sizing: border-box;
  color: var(--text);
  background: var(--input-bg);
  transition: border-color 0.2s;
}

.search-input::placeholder {
  color: var(--muted);
}

.search-input:focus {
  border-color: #396cd8;
}

.search-clear {
  position: absolute;
  right: 10px;
  border: none;
  background: none;
  color: var(--muted);
  font-size: 18px;
  cursor: pointer;
  line-height: 1;
}

.search-clear:hover {
  color: var(--text);
}

/* 搜索结果面板 */
.search-results {
  background: var(--card-bg);
  border: 1px solid var(--card-border);
  border-radius: 10px;
  padding: 12px;
  margin-bottom: 16px;
  max-height: 400px;
  overflow-y: auto;
}

.search-status {
  color: var(--muted);
  font-size: 14px;
  text-align: center;
  padding: 20px 0;
}

/* 搜索结果分组（相册 / 照片内容） */
.search-section + .search-section {
  margin-top: 6px;
  border-top: 1px solid var(--card-border);
  padding-top: 8px;
}

.search-section-title {
  font-size: 12px;
  color: var(--muted);
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
  background: var(--hover-bg);
}

.result-cover {
  width: 48px;
  height: 36px;
  object-fit: cover;
  border-radius: 6px;
  flex-shrink: 0;
}

.result-placeholder {
  background: var(--panel-bg);
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
  color: var(--text);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.result-path {
  font-size: 12px;
  color: var(--muted);
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
  background: var(--tint-bg);
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
  color: var(--muted);
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
  border: 1px solid var(--card-border);
  border-radius: 12px;
  overflow: hidden;
  background: var(--card-bg);
}

/* 路线图跳转后的高亮动画 */
.year-highlight {
  animation: yearFlash 1.5s ease;
}

@keyframes yearFlash {
  0% {
    box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.4);
    background: var(--hover-bg);
  }
  100% {
    box-shadow: none;
    background: var(--card-bg);
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
  background: var(--hover-bg);
}

.group-year {
  background: var(--tint-bg);
  border-bottom: 1px solid var(--tint-border);
}

.group-season {
  background: var(--panel-bg);
  border-bottom: 1px solid var(--card-border);
}

.group-month {
  background: var(--panel-bg);
  border-bottom: 1px solid var(--card-border);
  padding: 8px 16px 8px 32px;
}

.group-title {
  font-weight: 600;
  font-size: 15px;
  color: var(--text);
}

.group-count {
  font-size: 12px;
  color: var(--muted);
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
  color: var(--muted);
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
  color: var(--muted);
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
  background: var(--card-bg);
  border: 1px solid var(--card-border);
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
  color: var(--text);
  cursor: pointer;
  transition: background 0.15s;
}

.context-menu-item:hover {
  background: var(--hover-bg);
}

.context-menu-danger {
  color: #e5484d;
}

.context-menu-danger:hover {
  background: var(--danger-bg);
}

.ctx-icon {
  font-size: 14px;
}

/* 通用按钮 */
.btn {
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid var(--input-border);
  background: var(--card-bg);
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
  background: var(--panel-bg);
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
  color: var(--sub-text);
}

/* 批量选择提示栏 */
.select-bar {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  background: var(--tint-bg);
  border: 1px solid var(--tint-border);
  border-radius: 8px;
  padding: 8px 16px;
  margin-bottom: 16px;
  font-size: 14px;
  color: var(--text);
}

.select-hint {
  font-size: 12px;
  color: var(--muted);
}
.select-hint kbd {
  display: inline-block;
  min-width: 22px;
  padding: 1px 6px;
  margin: 0 1px;
  font-size: 11px;
  font-family: inherit;
  line-height: 1.4;
  color: var(--text, #4a5568);
  background: var(--panel-bg, rgba(127, 127, 127, 0.12));
  border: 1px solid var(--input-border, rgba(127, 127, 127, 0.3));
  border-radius: 4px;
  vertical-align: 1px;
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
  border: 1px solid var(--card-border);
  border-radius: 12px;
  overflow: hidden;
  background: var(--card-bg);
  margin-bottom: 12px;
}

.group-location {
  background: var(--tint-bg);
  border-bottom: 1px solid var(--tint-border);
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
  background: var(--card-bg);
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
  color: var(--text);
}

.path-row {
  display: flex;
  gap: 8px;
}

.input {
  flex: 1;
  padding: 8px 12px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  color: var(--text);
  background: var(--input-bg);
}

.input:focus {
  border-color: #396cd8;
}

.textarea {
  width: 100%;
  min-height: 90px;
  padding: 8px 12px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  font-size: 14px;
  outline: none;
  resize: vertical;
  color: var(--text);
  background: var(--input-bg);
  box-sizing: border-box;
}

.char-count {
  display: block;
  text-align: right;
  font-size: 12px;
  color: var(--muted);
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

/* 批量整理弹窗：分组/合并目标列表 */
.batch-select-tip {
  font-size: 12px;
  opacity: 0.72;
  margin: 0 0 10px;
}
.batch-current-tip {
  font-size: 12.5px;
  color: #4a5568;
  background: var(--tint-bg, #eef3ff);
  border: 1px solid var(--tint-border, #dbe3ff);
  border-radius: 8px;
  padding: 6px 10px;
  margin: 0 0 8px;
}
.batch-overwrite-warn {
  font-size: 12.5px;
  color: #6a4f00;
  background: #fff8e6;
  border: 1px solid #ffe2a0;
  border-radius: 8px;
  padding: 6px 10px;
  margin: 6px 0 0;
}
.batch-overwrite-warn strong { color: #d95a00; }
.folder-list {
  max-height: 300px;
  overflow-y: auto;
  border: 1px solid var(--card-border);
  border-radius: 8px;
  padding: 4px;
  margin-bottom: 14px;
}
.folder-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  text-align: left;
  padding: 9px 12px;
  border: 0;
  border-radius: 6px;
  font-size: 14px;
  color: inherit;
  cursor: pointer;
  transition: background 0.15s;
}
.folder-item:hover {
  background: rgba(120, 120, 140, 0.1);
}
.folder-item.active {
  background: rgba(57, 108, 216, 0.12);
  font-weight: 600;
}

/* 合并模式切换 */
.merge-mode {
  display: inline-flex;
  gap: 6px;
  margin-bottom: 8px;
}
.bmode {
  padding: 6px 14px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  background: var(--card-bg);
  font-size: 13px;
  cursor: pointer;
  transition: all 0.15s;
}
.bmode.active {
  background: rgba(57, 108, 216, 0.12);
  border-color: rgba(57, 108, 216, 0.5);
  font-weight: 600;
}

/* 回到顶部按钮 */
.back-to-top {
  position: fixed;
  right: 24px;
  bottom: 24px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  border: 1px solid var(--input-border);
  background: var(--card-bg);
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
  background: var(--tint-bg);
  transform: scale(1.08);
}

.back-to-top:active {
  transform: scale(0.95);
}
</style>
