<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useRouter } from "vue-router";
import { useAlbumStore } from "../stores/album";
import type { Album } from "../types/album";
import type { Folder, ManualTree } from "../types/folder";
import { trace } from "../utils/trace";
import AlbumMiniCard from "../components/AlbumMiniCard.vue";
import ConfirmDialog from "../components/ConfirmDialog.vue";
import { useNotify } from "../composables/useNotify";

const props = defineProps<{
  /** 从搜索结果传入：需要跳转到的分组 id */
  jumpFolderId?: number | null;
  /** 是否处于勾选管理模式（由 AlbumList 的'管理'按钮控制，两种排序视图复用） */
  selectMode?: boolean;
  /** 已勾选的相册 ID 集合（AlbumList 层维护） */
  selectedIds?: Set<number>;
}>();

const emit = defineEmits<{
  /** 管理模式点击卡片：通知 AlbumList 切换勾选 */
  (e: "toggle-select", id: number): void;
}>();

const router = useRouter();
const store = useAlbumStore();
const notify = useNotify();

// ---------- 状态 ----------
const tree = ref<ManualTree | null>(null);
const loading = ref(false);

// 新建分组
const showCreateFolder = ref(false);
const createName = ref("");
const createParentId = ref<number | null>(null);

// 编辑分组
const editingFolder = ref<Folder | null>(null);
const editName = ref("");
const editDescription = ref("");
const editTags = ref<string[]>([]);
const tagInput = ref("");

// 右键菜单
const contextMenu = ref<{ visible: boolean; x: number; y: number; target: "folder" | "album"; id: number; showMoveList: boolean }>({
  visible: false, x: 0, y: 0, target: "album", id: 0, showMoveList: false,
});

// ---------- 管理模式（复用 AlbumList 的'管理'按钮，仅负责渲染勾选状态） ----------
/** 单删（右键）确认弹窗 */
const oneDeleteConfirm = ref<{ visible: boolean; message: string; id: number; kind: "album" | "folder" }>({
  visible: false, message: "", id: 0, kind: "album",
});

/** 卡片点击：管理模式由 AlbumList 勾选（emit），普通模式跳转详情 */
function onMiniCardClick(id: number) {
  if (props.selectMode) {
    emit("toggle-select", id);
  } else {
    router.push(`/album/${id}`);
  }
}

/** 相册列表变化（批量删除等）后刷新分组树 */
watch(
  () => store.albums.length,
  () => {
    loadTree();
  },
);

/** 加载手动树 */
async function loadTree() {
  loading.value = true;
  try {
    tree.value = await invoke<ManualTree>("get_manual_tree");
  } finally {
    loading.value = false;
  }
}

// ---------- 树构建 ----------
interface TreeNode {
  folder: Folder;
  children: TreeNode[];
  albumIds: number[];
}

/** 构建文件夹树（最多三级） */
const rootNodes = computed<TreeNode[]>(() => {
  if (!tree.value) return [];
  const nodes = new Map<number, TreeNode>();
  for (const f of tree.value.folders) {
    nodes.set(f.id, { folder: f, children: [], albumIds: [] });
  }
  for (const fa of tree.value.folder_albums) {
    const node = nodes.get(fa.folder_id);
    if (node) node.albumIds = fa.album_ids;
  }
  const roots: TreeNode[] = [];
  for (const node of nodes.values()) {
    if (node.folder.parent_id === null) {
      roots.push(node);
    } else {
      const parent = nodes.get(node.folder.parent_id);
      if (parent) parent.children.push(node);
    }
  }
  // 排序
  const sortNodes = (arr: TreeNode[]) => arr.sort((a, b) => a.folder.sort_order - b.folder.sort_order);
  for (const node of nodes.values()) sortNodes(node.children);
  return sortNodes(roots);
});

/** 顶级游离相册（不属于任何文件夹） */
const rootAlbums = computed(() => tree.value?.root_albums ?? []);

/** 相册 by id（预构建 Map，避免模板内 O(N²) 线性查找） */
const albumMap = computed(() => new Map(store.albums.map((a) => [a.id, a])));
function albumById(id: number): Album | null {
  return albumMap.value.get(id) ?? null;
}

/** 将本地文件路径转为前端可访问的 URL（Tauri asset 协议，拖拽浮层用） */
function fileUrl(path: string | null): string {
  return path ? convertFileSrc(path) : "";
}

// ---------- 折叠功能（配合父组件"全部折叠/展开"按钮） ----------
/** 折叠的分组 id 集合 */
const collapsed = ref<Set<number>>(new Set());
/** 顶级"未分组相册"区域是否折叠 */
const rootCollapsed = ref(false);

/** 是否折叠某分组 */
function isFolderCollapsed(id: number): boolean {
  return collapsed.value.has(id);
}

/** 切换分组折叠 */
function toggleFolder(id: number) {
  const next = new Set(collapsed.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  collapsed.value = next;
}

/** 递归收集所有分组 id */
function collectFolderIds(nodes: TreeNode[]): number[] {
  const ids: number[] = [];
  for (const n of nodes) {
    ids.push(n.folder.id);
    ids.push(...collectFolderIds(n.children));
  }
  return ids;
}

/** 全部折叠 / 全部展开（由父组件按钮触发） */
function toggleAll() {
  if (collapsed.value.size > 0) {
    collapsed.value = new Set();
    rootCollapsed.value = false;
  } else {
    collapsed.value = new Set(collectFolderIds(rootNodes.value));
    rootCollapsed.value = true;
  }
}

/** 当前是否处于"全部折叠"状态（供父组件按钮显示文字） */
const isAllCollapsed = computed(() => collapsed.value.size > 0);

defineExpose({ toggleAll, isAllCollapsed });

// ---------- 创建分组 ----------
function openCreateFolder(parentId: number | null) {
  createParentId.value = parentId;
  createName.value = "";
  showCreateFolder.value = true;
}

async function submitCreateFolder() {
  if (!createName.value.trim()) return;
  try {
    await invoke("create_folder", { name: createName.value, parentId: createParentId.value });
    showCreateFolder.value = false;
    await loadTree();
    notify.success("分组创建成功");
  } catch (e) {
    notify.error("创建分组失败", String(e));
  }
}

// ---------- 编辑分组（标签/说明） ----------
function openEditFolder(id: number) {
  const folder = findFolder(id);
  if (!folder) return;
  editingFolder.value = folder;
  editName.value = folder.name;
  editDescription.value = folder.description ?? "";
  editTags.value = [...folder.tags];
  tagInput.value = "";
}

/** 通过 id 查找文件夹（遍历树） */
function findFolder(id: number): Folder | null {
  if (!tree.value) return null;
  return tree.value.folders.find((f) => f.id === id) ?? null;
}

function addTag() {
  const t = tagInput.value.trim();
  if (!t) return;
  if (editTags.value.length >= 5) {
    notify.warning("最多只能添加 5 个标签");
    return;
  }
  if (!editTags.value.includes(t)) {
    editTags.value.push(t);
  }
  tagInput.value = "";
}

function removeTag(index: number) {
  editTags.value.splice(index, 1);
}

async function saveEditFolder() {
  if (!editingFolder.value) return;
  try {
    await invoke("update_folder", {
      id: editingFolder.value.id,
      name: editName.value.trim(),
      description: editDescription.value,
      tags: editTags.value,
    });
    editingFolder.value = null;
    await loadTree();
    notify.success("分组保存成功");
  } catch (e) {
    notify.error("保存失败", String(e));
  }
}

// ---------- 删除分组 ----------
async function deleteFolder(id: number) {
  oneDeleteConfirm.value = {
    visible: true,
    message: "确定删除该分组吗？其下的相册会移到顶级，子分组会升级。",
    id,
    kind: "folder",
  };
}
/** 确认后真正删除分组 */
async function doDeleteFolder() {
  const { id } = oneDeleteConfirm.value;
  oneDeleteConfirm.value.visible = false;
  try {
    await invoke("delete_folder", { id });
    contextMenu.value.visible = false;
    await loadTree();
  } catch (e) {
    notify.error("删除失败", String(e));
  }
}

// ---------- 删除相册（手动排序右键） ----------
async function deleteAlbumContext(albumId: number) {
  const album = store.albums.find((a) => a.id === albumId);
  oneDeleteConfirm.value = {
    visible: true,
    message: `确定要删除相册「${album?.name ?? ""}」吗？\n\n此操作仅删除系统中的相册视图，不会删除本地照片文件。`,
    id: albumId,
    kind: "album",
  };
}
/** 确认后真正删除相册 */
async function doDeleteAlbumContext() {
  const { id } = oneDeleteConfirm.value;
  oneDeleteConfirm.value.visible = false;
  try {
    await store.deleteAlbum(id);
    closeContextMenu();
    await loadTree();
  } catch (e) {
    notify.error("删除失败", String(e));
  }
}

// ---------- 目录导航：跳转到分组 ----------
function jumpToFolder(folderId: number) {
  const el = document.getElementById(`folder-node-${folderId}`);
  if (el) {
    el.scrollIntoView({ behavior: "smooth", block: "start" });
    el.classList.add("folder-highlight");
    setTimeout(() => el.classList.remove("folder-highlight"), 1500);
  }
}

// ---------- 移动相册到分组 ----------
const moveAlbum = trace("moveAlbum", async (albumId: number, folderId: number | null) => {
  try {
    await invoke("move_album", { albumId, folderId });
    // 同步更新 store 中该相册的 folder_id，保证详情页等视图一致
    const album = store.albums.find((a) => a.id === albumId);
    if (album) {
      album.folder_id = folderId;
    }
    // 同步更新当前打开的相册详情（如果正在看该相册）
    if (store.currentAlbum?.id === albumId) {
      store.currentAlbum.folder_id = folderId;
    }
    await loadTree();
  } catch (e) {
    notify.error("移动失败", String(e));
  }
});

// ---------- 拖拽（Pointer Events，兼容 WebView2） ----------
// 说明：WebView2 对原生 HTML5 drag/drop 支持不可靠，改用 Pointer Events 实现，
// 通过指针位置判断落点（悬停的分组），drop 后调用 moveAlbum / reorderAlbum。

const draggingAlbumId = ref<number | null>(null);
const dragOverFolder = ref<number | null>(null);
const dragX = ref(0);
const dragY = ref(0);
/** 是否正在拖拽（指针移动超过阈值才为 true，防止点击触发拖拽） */
const isDragging = ref(false);
/** 拖拽开始时相册所在分组（用于判断是否跨组） */
const dragSourceFolder = ref<number | null>(null);
/** 同组拖拽时目标插入下标（落点所在相册的位置） */
const dragInsertIndex = ref<number | null>(null);
/** pointerdown 时的起始位置（用于判断是否只是点击） */
const dragStartX = ref(0);
const dragStartY = ref(0);
/** 指针移动超过多少像素才算真正拖拽 */
const DRAG_THRESHOLD = 5;

/** 相册 pointerdown：准备拖拽（不立即开始，等移动超过阈值） */
function onAlbumPointerDown(albumId: number, sourceFolder: number | null, event: PointerEvent) {
  event.preventDefault();
  // 只响应左键
  if (event.button !== 0) return;

  console.log("[DRAG] pointerdown: albumId=", albumId, "sourceFolder=", sourceFolder);
  draggingAlbumId.value = albumId;
  dragSourceFolder.value = sourceFolder;
  // 不立即设 isDragging=true，等移动超过阈值
  isDragging.value = false;
  dragX.value = event.clientX;
  dragY.value = event.clientY;
  dragStartX.value = event.clientX;
  dragStartY.value = event.clientY;
  dragOverFolder.value = null;
  dragInsertIndex.value = null;

  // 全局监听移动和释放
  window.addEventListener("pointermove", onGlobalPointerMove);
  window.addEventListener("pointerup", onGlobalPointerUp);
}

/** 全局 pointermove：更新浮层位置 + 判断悬停分组和组内位置 */
function onGlobalPointerMove(event: PointerEvent) {
  if (draggingAlbumId.value == null) return;
  dragX.value = event.clientX;
  dragY.value = event.clientY;

  // 检查是否移动超过阈值，才开始真正的拖拽
  const dx = event.clientX - dragStartX.value;
  const dy = event.clientY - dragStartY.value;
  if (!isDragging.value) {
    if (Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) {
      return; // 还没超过阈值，不开始拖拽
    }
    isDragging.value = true;
    console.log("[DRAG] drag started after threshold: albumId=", draggingAlbumId.value);
  }

  // 用 elementFromPoint 判断指针下的分组
  const el = document.elementFromPoint(event.clientX, event.clientY) as HTMLElement | null;
  const folderEl = el?.closest("[data-folder-id]") as HTMLElement | null;
  const albumEl = el?.closest("[data-album-index]") as HTMLElement | null;

  // 判断当前悬停分组
  if (folderEl && folderEl.dataset.folderId !== "") {
    dragOverFolder.value = Number(folderEl.dataset.folderId);
  } else {
    const rootEl = el?.closest("[data-root-drop]");
    dragOverFolder.value = rootEl ? null : dragOverFolder.value;
  }

  // 计算组内插入位置（相册上/分组内）
  if (albumEl) {
    const index = Number(albumEl.dataset.albumIndex);
    const rect = albumEl.getBoundingClientRect();
    // 指针在相册中心线之后（横向卡片，以中线判断）→ 插入到该相册之后
    dragInsertIndex.value = event.clientX > rect.left + rect.width / 2 ? index + 1 : index;
  } else {
    // 指针不在相册上：不改变位置
    dragInsertIndex.value = null;
  }
}

/** 全局 pointerup：完成拖拽或清理 */
async function onGlobalPointerUp(event: PointerEvent) {
  // 先清理监听和状态（无论是否拖拽，都要清理，防止状态残留）
  window.removeEventListener("pointermove", onGlobalPointerMove);
  window.removeEventListener("pointerup", onGlobalPointerUp);

  const albumId = draggingAlbumId.value;
  const targetFolder = dragOverFolder.value;
  const insertIndex = dragInsertIndex.value;
  const sourceFolder = dragSourceFolder.value;
  const wasDragging = isDragging.value;

  // 重置所有状态
  isDragging.value = false;
  dragOverFolder.value = null;
  dragInsertIndex.value = null;
  draggingAlbumId.value = null;
  dragSourceFolder.value = null;

  console.log("[DRAG] pointerup: albumId=", albumId, "targetFolder=", targetFolder,
    "sourceFolder=", sourceFolder, "insertIndex=", insertIndex, "wasDragging=", wasDragging);

  // 只有真正拖拽了才执行移动操作（防止点击触发的误移动）
  if (!wasDragging || albumId == null) {
    console.log("[DRAG] pointerup IGNORED (not a real drag): wasDragging=", wasDragging);
    return;
  }

  event.preventDefault();

  if (targetFolder !== sourceFolder) {
    // 跨组移动：直接归入目标分组末尾
    await moveAlbum(albumId, targetFolder);
  } else if (insertIndex != null) {
    // 同组排序：移动到目标下标
    try {
      await invoke("reorder_album", { albumId, folderId: targetFolder, newIndex: insertIndex });
      // 同步 folder_id
      const album = store.albums.find((a) => a.id === albumId);
      if (album) {
        album.folder_id = targetFolder;
      }
      if (store.currentAlbum?.id === albumId) {
        store.currentAlbum.folder_id = targetFolder;
      }
      await loadTree();
    } catch (e) {
      notify.error("排序失败", String(e));
    }
  }
}

/** 拖拽结束清理（组件卸载时调用） */
function cleanupDrag() {
  window.removeEventListener("pointermove", onGlobalPointerMove);
  window.removeEventListener("pointerup", onGlobalPointerUp);
  isDragging.value = false;
  draggingAlbumId.value = null;
  dragSourceFolder.value = null;
  dragOverFolder.value = null;
  dragInsertIndex.value = null;
}

// ---------- 右键菜单 ----------
function onRightClick(target: "folder" | "album", id: number, event: MouseEvent) {
  event.preventDefault();
  event.stopPropagation();
  contextMenu.value = { visible: true, x: event.clientX, y: event.clientY, target, id, showMoveList: false };
}

function closeContextMenu() {
  contextMenu.value.visible = false;
}

/** 所有分组（用于右键移动到分组） */
const allFolders = computed(() => tree.value?.folders ?? []);

/** 显示移动到分组的选项列表 */
function showMoveList() {
  contextMenu.value.showMoveList = true;
}

/** 从右键菜单移动相册到指定分组 */
function contextMoveTo(folderId: number | null) {
  const albumId = contextMenu.value.id;
  closeContextMenu();
  moveAlbum(albumId, folderId);
}

function onGlobalClick() {
  closeContextMenu();
}

onMounted(() => {
  loadTree();
  window.addEventListener("click", onGlobalClick);
});
// 监听外部传入的跳转分组 id（搜索结果点击分组路径）
watch(
  () => props.jumpFolderId,
  (val) => {
    if (val != null) {
      // 等待树加载完成后再滚动（组件可能刚挂载，树未加载）
      setTimeout(() => {
        jumpToFolder(val);
      }, 150);
    }
  },
  { immediate: true },
);
onBeforeUnmount(() => {
  window.removeEventListener("click", onGlobalClick);
  cleanupDrag();
});
</script>

<template>
  <div class="manual-sort">
    <!-- 顶部工具栏 -->
    <div class="manual-toolbar">
      <button class="btn btn-primary" @click="openCreateFolder(null)">新建分组</button>
      <span class="manual-hint">拖拽相册到分组中即可归类，最多支持三级</span>
    </div>

    <!-- 目录导航（类似日期排序的路线图，点击跳转分组） -->
    <div class="manual-nav">
      <span class="manual-nav-label">目录</span>
      <span v-if="rootNodes.length === 0" class="manual-nav-empty">暂无分组</span>
      <template v-for="node in rootNodes" :key="node.folder.id">
        <span class="manual-nav-sep">›</span>
        <span class="manual-nav-item" :title="`跳转到分组 ${node.folder.name}`" @click="jumpToFolder(node.folder.id)">
          {{ node.folder.name }}
        </span>
      </template>
    </div>

    <!-- 顶级游离相册（可拖入分组） -->
    <div class="root-albums" data-root-drop>
      <div class="root-title" @click="rootCollapsed = !rootCollapsed">
        <span class="fold-arrow">{{ rootCollapsed ? "▸" : "▾" }}</span>
        <span>未分组相册</span>
      </div>
      <div v-show="!rootCollapsed">
        <div v-if="rootAlbums.length === 0" class="empty-hint">暂无未分组相册</div>
        <div class="album-mini-row">
        <AlbumMiniCard
          v-for="(entry, idx) in rootAlbums"
          :key="entry.album_id"
          :album-id="entry.album_id"
          :folder-id="null"
          :index="idx"
          :album="albumById(entry.album_id)"
          :dragging="isDragging && draggingAlbumId === entry.album_id"
          :select-mode="props.selectMode"
          :selected="props.selectedIds?.has(entry.album_id)"
          @pointerdown="onAlbumPointerDown(entry.album_id, null, $event)"
          @click="onMiniCardClick(entry.album_id)"
          @contextmenu="onRightClick('album', entry.album_id, $event)"
        />
        </div>
      </div>
    </div>

    <!-- 文件夹树（递归渲染三级） -->
    <div class="folder-tree">
      <template v-for="node in rootNodes" :key="node.folder.id">
        <!-- 顶级分组 -->
        <div
          :id="`folder-node-${node.folder.id}`"
          class="folder-node level-1"
          :data-folder-id="node.folder.id"
          :class="{ 'drag-over': dragOverFolder === node.folder.id }"
        >
          <div class="folder-head" @contextmenu="onRightClick('folder', node.folder.id, $event)">
            <span class="fold-arrow" @click.stop="toggleFolder(node.folder.id)">{{ isFolderCollapsed(node.folder.id) ? "▸" : "▾" }}</span>
            <span class="folder-icon">📁</span>
            <span class="folder-name">{{ node.folder.name }}</span>
            <span class="folder-tags">
              <span v-for="t in node.folder.tags" :key="t" class="tag-chip">{{ t }}</span>
            </span>
            <span class="folder-actions">
              <button class="mini-btn" @click.stop="openCreateFolder(node.folder.id)">+子分组</button>
              <button class="mini-btn" @click.stop="openEditFolder(node.folder.id)">编辑</button>
            </span>
          </div>

          <!-- 组内相册 -->
          <div v-show="!isFolderCollapsed(node.folder.id)" class="folder-albums">
            <AlbumMiniCard
              v-for="(aid, idx) in node.albumIds"
              :key="aid"
              :album-id="aid"
              :folder-id="node.folder.id"
              :index="idx"
              :album="albumById(aid)"
              :dragging="isDragging && draggingAlbumId === aid"
              :select-mode="props.selectMode"
              :selected="props.selectedIds?.has(aid)"
              @pointerdown="onAlbumPointerDown(aid, node.folder.id, $event)"
              @click="onMiniCardClick(aid)"
              @contextmenu="onRightClick('album', aid, $event)"
            />
          </div>

          <!-- 子分组（二级） -->
          <div v-for="child in node.children" :key="child.folder.id"
               v-show="!isFolderCollapsed(node.folder.id)"
               :id="`folder-node-${child.folder.id}`" class="folder-node level-2"
               :data-folder-id="child.folder.id"
               :class="{ 'drag-over': dragOverFolder === child.folder.id }">
            <div class="folder-head" @contextmenu="onRightClick('folder', child.folder.id, $event)">
              <span class="fold-arrow" @click.stop="toggleFolder(child.folder.id)">{{ isFolderCollapsed(child.folder.id) ? "▸" : "▾" }}</span>
              <span class="folder-icon">📂</span>
              <span class="folder-name">{{ child.folder.name }}</span>
              <span class="folder-tags">
                <span v-for="t in child.folder.tags" :key="t" class="tag-chip">{{ t }}</span>
              </span>
              <span class="folder-actions">
                <button class="mini-btn" @click.stop="openCreateFolder(child.folder.id)">+子分组</button>
                <button class="mini-btn" @click.stop="openEditFolder(child.folder.id)">编辑</button>
              </span>
            </div>
            <div v-show="!isFolderCollapsed(child.folder.id)" class="folder-albums">
              <AlbumMiniCard
                v-for="(aid, idx) in child.albumIds"
                :key="aid"
                :album-id="aid"
                :folder-id="child.folder.id"
                :index="idx"
                :album="albumById(aid)"
                :dragging="isDragging && draggingAlbumId === aid"
                :select-mode="props.selectMode"
                :selected="props.selectedIds?.has(aid)"
                @pointerdown="onAlbumPointerDown(aid, child.folder.id, $event)"
                @click="onMiniCardClick(aid)"
                @contextmenu="onRightClick('album', aid, $event)"
              />
            </div>
            <!-- 三级分组 -->
            <div v-for="grand in child.children" :key="grand.folder.id"
                 v-show="!isFolderCollapsed(child.folder.id)"
                 :id="`folder-node-${grand.folder.id}`" class="folder-node level-3"
                 :data-folder-id="grand.folder.id"
                 :class="{ 'drag-over': dragOverFolder === grand.folder.id }">
              <div class="folder-head" @contextmenu="onRightClick('folder', grand.folder.id, $event)">
                <span class="fold-arrow" @click.stop="toggleFolder(grand.folder.id)">{{ isFolderCollapsed(grand.folder.id) ? "▸" : "▾" }}</span>
                <span class="folder-icon">📂</span>
                <span class="folder-name">{{ grand.folder.name }}</span>
                <span class="folder-tags">
                  <span v-for="t in grand.folder.tags" :key="t" class="tag-chip">{{ t }}</span>
                </span>
                <span class="folder-actions">
                  <button class="mini-btn" @click.stop="openEditFolder(grand.folder.id)">编辑</button>
                </span>
              </div>
              <div v-show="!isFolderCollapsed(grand.folder.id)" class="folder-albums">
                <AlbumMiniCard
                  v-for="(aid, idx) in grand.albumIds"
                  :key="aid"
                  :album-id="aid"
                  :folder-id="grand.folder.id"
                  :index="idx"
                  :album="albumById(aid)"
                  :dragging="isDragging && draggingAlbumId === aid"
                  :select-mode="props.selectMode"
                  :selected="props.selectedIds?.has(aid)"
                  @pointerdown="onAlbumPointerDown(aid, grand.folder.id, $event)"
                  @click="onMiniCardClick(aid)"
                  @contextmenu="onRightClick('album', aid, $event)"
                />
              </div>
            </div>
          </div>
        </div>
      </template>
    </div>

    <!-- 右键菜单 -->
    <div v-if="contextMenu.visible" class="context-menu"
         :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }" @click.stop>
      <template v-if="contextMenu.target === 'folder'">
        <div class="context-menu-item" @click="deleteFolder(contextMenu.id)">删除分组</div>
      </template>
      <template v-else>
        <div class="context-menu-item" @click="showMoveList">移动到分组 ▸</div>
        <!-- 分组列表（点击显示） -->
        <template v-if="contextMenu.showMoveList">
          <div class="ctx-submenu">
            <div class="context-menu-item ctx-sub" @click="contextMoveTo(null)">移出到顶级</div>
            <div v-for="f in allFolders" :key="f.id" class="context-menu-item ctx-sub" @click="contextMoveTo(f.id)">
              {{ "　".repeat(f.level - 1) }}📁 {{ f.name }}
            </div>
          </div>
        </template>
        <div class="context-menu-item context-menu-danger" @click="deleteAlbumContext(contextMenu.id)">🗑️ 删除相册</div>
      </template>
    </div>

    <!-- 拖拽浮层（跟随鼠标显示被拖相册） -->
    <div v-if="isDragging && draggingAlbumId != null" class="drag-ghost"
         :style="{ left: `${dragX + 10}px`, top: `${dragY + 10}px` }">
      <img v-if="albumById(draggingAlbumId)?.cover_path" :src="fileUrl(albumById(draggingAlbumId)!.cover_path)" class="ghost-cover" />
      <div v-else class="ghost-cover ghost-placeholder">📷</div>
      <span class="ghost-name">{{ albumById(draggingAlbumId)?.name }}</span>
    </div>

    <!-- 新建分组对话框 -->
    <div v-if="showCreateFolder" class="dialog-mask" @click.self="showCreateFolder = false">
      <div class="dialog">
        <h3>新建分组</h3>
        <input v-model="createName" class="input" placeholder="分组名称" maxlength="50" />
        <div class="dialog-actions">
          <button class="btn" @click="showCreateFolder = false">取消</button>
          <button class="btn btn-primary" @click="submitCreateFolder">创建</button>
        </div>
      </div>
    </div>

    <!-- 编辑分组对话框（名称/说明/标签≤5） -->
    <div v-if="editingFolder" class="dialog-mask" @click.self="editingFolder = null">
      <div class="dialog">
        <h3>编辑分组</h3>
        <label class="form-label">名称</label>
        <input v-model="editName" class="input" maxlength="50" />
        <label class="form-label">说明</label>
        <textarea v-model="editDescription" class="textarea" maxlength="200"></textarea>
        <label class="form-label">标签（最多 5 个）</label>
        <div class="tag-edit">
          <input v-model="tagInput" class="input" placeholder="输入标签后回车/点添加" maxlength="20"
                 @keyup.enter="addTag" />
          <button class="btn btn-sm" @click="addTag">添加</button>
        </div>
        <div class="tag-list">
          <span v-for="(t, i) in editTags" :key="i" class="tag-chip editable">
            {{ t }} <button class="tag-del" @click="removeTag(i)">×</button>
          </span>
        </div>
        <div class="dialog-actions">
          <button class="btn" @click="editingFolder = null">取消</button>
          <button class="btn btn-primary" @click="saveEditFolder">保存</button>
        </div>
      </div>
    </div>

    <!-- 删除分组确认 -->
    <ConfirmDialog
      :visible="oneDeleteConfirm.visible && oneDeleteConfirm.kind === 'folder'"
      title="删除分组"
      :message="oneDeleteConfirm.message"
      @confirm="doDeleteFolder"
      @cancel="oneDeleteConfirm.visible = false"
    />

    <!-- 删除相册确认（右键） -->
    <ConfirmDialog
      :visible="oneDeleteConfirm.visible && oneDeleteConfirm.kind === 'album'"
      title="删除相册"
      :message="oneDeleteConfirm.message"
      @confirm="doDeleteAlbumContext"
      @cancel="oneDeleteConfirm.visible = false"
    />
  </div>
</template>

<style scoped>
.manual-sort { padding-bottom: 30px; }
.manual-toolbar { display: flex; align-items: center; gap: 12px; margin-bottom: 16px; }
.manual-hint { color: #6b7280; font-size: 13px; }

/* 目录导航 */
.manual-nav {
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

.manual-nav-label {
  color: #396cd8;
  font-weight: 600;
  margin-right: 4px;
}

.manual-nav-empty {
  color: #6b7280;
}

.manual-nav-sep {
  color: #bbb;
}

.manual-nav-item {
  color: #396cd8;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  transition: background 0.2s, color 0.2s;
}

.manual-nav-item:hover {
  background: #396cd8;
  color: #fff;
}

/* 目录跳转后的分组高亮 */
.folder-highlight {
  animation: folderFlash 1.5s ease;
}

@keyframes folderFlash {
  0% {
    box-shadow: 0 0 0 3px rgba(57, 108, 216, 0.4);
    background: #f0f5ff;
  }
  100% {
    box-shadow: none;
    background: #fff;
  }
}

.btn { padding: 8px 16px; border-radius: 8px; border: 1px solid #ddd; background: #fff; cursor: pointer; font-size: 14px; }
.btn-primary { background: #396cd8; color: #fff; border-color: #396cd8; }
.btn-sm { padding: 5px 10px; font-size: 12px; }
.btn:disabled { opacity: .6; }

.root-albums { border: 1px dashed #ccc; border-radius: 10px; padding: 12px; margin-bottom: 20px; background: #fafbfd; }
.root-title { font-size: 13px; color: #5f6b7a; margin-bottom: 8px; cursor: pointer; user-select: none; display: flex; align-items: center; gap: 6px; }

/* 折叠箭头 */
.fold-arrow {
  color: #6b7280;
  font-size: 12px;
  width: 14px;
  text-align: center;
  cursor: pointer;
  flex-shrink: 0;
  user-select: none;
}
.empty-hint { color: #bbb; font-size: 13px; }
.album-mini-row { display: flex; flex-wrap: wrap; gap: 10px; }

/* 拖拽浮层 */
.drag-ghost {
  position: fixed;
  z-index: 9999;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px 6px 6px;
  background: #fff;
  border: 1px solid #396cd8;
  border-radius: 8px;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.2);
  pointer-events: none;
}

.ghost-cover {
  width: 40px;
  height: 28px;
  object-fit: cover;
  border-radius: 4px;
}

.ghost-placeholder {
  background: #f0f0f0;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
}

.ghost-name {
  font-size: 13px;
  color: #2c3e50;
  max-width: 140px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.folder-tree { display: flex; flex-direction: column; gap: 12px; }
.folder-node { border: 1px solid #eef0f4; border-radius: 10px; background: #fff; }
.folder-node.drag-over { border-color: #396cd8; background: #f0f5ff; box-shadow: 0 0 0 2px rgba(57,108,216,.2); }
.folder-head { display: flex; align-items: center; gap: 8px; padding: 10px 14px; cursor: pointer; border-bottom: 1px solid #f5f5f5; }
.folder-icon { font-size: 16px; }
.folder-name { font-weight: 600; font-size: 14px; color: #2c3e50; }
.folder-tags { display: flex; gap: 4px; }
.tag-chip { background: #eef3ff; color: #396cd8; font-size: 11px; padding: 1px 8px; border-radius: 10px; }
.tag-chip.editable { display: inline-flex; align-items: center; gap: 4px; }
.tag-del { border: none; background: none; color: #9a6a00; cursor: pointer; font-size: 12px; }
.folder-actions { margin-left: auto; display: flex; gap: 4px; }
.mini-btn { border: 1px solid #ddd; background: #fff; border-radius: 6px; font-size: 11px; padding: 3px 8px; cursor: pointer; }
.mini-btn:hover { border-color: #396cd8; color: #396cd8; }
.folder-albums { display: flex; flex-wrap: wrap; gap: 10px; padding: 10px 14px; }

.level-2 { margin-left: 24px; margin-bottom: 8px; background: #fafbfd; }
.level-3 { margin-left: 24px; margin-bottom: 8px; background: #f7f9fc; }

.context-menu { position: fixed; z-index: 200; min-width: 140px; max-height: 300px; overflow-y: auto; background: #fff; border: 1px solid #eee; border-radius: 8px; box-shadow: 0 6px 20px rgba(0,0,0,.15); padding: 6px; }
.context-menu-item { padding: 8px 12px; border-radius: 6px; font-size: 14px; cursor: pointer; }
.context-menu-item:hover { background: #f0f4ff; }
.ctx-submenu { border-top: 1px solid #f0f0f0; margin-top: 4px; padding-top: 4px; }
.ctx-sub { font-size: 13px; color: #555; }
.context-menu-danger { color: #e5484d; }
.context-menu-danger:hover { background: #fdf0f0; }

.dialog-mask { position: fixed; inset: 0; background: rgba(0,0,0,.4); display: flex; align-items: center; justify-content: center; z-index: 100; }
.dialog { width: 420px; max-width: 90vw; background: #fff; border-radius: 12px; padding: 24px; }
.dialog h3 { margin: 0 0 16px; }
.input { width: 100%; box-sizing: border-box; padding: 8px 12px; border: 1px solid #ddd; border-radius: 8px; font-size: 14px; margin-bottom: 12px; }
.textarea { width: 100%; box-sizing: border-box; min-height: 70px; padding: 8px 12px; border: 1px solid #ddd; border-radius: 8px; font-size: 14px; margin-bottom: 12px; resize: vertical; }
.form-label { display: block; font-size: 13px; color: #666; margin-bottom: 4px; }
.tag-edit { display: flex; gap: 8px; margin-bottom: 8px; }
.tag-edit .input { margin-bottom: 0; }
.tag-list { display: flex; flex-wrap: wrap; gap: 6px; min-height: 28px; }
.dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
</style>
