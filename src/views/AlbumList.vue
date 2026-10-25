<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useAlbumStore } from "../stores/album";
import { useContentStore } from "../stores/content";
import { useThemeStore } from "../stores/theme";
import { SCAN_MODE_TITLE, SCAN_MODE_TIP } from "../utils/scanModeTip";
import type { Album, CreateAlbumInput } from "../types/album";
import { isVirtualAlbum } from "../types/album";
import { useVaultStore } from "../stores/vault";
import type { Folder, ManualTree } from "../types/folder";
import { justifyRows, type JustifiedRow } from "../utils/justifiedRows";
import ManualSort from "./ManualSort.vue";
import AlbumCard from "../components/AlbumCard.vue";
import StatBand, { type StatItem } from "../components/ui/StatBand.vue";
import AppHeader from "../components/ui/AppHeader.vue";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import ImportErrorsDialog from "../components/ImportErrorsDialog.vue";
import EmptyState from "../components/EmptyState.vue";
import { useNotify } from "../composables/useNotify";
import type { ImportResult } from "../stores/album";

const router = useRouter();
const route = useRoute();
const store = useAlbumStore();
const contentStore = useContentStore();
const theme = useThemeStore();
const notify = useNotify();
/** FEAT-095：隐藏相册解锁态（未解锁时该相册不在 store.albums 里） */
const vault = useVaultStore();

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
  // FEAT-095：归档 / 隐藏是虚拟相册，path 是哨兵值而非真实目录，无法「在资源管理器中打开」
  if (isVirtualPath(path)) {
    notify.info("系统内置相册", "「归档」与「隐藏」相册是照片的聚合视图，没有对应的本地文件夹。");
    return;
  }
  try {
    await invoke("open_folder", { path });
  } catch (e) {
    notify.error("无法打开文件夹", `${path}\n${e}`);
  }
}

/** FEAT-095：判断是否为虚拟相册哨兵路径（`__virtual__://archive|hidden`） */
function isVirtualPath(path: string): boolean {
  return path.startsWith("__virtual__://");
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

/** FEAT-034-A：最近一次导入结果（用于错误详情对话框） */
const lastImportResult = ref<ImportResult | null>(null);
/** 错误详情弹窗显示状态 */
const showImportErrors = ref(false);
/** 用户选择的根目录（重试用） */
const importRoot = ref<string>("");

/**
 * 批量导入：选择一个大文件夹，遍历其一级子文件夹创建相册
 * 通过监听后端 import-progress 事件实时更新进度条
 *
 * FEAT-034-A：失败时弹窗 ImportErrorsDialog 展示明细 + 解决建议；
 * Toast 上提供「查看详情」按钮直达对话框。
 */
async function batchImport() {
  if (isImporting.value) return;
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择要批量导入的根文件夹",
  });
  if (typeof selected !== "string") return;

  importRoot.value = selected;
  await runImport(selected, /* showErrorsOnFail */ true);
}

/** 执行一次导入（用于首次/重试），失败时是否弹窗 */
async function runImport(rootPath: string, showErrorsOnFail: boolean) {
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
    const result = await store.importAlbums(rootPath);
    importProgress.value = 100;
    lastImportResult.value = result;
    // FEAT-034-C：跨用户占用现在是“已存在”友好跳过场景，不再视为失败
    const alreadyExists = (result.skipped_conflicts ?? []).length;
    const parts = [`成功导入 ${result.imported} 个相册`];
    if (result.skipped > 0) parts.push(`跳过 ${result.skipped} 个已存在的`);
    if (result.errors.length > 0) parts.push(`失败 ${result.errors.length} 个`);

    if (result.errors.length > 0) {
      // 失败：Toast 提示 + 「查看详情」按钮直达对话框
      notify.action(
        "error",
        "批量导入部分失败",
        [
          {
            label: "查看详情",
            style: "primary",
            onClick: () => {
              showImportErrors.value = true;
            },
          },
        ],
        parts.join("，") + "\n点击「查看详情」可逐项查看原因与解决建议。",
        0, // 持久化
      );
      console.error("[FEAT-034-A] 导入失败的文件夹:", result.errors);
      if (showErrorsOnFail) {
        // 首次失败自动弹窗，避免用户必须再点一次
        showImportErrors.value = true;
      }
    } else if (alreadyExists > 0) {
      // FEAT-034-C：所有项都已存在（跨用户或同用户占用的合并场景）→ 友好信息框
      const msg =
        result.imported > 0
          ? `${result.imported} 个新相册已导入；${alreadyExists} 个相册此前已导入，自动跳过。`
          : `${alreadyExists} 个相册已被导入，无需重复导入。`;
      // 弹信息框列出具体哪些相册已存在（不标红为错误）；同时给友好 toast
      showImportErrors.value = true;
      notify.success("批量导入完成", msg);
    } else {
      notify.success("批量导入完成", parts.join("，"));
    }
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

/** FEAT-034-A：失败详情对话框中点击「重试」后回调（folderNames 是失败项的子目录名） */
async function retryImport(folderNames: string[]) {
  if (!importRoot.value) {
    notify.warning("无法重试", "未记录根目录路径，请重新选择。");
    showImportErrors.value = false;
    return;
  }
  // 关闭对话框并重新执行：原 `importAlbums` 会自动跳过同名/已存在的相册，
  // 因此重试只会处理「上次失败」的项目（同名冲突会再次跳过，但不会重复导入已成功的项）。
  showImportErrors.value = false;
  notify.info("正在重试失败项", `根目录：${importRoot.value}\n失败项数：${folderNames.length}`);
  await runImport(importRoot.value, /* showErrorsOnFail */ true);
}

/** FEAT-034-A：失败详情对话框中点击「重新选择根目录」 */
async function reimportFromRoot() {
  showImportErrors.value = false;
  const selected = await open({
    directory: true,
    multiple: false,
    title: "重新选择根文件夹",
  });
  if (typeof selected !== "string") return;
  importRoot.value = selected;
  await runImport(selected, true);
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
      "点击卡片勾选 · Ctrl+A 全选当前可见 · Esc 退出 · Del 删除（仅删记录）",
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
  // date 模式：整墙扁平密排，没有可折叠分组，所有相册恒可见
  return dateOrderedAlbums.value.map((a) => a.id);
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

/* FEAT-037：批量扫描入库（勾选模式下对选中相册组合扫描 EXIF/影调/AI 并写库） */
const scanDialogOpen = ref(false);
// FEAT-SEM：语义向量默认勾选
const batchScanTypes = ref<string[]>(["basic", "tone", "person", "semantic"]);
const batchScanBatch = ref(8);
const batchScanning = ref(false);
/** FEAT-SEM：扫描方式确认（覆盖 / 增量 / 取消） */
const batchModeDialogVisible = ref(false);
const batchScanResult = ref<{ scanned: number; failed: { albumId: number; error: string }[]; total: number } | null>(null);

/** 打开批量扫描弹窗 */
function openBatchScan() {
  // 默认三种扫描方式全选
  batchScanTypes.value = ["basic", "tone", "person", "semantic"];
  batchScanBatch.value = 8;
  batchScanResult.value = null;
  scanDialogOpen.value = true;
}

const batchScanProgress = ref("");

/** 执行批量扫描：串行逐个相册扫描，弹窗内展示进度，完成后反馈汇总 */
function doBatchScan() {
  const ids = [...selectedIds.value];
  if (!ids.length) return;
  if (batchScanning.value) return;
  if (batchScanTypes.value.length === 0) {
    notify.warning("请选择至少一种扫描方式");
    return;
  }
  batchModeDialogVisible.value = true;
}

/** 用户在对话框选定扫描方式后执行批量扫描 */
async function launchBatchScan(overwrite: boolean) {
  batchModeDialogVisible.value = false;
  const ids = [...selectedIds.value];
  if (!ids.length) return;
  batchScanning.value = true;
  batchScanResult.value = null;
  try {
    const res = await contentStore.scanAlbumsCombined(
      ids,
      batchScanTypes.value,
      batchScanBatch.value,
      overwrite,
      (done, total) => {
        batchScanProgress.value = `已完成 ${done} / ${total} 个相册`;
      },
    );
    batchScanResult.value = { ...res, total: ids.length };
    // 成功后刷新相册列表，让「已入库」标记与统计更新
    await store.fetchAlbums().catch(() => {});
    if (res.failed.length) {
      notify.warning(
        "批量扫描完成（部分失败）",
        `成功 ${res.scanned}/${ids.length} 个相册；失败 ${res.failed.length} 个`,
      );
    } else {
      notify.success("批量扫描入库完成", `已扫描入库 ${res.scanned} 个相册`);
    }
    batchScanProgress.value = "";
    scanDialogOpen.value = false;
    exitSelectMode();
  } catch (e) {
    notify.error("批量扫描失败", String(e));
    batchScanProgress.value = "";
  } finally {
    batchScanning.value = false;
  }
}

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

/** FEAT-095：右键目标是否为系统内置虚拟相册（据此隐藏重命名 / 删除） */
const contextMenuIsVirtual = computed(() => {
  const album = store.albums.find((a) => a.id === contextMenu.value.albumId);
  return album ? isVirtualAlbum(album) : false;
});

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

/**
 * FEAT-095：归档 / 隐藏虚拟相册
 *
 * 这两个相册没有真实文件夹与拍摄时间，混进按年份/季节的分组里既无意义也难找，
 * 故单独抽出来在顶部固定成一块。普通相册的分组 / 排序视图一律排除它们
 * （`normalAlbums` 是唯一喂给分组逻辑的口子）。
 */
const virtualAlbums = computed(() => store.albums.filter(isVirtualAlbum));
const normalAlbums = computed(() => store.albums.filter((a) => !isVirtualAlbum(a)));

/** 按地点排序的相册（手动 location 标签），无地点的排在最后 */
const locationSortedAlbums = computed(() => {
  return [...normalAlbums.value].sort((a, b) => {
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

/* -------------------- 左侧栏筛选（FEAT-105） --------------------
   左栏是**模块内部件**，不是导航：它只收窄当前页的可见集合。
   三项互不冲突，可叠加：年份（date 模式可用的时间维度） × 入库状态 × 搜索词。
   选择写 localStorage：用户改过的视图要能留住（与 sortMode 一致的做法）。 */

/** 入库状态筛选：all=全部 · in=已入库 · out=未入库 */
const scanFilter = ref<"all" | "in" | "out">(
  (localStorage.getItem("album-scan-filter") as "all" | "in" | "out") ?? "all",
);
function setScanFilter(v: "all" | "in" | "out") {
  scanFilter.value = v;
  localStorage.setItem("album-scan-filter", v);
}

/** 年份筛选：null = 全部年份（0 代表"未分类"这一组，与 groupByTime 的口径一致） */
const yearFilter = ref<number | null>(null);
function setYearFilter(y: number | null) {
  yearFilter.value = y;
  // 切换年份时展开全部，否则用户点了年份却看到全是折叠的
  collapsed.value = new Set();
}

/** 当前可见相册：左栏筛选叠在普通相册上（虚拟相册单独成块，不受这里影响） */
const visibleAlbums = computed(() => {
  let list = normalAlbums.value;
  if (scanFilter.value === "in") list = list.filter((a) => (a.scanned_photo_count || 0) > 0);
  if (scanFilter.value === "out") list = list.filter((a) => !a.scanned_photo_count);
  // 年份筛选：扁平密排后没有分組标题可跳，所以直接在这里收窄集合
  if (yearFilter.value !== null) {
    list = list.filter((a) => {
      const y = a.shoot_time ? Number(String(a.shoot_time).slice(0, 4)) || 0 : 0;
      return y === yearFilter.value;
    });
  }
  return list;
});

/** 左栏"年份"区：从全量普通相册统计，不随年份筛选变化（否则选完就只剩一行） */
const yearBuckets = computed<Array<{ year: number; count: number }>>(() => {
  const map = new Map<number, number>();
  for (const a of normalAlbums.value) {
    const y = a.shoot_time ? Number(String(a.shoot_time).slice(0, 4)) || 0 : 0;
    map.set(y, (map.get(y) ?? 0) + 1);
  }
  return [...map.entries()]
    .map(([year, count]) => ({ year, count }))
    .sort((a, b) => (a.year === 0 ? 1 : b.year === 0 ? -1 : b.year - a.year));
});

/** 地点分组：按地点名分组（保持 A-Z 顺序，无地点排最后） */
const locationGroups = computed<Array<{ location: string | null; albums: Album[] }>>(() => {  const groups: Array<{ location: string | null; albums: Album[] }> = [];
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

/** 折叠状态：key 为地点分组的唯一标识（日期墙是扁平密排，没有折叠分组） */
const collapsed = ref<Set<string>>(new Set());

/** 年份筛选下的可见相册数（左栏右上角那个"12 个相册 · 653 张"用它） */
const visibleCount = computed(() => visibleAlbums.value.length);

/** FEAT-105：日期模式的扁平列表 —— 按拍摄时间倒序（新的在前）。
 *  原来靠 年→季→月 三层嵌套表达时间，现在改成一条时间线：
 *  时间信息在每张卡片自己的封面左下角，读起来是"扫过一条时间线"，
 *  而不是"翻一本按月份分好组的目录"。 */
const dateOrderedAlbums = computed(() =>
  [...visibleAlbums.value].sort((a, b) => {
    const ta = a.shoot_time ?? "";
    const tb = b.shoot_time ?? "";
    if (ta === tb) return (b.photo_count || 0) - (a.photo_count || 0);
    return tb.localeCompare(ta);
  }),
);

/**
 * FEAT-103：顶部统计数据带（替代原来的「无统计」空白页头）
 *
 * 这一页之前**没有任何聚合读数** —— 用户看到的是一堆卡片，但不知道
 * "总量多少 / 多少已入库 / 多少还没扫 / 占多大空间"。这些恰恰是相册管理
 * 最该先回答的问题，所以数据带放在页头正下方，先于任何分组。
 *
 * 读数的取舍依据：
 *   · 相册 / 照片 —— 最基础的两个量级
 *   · 已入库 / 待入库 —— 直接指向"下一步该做什么"（待入库非 0 就该去扫）
 *   · 占用 —— 本机存储压力，笔记本用户关心
 *   ← 组级/卡片级已有"每相册张数、拍摄年、路径"，不重复上移
 */
/**
 * 数据带的微型柱阵（FEAT-106）：按年份分布的相册数。
 *
 * 为什么这个数据有意义：它是“照片墙上方的一眼概览”——
 * 哪个年份的相册多，一眼看到。而不用点开左栏的年份列表。
 */
const yearBars = computed(() => {
  const map = new Map<number, number>();
  for (const a of normalAlbums.value) {
    const y = a.shoot_time ? Number(String(a.shoot_time).slice(0, 4)) || 0 : 0;
    map.set(y, (map.get(y) ?? 0) + 1);
  }
  const years = [...map.entries()].sort((a, b) => a[0] - b[0]);
  if (!years.length) return [];
  const max = Math.max(...years.map(([, n]) => n));
  return years.map(([, n]) => (n / max) * 100);
});

const statItems = computed<StatItem[]>(() => {
  const all = normalAlbums.value;
  const totalPhotos = all.reduce((n, a) => n + (a.photo_count || 0), 0);
  const scannedAlbums = all.filter((a) => (a.scanned_photo_count || 0) > 0).length;
  const pending = all.length - scannedAlbums;
  const scannedPhotos = all.reduce((n, a) => n + (a.scanned_photo_count || 0), 0);
  const totalBytes = all.reduce((n, a) => n + (a.size_bytes || 0), 0);
  const gb = totalBytes / 1024 / 1024 / 1024;

  return [
    {
      label: "相册",
      value: all.length,
      delta: virtualAlbums.value.length ? `+${virtualAlbums.value.length} 系统` : "",
      // 相册数占「全部相册」的比例：1（即 100%），意味着没有待新建的
      progress: all.length ? 1 : 0,
      progressTone: "muted",
      // 年度分布柱阵：一眼看出哪个年份的相册多
      bars: yearBars.value,
    },
    {
      label: "照片",
      value: totalPhotos,
      // 照片数没有天然上限，用「已入库比例」作为它的进度（与「已入库」格呼应）
      progress: totalPhotos ? scannedPhotos / totalPhotos : 0,
      progressTone: "muted",
    },
    {
      label: "已入库",
      value: scannedPhotos,
      delta: `${scannedAlbums} 个相册`,
      tone: scannedAlbums ? "up" : "mute",
      // FEAT-106：进度条 = 入库完成度（已入库照片 / 总照片）
      progress: totalPhotos ? scannedPhotos / totalPhotos : 0,
    },
    {
      label: "待入库",
      value: pending,
      delta: pending ? "需要扫描" : "全部已扫",
      tone: pending ? "up" : "mute",
      // 待入库：显示的是「还差多少」的补数进度，用弱化色（不抢"已入库"的强调色）
      progress: totalPhotos ? 1 - scannedPhotos / totalPhotos : 0,
      progressTone: "muted",
    },
    {
      label: "占用",
      value: gb >= 100 ? Math.round(gb) : gb.toFixed(1),
      unit: "GB",
      // 占用：相对 100GB 的比例，给个"占满本地空间"的直觉
      progress: Math.min(1, gb / 100),
      progressTone: "muted",
    },
  ];
});

/** 页头读数：一行说清这页装了多少（取代旧工具栏里的"我的相册"大标题） */
const headerMeta = computed(() => {
  const n = normalAlbums.value.length;
  const v = virtualAlbums.value.length;
  return v ? `${n} 个相册 · ${v} 个系统相册` : `${n} 个相册`;
});

/* ══════════════════════════════════════════════════════════════════════════
   FEAT-106：行式密排（justified rows）
   ──────────────────────────────────────────────────────────────────────────
   旧实现用 CSS grid 的 `auto-fill` + `grid-auto-flow: dense` + 大瓦片跨 2×4，
   靠 dense 回填制造错落。实测两个硬伤：
     ① 行尾出现无法回填的**空洞**（dense 需要连续空位，跨列大瓦片上方
        那两列已被占满，剩下的空位是奇数格，回填不上）；
     ② dense 的回填顺序依赖元素流 —— 筛选/滚动后同一个相册会"跳位置"。
   现在改成行式密排（见 utils/justifiedRows.ts）：每行是一个 flex 容器，
   瓦片宽度按整数比例分配、行宽精确铺满容器，没有空洞也没有回填。

   容器宽度靠 ResizeObserver 量（而不是写死）：左右栏宽度会随窗口变，
   写死的行宽会在窄窗下溢出。
   ══════════════════════════════════════════════════════════════════════════ */

/** 密排容器的宽度（px）：0 = 还没量到（首帧或容器隐藏） */
const wallWidth = ref(0);
/** 密排容器元素 */
const wallEl = ref<HTMLElement | null>(null);
let wallRO: ResizeObserver | null = null;

/** 相册墙的行式密排结果 */
const wallRows = computed<JustifiedRow[]>(() =>
  justifyRows({
    count: dateOrderedAlbums.value.length,
    containerWidth: wallWidth.value,
    gap: 12,
    // 图注已嵌入封面内部（左下角落款），行高 = 封面高，不再预留图注条
    captionH: 0,
    // 名义高度 232：实际封面高由缩放决定（≤ 232）。
    // 基础比例 1.28（约 5:4）比 4:3 更方 —— 参考图里的照片面板接近正方形，
    // 而不是扁扁的横幅。1.28 × 232 ≈ 297px 宽（行内 3~4 块），
    // 封面高约 232px，接近方形。
    nominalHeight: 232,
    baseAspect: 1.28,
    wideEvery: 5,
    // 宽幅 = 窄幅的 1.8 倍宽（5:4 → 2.25:1），足够读出"这块不一样"
    wideFactor: 1.8,
    minCoverH: 130,
    maxCoverH: 260,
  }),
);

/** 行分组：渲染时按行输出 flex 容器 */
const wallRowGroups = computed(() =>
  wallRows.value.map((row) => ({
    row,
    items: row.cells.map((cell) => ({ cell, album: dateOrderedAlbums.value[cell.index] })),
  })),
);

/** 量容器宽度；用 requestAnimationFrame 节流，避免 resize 风暴时反复重排 */
function measureWall() {
  const el = wallEl.value;
  if (!el) return;
  const w = el.clientWidth;
  if (w > 0 && Math.abs(w - wallWidth.value) > 1) wallWidth.value = w;
}

/** 观察容器尺寸变化（窗口缩放 / 左栏折叠 / 字体加载导致的回流） */
function observeWall(el: HTMLElement | null) {
  wallRO?.disconnect();
  wallRO = null;
  wallEl.value = el;
  if (!el) return;
  measureWall();
  if (typeof ResizeObserver !== "undefined") {
    wallRO = new ResizeObserver(() => {
      if (rafWallId != null) return;
      rafWallId = requestAnimationFrame(() => {
        rafWallId = null;
        measureWall();
      });
    });
    wallRO.observe(el);
  }
}

let rafWallId: number | null = null;

onMounted(() => {
  window.addEventListener("resize", measureWall, { passive: true });
  // 挂载后建立尺寸观察（模板 ref 此时才拿到元素）
  observeWall(wallEl.value);
});
onBeforeUnmount(() => {
  window.removeEventListener("resize", measureWall);
  wallRO?.disconnect();
  wallRO = null;
  if (rafWallId != null) cancelAnimationFrame(rafWallId);
});

/** 数据变化（筛选/排序）后重算一次行宽 */
watch(dateOrderedAlbums, () => {
  // 等 DOM 更新后再量：瓦片数量变了但容器宽度没变时，其实不需要重算，
  // 但重算一次最省心（算法是纯函数，幂等）。
  requestAnimationFrame(measureWall);
});

/** 切换排序模式：日期模式的墙板元素会重新挂载，需要重新建立尺寸观察。
 *  否则从「按地点」切回「按时间」时墙板宽度停在 0，页面会一片空白。 */
watch(sortMode, (mode) => {
  if (mode !== "date") {
    wallRO?.disconnect();
    wallRO = null;
    return;
  }
  requestAnimationFrame(() => observeWall(wallEl.value));
});

/* 本模块不加二级 tab：相册管理只有一块内容（时间分组 + 系统相册区块），
   没有"可以切过去的另一块"。链一个指向 /scan/test 的 tab 更是错的 ——
   那是扫描入库模块的东西。二级 tab 只在真有同级功能时才加。 */

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

/** 地点折叠 key */
function locationKey(location: string | null): string {
  return `loc-${location ?? "__none__"}`;
}

/* FEAT-105：删掉 jumpToYear —— 「时间路线图」横条已删除（年份筛选改由左栏承担），
   没有滚动目标了。 */

/** 折叠 / 展开全部地点分组（日期墙无分组，此函数仅在地点模式调用） */
function toggleAllGroups() {
  const keys = locationGroups.value.map((g) => locationKey(g.location));
  if (collapsed.value.size > 0) {
    collapsed.value = new Set(); // 已有折叠（含部分折叠）→ 全部展开
  } else {
    collapsed.value = new Set(keys); // 全部折叠
  }
}

/** 当前模式是否处于“全部折叠”状态（用于按钮文字）。
 *  地点模式要求**每个**分组都折叠才算（部分折叠时按钮仍给“全部折叠”）。 */
const allCollapsed = computed(() => {
  if (sortMode.value === "manual") {
    return manualSortRef.value?.isAllCollapsed ?? false;
  }
  return (
    locationGroups.value.length > 0 &&
    locationGroups.value.every((g) => collapsed.value.has(locationKey(g.location)))
  );
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
const isSearching = ref(false);
let searchTimer: ReturnType<typeof setTimeout> | null = null;

/* ══════════════════════════════════════════════════════════════════════════
   FEAT-107：相册管理的搜索**只搜相册名**（功能边界隔离）
   ────────────────────────────────────────────────────────────────────────
   改之前：这里用 Promise.all 同时跑 search_albums + searchPhotoContent，
   于是一个 placeholder 写着「搜索相册名称…」的框，实际在做**全库照片内容检索**，
   结果面板里还会多出一组「照片内容」。

   为什么这是缺陷而不是“功能更强大”：
     · 相册管理和智慧相册→智能搜索是**两个大组件**。用户对“搜索”的期待
       取决于他在哪个组件里：在相册管理里想找“哪个相册”，在智能搜索里
       想找“哪张照片”。同一个框同时干两件事，两边都说不清在搜什么。
     · 内容检索要走 content_scan 的内容库（可能没扫描过），而相册名检索
       是纯 SQL。两者成本、命中语义、失败原因完全不同，混在一个
       loading 态里会让“为什么没结果”无法归因。

   现在：只走 search_albums；相册名搜不到时给一句**明确出口**
   （去智能搜索），而不是默默换一种检索方式。
   ══════════════════════════════════════════════════════════════════════════ */

/** 搜索框输入（防抖）：只检索相册名 + 分组名 */
function onSearchInput() {
  if (searchTimer) clearTimeout(searchTimer);
  searchTimer = setTimeout(async () => {
    const kw = searchKeyword.value.trim();
    if (!kw) {
      searchResults.value = [];
      return;
    }
    isSearching.value = true;
    try {
      const albums = await invoke<
        Array<{ album: Album; folder_id: number | null; folder_path: string }>
      >("search_albums", { keyword: kw });
      searchResults.value = albums.map((r) => ({
        album: r.album,
        folder_id: r.folder_id,
        folder_path: r.folder_path,
      }));
    } catch {
      searchResults.value = [];
    } finally {
      isSearching.value = false;
    }
  }, 300);
}

/** 跳去智慧相册的智能搜索（照片内容检索的唯一入口） */
function gotoSmartSearch() {
  const kw = searchKeyword.value.trim();
  router.push({ path: "/search", query: kw ? { q: kw } : undefined });
}

/** 是否处于搜索状态（有输入关键词） */
const isSearchingActive = computed(() => searchKeyword.value.trim().length > 0);

/** 清除搜索 */
function clearSearch() {
  searchKeyword.value = "";
  searchResults.value = [];
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
  // 右键菜单以 fixed 定位，页面滚动/窗口变化时会与卡片脱锚，需关闭
  window.addEventListener("scroll", closeContextMenu, { passive: true });
  window.addEventListener("resize", closeContextMenu);
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
  window.removeEventListener("scroll", closeContextMenu);
  window.removeEventListener("resize", closeContextMenu);
  window.removeEventListener("keydown", onKey);
  // 清理防抖定时器与未完成的 rAF，避免组件卸载后回调仍执行
  if (searchTimer) clearTimeout(searchTimer);
  if (rafId != null) cancelAnimationFrame(rafId);
});

/* ---------------- 键盘交互 ---------------- */
/**
 * 选模式下提供快捷键：
 *  - Esc：优先关闭打开的弹窗；无弹窗则退出选模式
 *  - Ctrl/Cmd + A：全选/取消全选
 *  - Delete：打开删除确认（仅删相册记录，本地照片文件不动）
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
    if (scanDialogOpen.value) { e.preventDefault(); scanDialogOpen.value = false; return; }
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
    scanDialogOpen.value ||
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
      // 弱文字直接落在页面背景上：跟随实际背景对比度（旧 isDark 二分在浅色预设下融底）
      '--muted': theme.onBgSubColor,
      '--card-bg': theme.cardBg,
      '--card-border': theme.cardBorder,
      '--panel-bg': theme.onBgDark ? 'rgba(0,0,0,.04)' : 'rgba(255,255,255,.06)',
      '--input-bg': theme.onBgDark ? 'rgba(255,255,255,.65)' : 'rgba(20,22,30,.7)',
      '--input-border': theme.onBgDark ? 'rgba(0,0,0,.14)' : 'rgba(255,255,255,.14)',
      '--tint-bg': theme.onBgDark ? 'color-mix(in srgb, var(--color-primary) 14%, transparent)' : 'color-mix(in srgb, var(--color-primary) 18%, transparent)',
      '--tint-border': theme.onBgDark ? 'color-mix(in srgb, var(--color-primary) 40%, transparent)' : 'color-mix(in srgb, var(--color-primary) 35%, transparent)',
      '--hover-bg': theme.onBgDark ? 'color-mix(in srgb, var(--color-primary) 8%, transparent)' : 'rgba(255,255,255,.07)',
      '--danger-bg': theme.onBgDark ? 'color-mix(in srgb, var(--color-danger) 12%, transparent)' : 'color-mix(in srgb, var(--color-danger) 15%, transparent)',
    }"
  >
    <AppHeader title="相册管理" icon="📁" :meta="headerMeta" compact />

    <!-- FEAT-103：统计数据带（页头正下方，先于任何分组） -->
    <StatBand :items="statItems" class="album-stats" />


    <!--
      FEAT-105：双栏布局 —— 左栏是**组件内部的筛选器**（不是导航），
      右栏是内容。参考效果图：左侧“排序 / 入库 / 年份”负责收窄当前视图，
      顶部那条栏只负责“本组件内部的分区”，两者职责不重叠。

      为什么筛选放左栏而不是顶栏：顶栏只有 56px，塞不下“按时间/按地点/按入库 +
      8 个年份”两百多个可点区域；硬塞会让顶栏变成设置面板。
    -->
    <div class="album-layout">
      <!-- 左栏：本页内部筛选 -->
      <aside class="album-side" aria-label="相册筛选">
        <div class="side-group">
          <h3 class="side-h">排序</h3>
          <button
            v-for="opt in [
              { v: 'date', label: '按时间' },
              { v: 'location', label: '按地点' },
              { v: 'manual', label: '手动排序' },
            ]"
            :key="opt.v"
            class="side-item"
            :class="{ on: sortMode === opt.v }"
            @click="setSortMode(opt.v as 'date' | 'location' | 'manual')"
          >{{ opt.label }}</button>
        </div>

        <div class="side-group">
          <h3 class="side-h">入库</h3>
          <button
            v-for="opt in [
              { v: 'all', label: '全部' },
              { v: 'in', label: '已入库' },
              { v: 'out', label: '未入库' },
            ]"
            :key="opt.v"
            class="side-item"
            :class="{ on: scanFilter === opt.v }"
            @click="setScanFilter(opt.v as 'all' | 'in' | 'out')"
          >
            {{ opt.label }}
            <span class="side-n">{{
              opt.v === "all"
                ? normalAlbums.length
                : opt.v === "in"
                  ? normalAlbums.filter((a) => (a.scanned_photo_count || 0) > 0).length
                  : normalAlbums.filter((a) => !a.scanned_photo_count).length
            }}</span>
          </button>
        </div>

        <div class="side-group">
          <h3 class="side-h">年份</h3>
          <button class="side-item" :class="{ on: yearFilter === null }" @click="setYearFilter(null)">
            全部年份
            <span class="side-n">{{ normalAlbums.length }}</span>
          </button>
          <button
            v-for="b in yearBuckets"
            :key="b.year"
            class="side-item"
            :class="{ on: yearFilter === b.year }"
            @click="setYearFilter(b.year)"
          >
            {{ b.year === 0 ? "未分类" : `${b.year} 年` }}
            <span class="side-n">{{ b.count }}</span>
          </button>
        </div>
      </aside>

      <!-- 右栏：内容 -->
      <div class="album-main">
        <!-- 内容区头部：只说"当前看的是什么范围"，不重复组件名（页头已有） -->
        <div class="main-head">
          <span class="main-sub">
            {{ visibleCount }} 个相册
            <template v-if="yearFilter !== null">
              · {{ yearFilter === 0 ? "未分类" : `${yearFilter} 年` }}
            </template>
            <template v-if="scanFilter !== 'all'">
              · {{ scanFilter === "in" ? "已入库" : "未入库" }}
            </template>
            <template v-if="yearFilter !== null || scanFilter !== 'all'">
              · <button class="main-reset" @click="setYearFilter(null); setScanFilter('all')">清除筛选</button>
            </template>
          </span>
          <div class="main-actions">
      <div class="toolbar-actions">
        <!-- 非勾选模式：批量导入 / 管理 / 新建 -->
        <template v-if="!isSelectMode">
          <button v-if="sortMode !== 'date'" class="btn" @click="onToggleAll">
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

          <!-- FEAT-037：批量扫描入库（勾选模式下对选中相册组合扫描写库） -->
          <!-- 视觉重点分组：底色用主题色淡填充 + 左侧色块，作为工具条内唯一的功能入口强调 -->
          <div class="tb-group tb-scan">
            <span class="tb-group-label">扫描</span>
            <button
              class="btn btn-scan-feature"
              :disabled="selectedIds.size === 0 || batchRunning"
              :title="selectedIds.size === 0 ? '请先勾选至少 1 个相册' : `对选中的 ${selectedIds.size} 个相册执行 EXIF / 影调 / 人物 · 文档识别并写库，用于智能搜索与统计`"
              @click="openBatchScan"
            >📥 批量扫描入库…</button>
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

          </div>
        </div>

    <!-- 搜索栏：只检索相册名（FEAT-107 功能边界隔离）
         右侧常驻「不检索照片内容」+ 一个去智能搜索的出口 ——
         范围写在框里，而不是靠用户点进去试。 -->
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
        <span class="search-scope">不检索照片内容</span>
      </div>
      <button
        class="search-elsewhere"
        type="button"
        :title="searchKeyword ? `去智能搜索「${searchKeyword}」` : '去智能搜索（按内容 / 人物 / 地点找照片）'"
        @click="gotoSmartSearch"
      >
        找照片内容？去智能搜索 →
      </button>
    </div>

    <!-- 搜索结果面板：只有「相册」一组（内容检索已移到智能搜索） -->
    <div v-if="isSearchingActive" class="search-results">
      <div v-if="isSearching" class="search-status">正在搜索…</div>
      <template v-else>
        <!-- 相册匹配 -->
        <div v-if="searchResults.length" class="search-section">
          <div class="search-section-title">相册（{{ searchResults.length }}）</div>
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
        <!-- 空态：说清「只搜了名字」，并给出另一条路 -->
        <div v-else class="search-status">
          没有名称匹配的相册。
          <button class="search-status-link" type="button" @click="gotoSmartSearch">
            按照片内容在智能搜索里找 →
          </button>
        </div>
      </template>
    </div>

    <!-- FEAT-105：删掉"时间路线图"横条 —— 左栏已有年份导航（可点击筛选），
         这里再列一遍年份是重复；而且它把年份排成一行等距文字，
         与"错落"的诉求相反。年份筛选改由左栏承担。 -->

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
        点击卡片勾选 · <kbd>Esc</kbd> 退出 · <kbd>Ctrl+A</kbd> 全选当前可见 · <kbd>Del</kbd> 删除（仅删记录）
      </span>
    </div>
    <!--
      FEAT-095：归档 / 隐藏相册固定区块
      不参与时间 / 地点 / 手动排序（没有真实文件夹与拍摄时间），始终置顶展示。
    -->
    <section v-if="virtualAlbums.length" class="virtual-albums">
      <h2 class="group-head group-year">📦 系统相册</h2>
      <div class="album-grid">
        <AlbumCard
          v-for="album in virtualAlbums"
          :key="album.id"
          :album="album"
          :select-mode="false"
          :selected="false"
          @click="router.push(`/album/${album.id}`)"
          @contextmenu="onRightClick(album.id, $event)"
          @open-path="openAlbumPath(album.path, $event)"
        />
      </div>
      <p class="virtual-hint">
        「归档」与「隐藏」是照片的聚合视图：照片仍在原位置不变。
        归档可在相册中直接查看；隐藏相册需在
        <RouterLink to="/home">个人信息</RouterLink>
        中输入登录密码解锁
        <template v-if="vault.hiddenUnlocked">（当前已解锁）</template>。
      </p>
    </section>

    <!-- 日期模式：时间分组视图（年 → 季节 → 月，可折叠） -->
    <!-- FEAT-105：日期模式改为**扁平密排**（不再年→季→月三层嵌套）
         理由：左栏已有年份导航，年份再当一层大标题就是重复；
         季、月三层折叠头把一屏切成十几条横带，看起来像"表格"而不是"相册墙"。
         现在：所有相册按拍摄时间倒序平铺在一个 dense 网格里，
         时间信息移到封面左下角的落款里（AlbumCard 的 .tile-seal），
         网格用 grid-auto-rows + 行高错落制造"密排但不死板"的节奏。 -->
    <main v-if="sortMode === 'date'" class="timeline-view">
      <!-- FEAT-106：行式密排。
           每一行是一个 flex 容器（display:flex），瓦片宽度由算法算好，
           行宽精确等于容器宽度 —— 不会有空洞。
           gap 是唯一的面板缝隙，缝里露出壁纸。 -->
      <div ref="wallEl" class="album-wall" :style="{ '--wall-gap': '12px' }">
        <div v-for="(g, ri) in wallRowGroups" :key="ri" class="album-wall-row" :style="{ height: `${g.row.height}px` }">
          <div
            v-for="item in g.items"
            :key="item.album.id"
            class="album-wall-cell"
            :style="{ width: `${item.cell.width}px`, '--cover-h': `${item.cell.coverH}px` }"
          >
            <AlbumCard
              :album="item.album"
              variant="tile"
              wall
              :mosaic="item.cell.wide"
              :select-mode="isSelectMode"
              :selected="selectedIds.has(item.album.id)"
              @click="isSelectMode ? toggleSelect(item.album.id) : router.push(`/album/${item.album.id}`)"
              @contextmenu="onRightClick(item.album.id, $event)"
              @toggle-select="toggleSelect(item.album.id, $event)"
              @open-path="openAlbumPath(item.album.path, $event)"
              @open-source-path="openSourcePath($event)"
            />
          </div>
        </div>
      </div>
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
    <EmptyState
      v-if="!store.isLoading && store.albums.length === 0 && sortMode !== 'manual'"
      icon="📁"
      title="还没有相册"
      description="创建第一个相册，把硬盘里的照片收进来。"
      action-label="创建第一个相册"
      @action="openCreateDialog"
    />
      </div>
      <!-- /右栏 album-main -->
    </div>
    <!-- /双栏 album-layout -->

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
      <!-- FEAT-095：归档 / 隐藏为系统内置虚拟相册，重命名与删除入口不给（改名会让「名为归档的相册」丢失语义） -->
      <div v-if="!contextMenuIsVirtual" class="context-menu-item" @click="openRenameDialog">
        <span class="ctx-icon">✏️</span> 重命名
      </div>
      <div v-if="!contextMenuIsVirtual" class="context-menu-item context-menu-danger" @click="contextDelete">
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

    <!-- FEAT-037：批量扫描入库弹窗（扫描方式选择，默认三种全选） -->
    <div v-if="scanDialogOpen" class="dialog-mask" @click.self="scanDialogOpen = false">
      <div class="dialog dialog-scan-feature">
        <h2 class="dialog-title dialog-title-accent">📥 批量扫描入库</h2>
        <p class="batch-select-tip">
          将对选中的 <b>{{ selectedIds.size }}</b> 个相册执行内容识别并写入内容库（用于智能搜索与库存统计）。
        </p>
        <p class="batch-select-tip">选择扫描方式（可多选）：</p>
        <div class="combo-checks batch-scan-types">
          <label class="combo-check" :class="{ active: batchScanTypes.includes('basic') }">
            <input type="checkbox" value="basic" v-model="batchScanTypes" />
            <span class="combo-check-label">EXIF 基础</span>
            <span class="combo-check-desc">ISO / 焦段 / 光圈 / 快门</span>
          </label>
          <label class="combo-check" :class="{ active: batchScanTypes.includes('tone') }">
            <input type="checkbox" value="tone" v-model="batchScanTypes" />
            <span class="combo-check-label">影调分析</span>
            <span class="combo-check-desc">低调 / 中间调 / 高调</span>
          </label>
          <label class="combo-check" :class="{ active: batchScanTypes.includes('person') }">
            <input type="checkbox" value="person" v-model="batchScanTypes" />
            <span class="combo-check-label">人物 · 文档识别</span>
            <span class="combo-check-desc">人脸标号 / 夜景 / 文档（分类已改语义）</span>
          </label>
          <label class="combo-check" :class="{ active: batchScanTypes.includes('semantic') }">
            <input type="checkbox" value="semantic" v-model="batchScanTypes" />
            <span class="combo-check-label">语义向量</span>
            <span class="combo-check-desc">自然语言搜图 · 需 CLIP 模型</span>
          </label>
        </div>
        <div class="batch-scan-meta">
          <label class="batch-select">批次
            <select v-model="batchScanBatch">
              <option v-for="b in [8, 16, 32]" :key="b" :value="b">{{ b }}</option>
            </select>
          </label>
        </div>

        <!-- FEAT-037：扫描进度（串行逐个相册扫描时展示） -->
        <p v-if="batchScanning" class="batch-scan-progress">⏳ {{ batchScanProgress || "准备中…" }} · 后台扫描中，请稍候</p>

        <div class="dialog-actions">
          <button class="btn" :disabled="batchScanning" @click="scanDialogOpen = false">取消</button>
          <button class="btn btn-scan-feature" :disabled="batchScanning" @click="doBatchScan">
            {{ batchScanning ? "扫描中…" : "🚀 开始扫描" }}
          </button>
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

    <!-- FEAT-034-A：批量导入失败详情（错误分类 + 解决建议 + 重试） -->
    <ImportErrorsDialog
      :visible="showImportErrors"
      :result="lastImportResult"
      @close="showImportErrors = false"
      @retry="retryImport"
      @reimport="reimportFromRoot"
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

  <!-- FEAT-SEM：扫描方式确认（覆盖 / 增量 / 取消） -->
  <ConfirmDialog
    :visible="batchModeDialogVisible"
    :title="SCAN_MODE_TITLE"
    :message="SCAN_MODE_TIP"
    confirm-text="覆盖扫描"
    neutral-text="增量扫描"
    cancel-text="取消"
    :danger="false"
    @confirm="launchBatchScan(true)"
    @neutral="launchBatchScan(false)"
    @cancel="batchModeDialogVisible = false"
  />
</template>

<style scoped>
/* 页面壳（FEAT-105）：不再锁 max-width、不再自带左右 padding。
   旧版 max-width:1200 + padding:24 会让顶栏（52px 内边距、全宽）与内容各算各的，
   宽屏下顶栏贴着窗口、内容却居中缩窄。左右留白统一由 --sp-8 提供。

   FEAT-106：顶栏改成一块**浮起的玻璃板**后，它下面需要一段空气
   （否则板底缘与第一行内容贴在一起，板的厚度读数就没了）。 */
.album-page {
  min-height: 100vh;
  padding-bottom: 48px;
}

/* ---- 双栏：左筛选（176px 固定） + 右内容（自适应） ----
   为何固定宽度而不百分比：左栏是“分组标签 + 计数”，宽度由最长标签（“全部年份”）
   决定，与窗口无关；用百分比会让它在大屏上宽到能再放下一列，反而不像筛选器。

   FEAT-107：间距 28 → 30，左栏 188 → 176。
   左栏不再有玻璃板后，188px 的内容框看起來会比实际宽一截（四周没东西了），
   176 才是“一列文字”该有的宽度。 */
.album-layout {
  display: grid;
  grid-template-columns: 176px minmax(0, 1fr);
  gap: 30px;
  padding: 16px var(--gutter) 0;
  align-items: start;
}

/* 左栏吸顶：滚长列表时筛选始终可改，不用先滚回顶部。
   top 值要避开顶栏 + 数据带 + 间距。
   FEAT-107：旧版 168px 是按“顶栏 56 + 数据带 96 + 间距”算的，
   数据带改成文本列后变矮了（~112），不用下移这么多。 */
.album-side {
  position: sticky;
  top: 150px;
  padding-top: 2px;
}

/* ══════════════════════════════════════════════════════════════════════════
   FEAT-106：行式密排（justified rows）—— 相册墙
   ──────────────────────────────────────────────────────────────────────────
   结构：
     .album-wall        容器（只用来量宽度）
       .album-wall-row  一行（flex 容器，宽度 = 100%）
         .album-wall-cell  一格（宽度由算法算好）

   为什么不用 grid：
     grid 的 auto-fill 每行等宽，dense 回填又会留下空洞（见 utils/justifiedRows.ts
     的注释）。行式密排是 flex：瓦片宽度是 inline style（算法给的整数 px），
     整行宽度 = Σ宽 + gap×(n−1) = 容器宽度，**精确铺满**。

   行高由行容器的 height 定（inline style），格内的封面用 --cover-h 拿同一个值，
   于是整行封面严格对齐（图注已嵌入封面内部，不再占额外行高）。
   ══════════════════════════════════════════════════════════════════════════ */
.album-wall {
  display: flex;
  flex-direction: column;
  gap: var(--wall-gap, 12px);
  min-width: 0;
}

.album-wall-row {
  display: flex;
  gap: var(--wall-gap, 12px);
  align-items: stretch;
  width: 100%;
  min-width: 0;
}

.album-wall-cell {
  min-width: 0;
  flex: 0 0 auto;
}

/* 封面高度由算法给出（内联 --cover-h），行容器给出总高（= 封面高）。
   两者都在 px 上，行内所有瓦片严格对齐。 */

/* 窄窗：墙板不再分宽幅（3 列屏上宽幅瓦片会占掉大半个屏），
   改为等宽平铺 —— 由容器宽度自然决定瓦片数。 */
@media (max-width: 900px) {
  .album-wall-row {
    flex-wrap: wrap;
    height: auto !important;
  }
  .album-wall-cell {
    flex: 1 1 calc(50% - (var(--wall-gap, 12px) / 2));
    width: auto !important;
  }
  .album-wall-cell .tile-cover {
    height: auto !important;
    aspect-ratio: 3 / 2;
  }
}

/* ══════════════════════════════════════════════════════════════════════════
   FEAT-107：左栏不再是一块玻璃板
   ──────────────────────────────────────────────────────────────────────────
   旧实现用 ::before 画一整块玻璃（inset: 0 -10px -10px -12px）。
   两个问题：
     ① 左栏是板、右栏内容又是一片 —— 两栏之间一条暗缝，
        读起来是“两个抽屉并列”，不是一个页面的两栏；
     ② inset 向外扩 10-12px，左栏的实际边界比它的内容框大，
        与右栏的起始线永远对不齐（这也是这条侧栏无论怎么调都“看着别扭”的原因）。

   现在：整块 ::before 删除 —— 左栏承载的是文字列表，
   文字自己会形成一列，不需要底板。分组标题用 9.5px 大写 + .15em 字距，
   与数据带标签用**同一套字阶**（左右两处的分组标题以前是两套字号）。
   ══════════════════════════════════════════════════════════════════════════ */

.side-group + .side-group {
  margin-top: 18px;
}
.side-h {
  margin: 0 0 6px;
  /* 与数据带标签、组头读数同一套字阶（FEAT-107） */
  font-size: 9.5px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  font-weight: 600;
  /* 分组标题是“这一栏是什么”的唯一标识，属于要读的文字。
     之前用 --c-fg-3（3.15:1）不达标；现在用 --fg-2（≥5:1）。 */
  color: var(--fg-2);
}

/* 筛选项：整行可点，选中用左侧 2px 金条 + 文字提亮 ——
   不用背景块填充：一栏里 15 个带背景的格子会变成"标签云"，读不出层级。 */
.side-item {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  width: 100%;
  height: 27px;
  padding: 0 7px 0 9px;
  border: none;
  border-radius: 5px;
  background: none;
  font-size: 12.5px;
  color: var(--fg-2);
  text-align: left;
  cursor: pointer;
  transition: color var(--dur-2) var(--ease-out), background var(--dur-2) var(--ease-out);
}
.side-item:hover {
  color: var(--fg);
  background: var(--hover);
}
/* 选中 = 左侧 2px 竖条 + 文字提亮；底只给 --hover（≈5% 白，几乎不可见）。
   不用“整块高亮”：一列十几项，每项都有一块底就是十几个小卡片。 */
.side-item.on {
  color: var(--fg);
  font-weight: 600;
  background: var(--hover);
}
.side-item.on::before {
  content: "";
  position: absolute;
  left: 0;
  top: 7px;
  bottom: 7px;
  width: 2px;
  border-radius: 2px;
  background: var(--c-accent);
}
.side-item:disabled {
  color: var(--fg-3);
  cursor: not-allowed;
}
.side-n {
  font-size: 10.5px;
  /* 计数是用户判断"该组多少个"的依据 —— 要读，用 --fg-3（已提到 ≥4.5:1） */
  color: var(--fg-3);
  font-variant-numeric: tabular-nums;
}
.side-item.on .side-n {
  /* 选中项的计数跟着提亮一档（仍在弱色系，不再用金色 —— 金色留给竖条） */
  color: var(--fg-2);
}

/* 右栏内容头部：左计数 + 右操作，同一行
   （原来 toolbar 独占一行且贴窗口右边，跟内容对不齐） */
.main-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
.main-sub {
  font-size: var(--fs-meta);
  color: var(--c-fg-2);
}
.main-actions {
  margin-left: auto;
}
.main-actions > :deep(.toolbar-actions) {
  display: flex;
  align-items: center;
  gap: 8px;
}
.main-reset {
  padding: 0;
  border: none;
  background: none;
  font-size: inherit;
  color: var(--c-accent);
  text-decoration: underline;
  text-underline-offset: 2px;
  cursor: pointer;
}

/* 窄窗：左栏从固定宽度变为顶部横向条 —— 手机上竖向侧栏会把内容挤到 200px 宽。 */
@media (max-width: 900px) {
  .album-layout {
    grid-template-columns: 1fr;
    gap: 14px;
    padding: 0 var(--sp-5);
  }
  .album-side {
    position: static;
    display: flex;
    gap: 16px;
    overflow-x: auto;
    padding-bottom: 6px;
    scrollbar-width: none;
  }
  .album-side::-webkit-scrollbar {
    display: none;
  }
  .side-group + .side-group {
    margin-top: 0;
  }
  .side-group {
    flex: 0 0 auto;
  }
  .side-item {
    height: 24px;
  }
}

/* 工具栏：与页头/数据带同一侧边留白（--sp-8） */
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--sp-8);
  margin-bottom: 14px;
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
  background: color-mix(in srgb, var(--color-primary) 6%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-primary) 18%, transparent);
}
.tb-label {
  background: rgba(34, 159, 110, 0.06);
  border: 1px solid rgba(34, 159, 110, 0.18);
}
.tb-danger {
  background: color-mix(in srgb, var(--color-danger) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-danger) 22%, transparent);
}
.tb-selected-pill {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  font-size: 13px;
  font-weight: 600;
  color: var(--color-primary-text);
  background: color-mix(in srgb, var(--color-primary) 15%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-primary) 30%, transparent);
  border-radius: 999px;
}
.tb-selected-pill b {
  font-size: 14px;
  color: var(--color-primary-text);
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
  border-color: var(--color-primary);
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
  /* 标签落在页面背景条上：用背景向链接色（--color-link 浅蓝是玻璃卡语境） */
  color: var(--color-link-on-bg, var(--color-link));
  font-weight: 600;
  margin-right: 4px;
}

.bc-sep {
  color: var(--muted);
}

.bc-item {
  /* 面包屑落在页面背景上：用背景向链接色（--color-link 是玻璃卡语境的浅蓝，浅背景上失比） */
  color: var(--color-link-on-bg, var(--color-link));
}

.bc-link {
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  transition: background var(--dur-2), color var(--dur-2);
}

.bc-link:hover {
  background: var(--color-primary);
  color: var(--color-on-primary);
}

/* 搜索行（FEAT-107）：输入框 + 范围后缀 + 去智能搜索的出口。

   为什么搜索框不能只写个 placeholder：
     placeholder 在**开始输入后就消失**，恰好是用户最需要知道
     “我在搜什么范围”的那一刻。所以范围得是框内一个常驻的灰字。 */
.search-bar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}

.search-input-wrap {
  position: relative;
  display: flex;
  align-items: center;
  flex: 1 1 auto;
  max-width: 460px;
  min-width: 0;
}

.search-icon {
  position: absolute;
  left: 12px;
  font-size: 12px;
  /* 装饰图标但也要看得见：用 fg-2（≈5:1）而不是 fg-3（≈3.5:1） */
  color: var(--c-fg-2);
  pointer-events: none;
}

.search-input {
  width: 100%;
  height: 34px;
  /* 右侧要同时容纳「清除 ×」与「范围后缀」，所以留较宽的内边距 */
  padding: 0 118px 0 32px;
  border: 1px solid var(--c-rule);
  border-radius: var(--r-pill);
  font-size: var(--fs-body);
  outline: none;
  box-sizing: border-box;
  color: var(--c-fg);
  background: color-mix(in srgb, var(--c-fg) 5%, transparent);
  transition: border-color var(--dur-2) var(--ease-out), background var(--dur-2) var(--ease-out);
}

/* 范围后缀：框内右侧、常驻、不可交互 —— 它是“边界说明”而不是控件。
   用一根发丝竖线与输入区分开（否则会读成填好的内容）。

   ⚠️ 颜色用 --fg-2 而不是 --fg-3：这句“不检索照片内容”是**功能边界**，
   它存在的全部意义就是被读到；用弱色（实测浅色 3.54:1）等于白写。 */
.search-scope {
  position: absolute;
  right: 12px;
  padding-left: 11px;
  border-left: 1px solid var(--c-rule);
  font-size: 10.5px;
  letter-spacing: 0.01em;
  color: var(--c-fg-2);
  white-space: nowrap;
  pointer-events: none;
}

/* 出口：把「内容检索」明确指到智能搜索，而不是在这个框里默借实现 */
.search-elsewhere {
  flex-shrink: 0;
  padding: 0;
  border: none;
  background: none;
  font-size: 11.5px;
  color: var(--c-fg-2);
  cursor: pointer;
  white-space: nowrap;
  transition: color var(--dur-2) var(--ease-out);
}
.search-elsewhere:hover {
  color: var(--c-accent-bright, var(--c-accent));
}

/* 空态里的同行链接（不单独占行） */
.search-status-link {
  margin-left: 6px;
  padding: 0;
  border: none;
  background: none;
  font-size: inherit;
  color: var(--c-accent-bright, var(--c-accent));
  cursor: pointer;
  text-decoration: underline;
  text-underline-offset: 2px;
}

.search-input::placeholder {
  color: var(--c-fg-3);
}

.search-input:focus {
  border-color: color-mix(in srgb, var(--c-accent) 55%, transparent);
  background: color-mix(in srgb, var(--c-fg) 8%, transparent);
}

.search-clear {
  position: absolute;
  right: 92px;
  border: none;
  background: none;
  color: var(--c-fg-3);
  font-size: 16px;
  cursor: pointer;
  line-height: 1;
}

.search-clear:hover {
  color: var(--c-fg);
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
  transition: background var(--dur-2);
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
  border: 1px solid var(--color-primary);
  background: var(--tint-bg);
  color: var(--color-primary-text);
  font-size: 11px;
  padding: 2px 8px;
  border-radius: 10px;
  cursor: pointer;
  transition: all var(--dur-2);
}

.result-jump:hover {
  background: var(--color-primary);
  color: var(--color-on-primary);
}

.result-arrow {
  color: var(--muted);
  flex-shrink: 0;
}

.btn-back {
  border: none;
  background: transparent;
  color: var(--color-primary-text);
  padding: 4px 8px;
  font-size: 14px;
}

.btn-back:hover {
  background: color-mix(in srgb, var(--color-primary) 8%, transparent);
  border-color: transparent;
  color: var(--color-primary-text);
}

.page-title {
  font-size: 24px;
  margin: 0;
  /* 页面背景上的标题：on-bg 对比色（BUG-2026-0919-004） */
  color: var(--color-on-bg, var(--text));
}

/* 时间分组视图 */
.timeline-view {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.year-group {
  /* 去卡片化（方案 §三）：分组容器不再自带卡片底，靠间距 + 细线分隔 */
  margin-bottom: 4px;
}

/* 路线图跳转后的高亮动画 */
.year-highlight {
  animation: yearFlash 1.5s ease;
}

@keyframes yearFlash {
  0% {
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-primary) 40%, transparent);
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
  transition: background var(--dur-2);
}

.group-head:hover {
  background: var(--hover-bg);
}

.group-year {
  background: transparent;
  border-bottom: 1px solid var(--color-border);
  /* 标题上方留白 > 下方 */
  padding-top: 20px;
}

.group-season {
  background: transparent;
  border-bottom: 1px solid var(--color-border);
}

.group-month {
  background: transparent;
  border-bottom: 1px solid var(--color-border);
  padding: 6px 16px 6px 32px;
}

.group-title {
  font-weight: 600;
  font-size: 15px;
  /* 分组标题直接落在页面背景上：on-bg 对比色（BUG-2026-0919-004） */
  color: var(--color-on-bg, var(--text));
}

.group-count {
  font-size: 12px;
  /* FEAT-105：--muted（浅色下仅 2.96:1）→ --sub-text。
     12px 的"N 个相册"是要读的（用户靠它判断该组多大），不是装饰，
     所以不应用 muted 档。 */
  color: var(--sub-text);
  font-weight: normal;
}

.group-bc {
  font-size: 12px;
  /* FEAT-105 实测：原来用 --color-link-on-bg（浅金），在浅色预设的暗端仍只有
     4.13:1；而这个胶囊底是半透明白，叠在暗端反而更暗，两者叠加后不稳定。
     现在文字用 --c-fg-2（已按最暗端标定），胶囊只做“这是一个范围标记”的形状，
     不靠颜色区分含义。 */
  color: var(--c-fg-2);
  background: var(--slip-fill);
  box-shadow: inset 0 0 0 1px var(--c-rule);
  border-radius: 4px;
  padding: 1px 8px;
}

.fold-arrow {
  /* FEAT-105：--muted → --sub-text。折叠箭头是 13px 的可点击区域，
     它不只是装饰（用户得看到有这个开关），浅色下 muted 仅 2.96:1。 */
  color: var(--sub-text);
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

/* ===== FEAT-103：统计带与 tile 网格 ===== */

/* 数据带（FEAT-107）：它自带 padding: 18px var(--gutter) 16px + 下缘发丝线，
   所以外层只控制上下节奏。

   ⚠️ 这里**不能**写 padding: 0 —— scoped 属性选择器与组件内部规则同特异性，
   靠声明顺序取胜，写一层 padding: 0 会把组件自己的左右内边距一并抹掉，
   第一列直接贴在窗口左边缘（实测踩过）。 */
.album-stats {
  margin: 20px 0 4px;
}

/* tile 网格：比 card 网格更密（封面不需要内边距，可以排到 216px）。
   小图密排是相册浏览的正确密度：一屏多看到 3~4 张，扫视成本更低。
   auto-fill + minmax 保证窄窗自动降列，不用写断点。

   FEAT-105：gap 30/26 → 12/12，并加 grid-auto-flow: dense。
   为什么只动间距不动瓦片尺寸：一屏的信息量由"列数 × 行距"决定，
   而封面本身已经是内容，不需要再加底、加框、加影（加了就成了"贴纸墙"）。
   间距收窄后同宽度下每行能多容纳一列，视觉上就是"更密但不更乱"。
   dense 负责把被展开分组撑剩的尾位填满，避免折叠分组后留下空洞。 */
.album-grid.is-tile {
  grid-template-columns: repeat(auto-fill, minmax(208px, 1fr));
  grid-auto-flow: dense;
  gap: 12px;
}

/* FEAT-106：旧的 mosaic 网格样式已删除。
   dense 回填会造成行尾空洞，且位置不稳定 —— 改用行式密排（.album-wall）。
   保留 .album-grid.is-tile（地点/手动视图仍用普通网格）。 */
.album-grid.is-tile {
  grid-template-columns: repeat(auto-fill, minmax(208px, 1fr));
  grid-auto-flow: dense;
  gap: 12px;
}

@media (max-width: 900px) {
  .album-grid.is-tile {
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 10px;
  }
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
  transition: background var(--dur-2);
}

.context-menu-item:hover {
  background: var(--hover-bg);
}

.context-menu-danger {
  color: var(--color-danger);
}

.context-menu-danger:hover {
  background: var(--danger-bg);
}

.ctx-icon {
  font-size: 14px;
}

/* 通用按钮（FEAT-106）：与顶栏/数据带同一套“玻璃面板上的控件”语言。
   旧版是 border 1px + 实心底（card-bg）—— 在一墙磨砂玻璃面板里
   那是一块块“深色描边盒子”，看得见的只有边框。
   现在：透明底 + 发丝描边 + 悬停时才出现一点点提亮。 */
.btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 32px;
  padding: 0 13px;
  border-radius: 8px;
  border: 1px solid var(--c-rule);
  background: color-mix(in srgb, var(--c-fg) 5%, transparent);
  color: var(--c-fg-2);
  cursor: pointer;
  font-size: var(--fs-body);
  letter-spacing: var(--ls-body);
  transition: color var(--dur-2) var(--ease-out), border-color var(--dur-2) var(--ease-out),
    background var(--dur-2) var(--ease-out);
}

.btn:hover:not(:disabled) {
  color: var(--c-fg);
  border-color: color-mix(in srgb, var(--c-fg) 22%, transparent);
  background: color-mix(in srgb, var(--c-fg) 9%, transparent);
}

.btn-primary {
  /* 主按钮固定用“亮金 + 墨字”这一对 —— 不要用 --c-accent。
     --c-accent 在浅色预设下是**深棕**（#7d571a，为的是在浅底上当文字用），
     拿它做填充底色 + 墨字 = 2.71:1，实测已经被审计拓到过。
     --color-primary / --color-on-primary 是全局定好的“金底墨字”配对，两套模式都达标。 */
  background: var(--color-primary);
  color: var(--color-on-primary);
  border-color: var(--color-primary);
  font-weight: var(--fw-em);
}

.btn-primary:hover:not(:disabled) {
  background: var(--color-primary-hover);
  color: var(--color-on-primary);
  border-color: var(--color-primary-hover);
}

.btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.btn-danger {
  background: var(--color-danger);
  color: #fff;
  border-color: var(--color-danger);
}

.btn-danger:hover {
  background: var(--color-danger-hover);
  color: #fff;
}

/* FEAT-037：批量扫描入库「强调」按钮（深浅色主题适配）
   目的：让工具条扫描分组在众多 .btn 中一眼可见。
   实现：强调色边框 + 软强调底 + 阴影；**去掉左侧 3px 色条**
   （impeccable detect 报 side-tab slop：卡片侧边粗色条是 AI 生成 UI 的典型特征）。 */
.tb-group.tb-scan {
  position: relative;
  padding: 2px 10px 2px 12px;
  margin-left: 4px;
  border-radius: 10px;
  background: var(--tint-bg);
  border: 1px solid var(--tint-border);
}
.tb-group.tb-scan .tb-group-label {
  color: var(--color-primary-text);
  font-weight: 600;
}

/* 液态玻璃主按钮（全页唯一实心主按钮，方案 §三） */
.btn-scan-feature {
  background: var(--color-primary);
  color: var(--color-on-primary);
  border: 1px solid var(--color-primary);
  font-weight: 600;
  letter-spacing: 0.2px;
  box-shadow: var(--shadow-1), inset 0 1px 0 rgba(255, 255, 255, 0.18);
}
.btn-scan-feature:hover:not(:disabled) {
  background: var(--color-primary-hover);
  color: #fff;
  border-color: var(--color-primary-hover);
  box-shadow: var(--shadow-2), inset 0 1px 0 rgba(255, 255, 255, 0.22);
  transform: translateY(-1px);
}
.btn-scan-feature:active:not(:disabled) {
  transform: translateY(0);
  box-shadow: var(--shadow-1);
}
.btn-scan-feature:disabled {
  background: var(--input-border);
  color: var(--sub-text, #6b7280);
  border-color: var(--input-border);
  box-shadow: none;
  cursor: not-allowed;
  opacity: 0.7;
}

/* 弹窗标题：单点强调（去掉渐变文字——强调靠字重，不靠渐变） */
.dialog-title-accent {
  color: var(--color-link);
  font-weight: 700;
}
.dialog-scan-feature {
  border-top: 1px solid var(--color-border);
}

/* FEAT-095：归档 / 隐藏系统相册区块 */
.virtual-albums {
  margin-bottom: 28px;
  padding-bottom: 20px;
  border-bottom: 1px solid var(--color-border, rgba(128, 138, 158, 0.24));
}
/* FEAT-105：系统相册通常只有 1~2 张卡，用 auto-fill 会在右边留下一大片空白。
   这里改成定宽列（不拉伸），卡片保持封面该有的尺寸，空白留在右侧 ——
   比让卡片被拉到 700px 宽要合理（一张卡片比一栋楼还宽，认不出那是封面）。 */
.virtual-albums .album-grid {
  grid-template-columns: repeat(auto-fill, 260px);
  gap: 16px;
}
.virtual-hint {
  margin: 10px 0 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--color-link);
  opacity: 0.85;
}
.virtual-hint a {
  color: var(--color-link);
  text-decoration: underline;
  text-underline-offset: 2px;
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
  background: var(--color-primary);
  border-radius: 4px;
  transition: width var(--dur-3) ease;
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
  color: var(--color-primary-text);
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
  /* 去卡片化：地点分组头直接浮在页面背景上 */
  margin-bottom: 4px;
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
  border-color: var(--color-primary);
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
  color: var(--color-danger);
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
  color: var(--sub-text, #4a5568);
  background: var(--tint-bg, #eef3ff);
  border: 1px solid var(--tint-border, #dbe3ff);
  border-radius: 8px;
  padding: 6px 10px;
  margin: 0 0 8px;
}

/* FEAT-037：批量扫描弹窗 —— 扫描方式选择（复刻 ScanPanel 的 combo-check 样式） */
.batch-scan-types {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.combo-check {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 10px;
  border: 1px solid var(--input-border, #d0d5dd);
  border-radius: 6px;
  cursor: pointer;
  transition: all var(--dur-2);
  color: var(--text, inherit);
}
.combo-check:hover {
  border-color: var(--color-primary);
  background: color-mix(in srgb, var(--color-primary) 14%, transparent);
}
.combo-check.active {
  border-color: var(--color-primary);
  background: color-mix(in srgb, var(--color-primary) 20%, transparent);
}
.combo-check input[type="checkbox"] {
  margin-right: 2px;
}
.combo-check-label {
  font-weight: 500;
  font-size: 13px;
}
.combo-check-desc {
  font-size: 11px;
  color: var(--muted, #667085);
}
.batch-scan-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 12px;
}
.batch-scan-meta .batch-select {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--muted, #667085);
}
.batch-scan-meta .batch-select select {
  padding: 2px 4px;
  border: 1px solid var(--input-border, #d0d5dd);
  border-radius: 4px;
  font-size: 12px;
  background: var(--input-bg, #fff);
  color: var(--text, inherit);
}
.batch-scan-progress {
  font-size: 12.5px;
  color: var(--color-primary-ink);
  background: #eef3fb;
  border: 1px solid #dbe3ff;
  border-radius: 8px;
  padding: 8px 10px;
  margin: 0 0 12px;
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
  transition: background var(--dur-2);
}
.folder-item:hover {
  background: rgba(120, 120, 140, 0.1);
}
.folder-item.active {
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
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
  transition: all var(--dur-2);
}
.bmode.active {
  background: color-mix(in srgb, var(--color-primary) 12%, transparent);
  border-color: color-mix(in srgb, var(--color-primary) 50%, transparent);
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
  color: var(--color-primary-text);
  cursor: pointer;
  transition: opacity var(--dur-3) ease, transform var(--dur-3) ease, background var(--dur-2);
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

/* 窗口较窄时（桌面窗口可自由缩放）：工具栏与卡片网格降级排布，避免按钮溢出 */
@media (max-width: 900px) {
  .toolbar {
    flex-direction: column;
    align-items: stretch;
    gap: 12px;
  }

  .toolbar-left {
    /* 去掉重复面包屑后只剩「返回 + 标题」两项，space-between 会把标题推到最右，
       与返回按钮断开。改为左对齐并允许换行，标题紧跟返回按钮。 */
    justify-content: flex-start;
    flex-wrap: wrap;
    gap: 12px;
  }

  .toolbar-actions {
    justify-content: flex-start;
    flex-wrap: wrap;
  }

  /* 批量管理模式：分组容器换行铺满，分隔线改为横向更直观 */
  .tb-group {
    flex-wrap: wrap;
  }

  .tb-divider {
    width: auto;
    height: 1px;
    flex: 1 1 100%;
    background: var(--card-border);
  }

  .album-grid {
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 14px;
  }
}

@media (max-width: 640px) {
  .album-page {
    padding: 16px;
  }

  .page-title {
    font-size: 20px;
  }

  .album-grid {
    grid-template-columns: 1fr;
  }

  .dialog {
    width: 100%;
    padding: 20px;
  }
}
</style>

